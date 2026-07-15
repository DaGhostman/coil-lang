//! Mark-and-sweep heap: intrusive object list, string interning, and GC.

use std::collections::{HashMap, HashSet};

const GC_NEXT_THRESHOLD: usize = 1024 * 1024;
const GC_GROWTH_FACTOR: usize = 2;

/// Managed heap. Objects are linked in an intrusive list for traversal.
/// `Gc<T>` handles are copyable; the VM controls when objects become unreachable.
pub struct Heap {
    alloc_bytes: usize,
    gc_next_threshold: usize,
    gc_growth_factor: usize,
    strings: Table<()>,
    head: Option<Object>,
    /// O(1) lookup of live objects by address (updated on alloc/sweep).
    addr_index: HashMap<u64, Object>,
}

impl Default for Heap {
    fn default() -> Self {
        Self {
            alloc_bytes: 0,
            gc_next_threshold: GC_NEXT_THRESHOLD,
            gc_growth_factor: GC_GROWTH_FACTOR,
            strings: Table::default(),
            head: None,
            addr_index: HashMap::new(),
        }
    }
}

impl Heap {
    /// Look up a heap string by address and return a NUL-terminated C string
    /// for FFI. The returned pointer is leaked for the duration of the call.
    #[must_use]
    pub fn cstr_from_addr(&self, addr: u64) -> Option<*const std::os::raw::c_char> {
        let mut current = self.head_for_lookup();
        while let Some(reference) = current {
            if reference.addr() == addr
                && let crate::memory::Object::String(gc) = reference
            {
                let s: std::ffi::CString =
                    std::ffi::CString::new(gc.as_ref().data.as_bytes()).ok()?;
                let boxed: &'static std::ffi::CString = Box::leak(Box::new(s));
                return Some(boxed.as_ptr());
            }
            current = reference.get_next();
        }
        None
    }

    /// Allocates an object and returns its handle. The object is pushed to the
    /// front of the list of allocated objects.
    pub fn alloc<T: GcSized, F>(&mut self, data: T, map: F) -> (Object, Gc<T>)
    where
        F: Fn(Gc<T>) -> Object,
    {
        let boxed = Box::new(GcData::new(self.head, data));
        let content = Gc::new(boxed);
        let object = map(content);
        let size = object.size();
        self.head = Some(object);
        self.alloc_bytes += size;
        self.addr_index.insert(object.addr(), object);

        #[cfg(debug_assertions)]
        println!(
            "0x{:x} alloc {object} ({size} bytes) [{}]",
            object.addr(),
            self.alloc_bytes
        );

        (object, content)
    }

    /// Interns a string and returns its handle. The same reference is returned
    /// for two equal strings.
    pub fn intern(&mut self, data: String) -> RefString {
        let hash = ObjString::hash(&data);
        if let Some(s) = self.strings.find(&data, hash) {
            return s;
        }
        let obj_string = ObjString { data, hash };
        let (_, s) = self.alloc(obj_string, Object::String);
        self.strings.insert(s, ());
        s
    }

    /// Allocate a loaded FFI library as `Object::Library`.
    pub fn alloc_library(
        &mut self,
        library: std::sync::Arc<crate::ffi::Library>,
    ) -> (Object, crate::memory::Gc<ObjLibrary>) {
        let obj_lib = ObjLibrary {
            library,
            signatures: Vec::new(),
            by_name: std::collections::HashMap::new(),
            closures: Vec::new(),
        };
        self.alloc(obj_lib, Object::Library)
    }

    /// Releases all objects that aren't marked. This method also removes
    /// interned strings when no object is referencing them.
    ///
    /// ## Safety
    ///
    /// The caller must ensure that all reachable pointers have been marked.
    /// Otherwise, we'll deallocate objects that are in use and leave dangling
    /// pointers.
    pub unsafe fn sweep(&mut self) {
        let mut prev_obj: Option<Object> = None;
        let mut curr_obj = self.head;

        let mut dangling_strings = Vec::with_capacity(self.strings.len());
        for (k, ()) in self.strings.iter() {
            if !k.is_marked() {
                dangling_strings.push(k);
            }
        }
        for s in dangling_strings {
            self.strings.remove(s);
        }

        while let Some(curr_ref) = curr_obj {
            let next = curr_ref.get_next();
            if curr_ref.is_marked() {
                curr_ref.unmark();
                prev_obj = curr_obj;
                curr_obj = next;
            } else {
                self.addr_index.remove(&curr_ref.addr());
                unsafe { self.dealloc(curr_ref) };
                curr_obj = next;
                if let Some(prev_ref) = prev_obj {
                    prev_ref.set_next(next);
                } else {
                    self.head = curr_obj;
                }
            }
        }

        self.gc_next_threshold = self.alloc_bytes * self.gc_growth_factor;
    }

    /// Returns the number of bytes that are being allocated.
    pub const fn size(&self) -> usize {
        self.alloc_bytes
    }

    /// Returns the next GC threshold in bytes. If `Self::size() > Self::next_gc()`,
    /// we should start tracing all reachable objects and call `Self::sweep`.
    pub const fn next_gc(&self) -> usize {
        #[cfg(not(debug_assertions))]
        {
            self.gc_next_threshold
        }
        #[cfg(debug_assertions)]
        {
            _ = self;
            0
        }
    }

    /// Deallocates an object.
    ///
    /// ## Safety
    ///
    /// + The caller must ensure that no other piece of code will ever use this
    ///   reference. Otherwise, we'll risk dereferencing a dangling pointer.
    /// + Before calling this method, the caller must ensure that the object was
    ///   removed from the linked list of heap-allocated objects.
    unsafe fn dealloc(&mut self, object: Object) {
        let size = object.size();
        self.alloc_bytes -= size;

        #[cfg(debug_assertions)]
        println!(
            "0x{:x} free {object} ({size} bytes) [{}]",
            object.addr(),
            self.alloc_bytes
        );

        match object {
            Object::String(s) => {
                s.release();
            }
            Object::Instance(i) => {
                i.release();
            }
            Object::Enum(e) => {
                e.release();
            }
            Object::Library(l) => {
                l.release();
            }
            Object::Tuple(t) => {
                t.release();
            }
            Object::Array(a) => {
                a.release();
            }
            Object::Coroutine(c) => {
                c.release();
            }
        }
    }

    pub fn trace(&mut self, values: &[u64]) {
        let roots: HashSet<u64> = values.iter().copied().collect();
        let mut current = self.head;

        let mut gray = Vec::with_capacity(values.len());
        while let Some(reference) = current {
            if !reference.is_marked() && roots.contains(&reference.addr()) {
                reference.mark(&mut gray);
            }

            current = reference.get_next();
        }
    }

    /// Head of the intrusive object list (for address lookup).
    pub fn head_for_lookup(&self) -> Option<Object> {
        self.head
    }

    /// Find a heap object by its address (O(1) via addr index).
    pub fn find_object_by_addr(&self, addr: u64) -> Option<Object> {
        self.addr_index.get(&addr).copied()
    }

    /// Write back FFI scratch-buffer values into a live `ObjArray`.
    pub fn update_array_elements(&mut self, addr: u64, values: &[i64]) {
        let mut current = self.head;
        while let Some(reference) = current {
            if reference.addr() == addr {
                if let Object::Array(mut gc) = reference {
                    let arr = gc.as_mut();
                    for (i, &v) in values.iter().enumerate() {
                        if i < arr.elements.len() {
                            arr.elements[i] = Value::from(v);
                        }
                    }
                }
                return;
            }
            current = reference.get_next();
        }
    }

    /// True if `addr` is a live heap object.
    pub fn contains_addr(&self, addr: *mut u8) -> bool {
        self.addr_index.contains_key(&(addr as u64))
    }
}

impl Drop for Heap {
    fn drop(&mut self) {
        for object in &*self {
            unsafe {
                self.dealloc(object);
            }
        }

        debug_assert_eq!(0, self.alloc_bytes);
    }
}

impl IntoIterator for &Heap {
    type Item = Object;

    type IntoIter = HeapIter;

    fn into_iter(self) -> Self::IntoIter {
        Self::IntoIter { next: self.head }
    }
}

/// An iterator through all currently allocated objects.
pub struct HeapIter {
    next: Option<Object>,
}

impl Iterator for HeapIter {
    type Item = Object;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(node) = self.next {
            self.next = node.get_next();
            return Some(node);
        }
        None
    }
}

#[cfg(debug_assertions)]
use std::fmt::Debug;

use std::{
    cell::Cell,
    error, fmt, mem,
    ops::{self, BitXor, Deref},
    ptr::NonNull,
};

pub type RefString = Gc<ObjString>;
pub type RefInstance = Gc<ObjInstance>;
pub type RefEnum = Gc<ObjEnum>;
pub type RefLibrary = Gc<ObjLibrary>;
pub type RefCoroutine = Gc<ObjCoroutine>;

/// Lifecycle of a heap-allocated coroutine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoroState {
    /// Created but never resumed, or suspended at a `yield`.
    Suspended,
    /// Body returned; further `resume` is a no-op (returns default).
    Done,
}

/// An enumeration of all potential errors that occur when working with objects.
#[derive(Debug)]
pub enum Error {
    InvalidCast,
}

impl error::Error for Error {}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCast => write!(f, "Invalid cast."),
        }
    }
}

#[derive(Clone, Copy)]
pub enum Object {
    String(RefString),
    Instance(RefInstance),
    Enum(RefEnum),
    Library(RefLibrary),
    Tuple(crate::memory::Gc<ObjTuple>),
    Array(crate::memory::Gc<ObjArray>),
    Coroutine(RefCoroutine),
}

impl Object {
    /// Mark the current object reference and put it in `grey_objects` if its has not been marked.
    pub fn mark(&self, grey_objects: &mut Vec<Self>) {
        let marked = match self {
            Self::String(s) => s.mark(),
            Self::Instance(i) => i.mark(),
            Self::Enum(e) => e.mark(),
            Self::Library(l) => l.mark(),
            Self::Tuple(t) => t.mark(),
            Self::Array(a) => a.mark(),
            Self::Coroutine(c) => c.mark(),
        };
        if marked {
            grey_objects.push(*self);
        }
    }

    /// Unmark the object.
    pub fn unmark(&self) {
        match self {
            Self::String(s) => s.unmark(),
            Self::Instance(i) => i.unmark(),
            Self::Enum(e) => e.unmark(),
            Self::Library(l) => l.unmark(),
            Self::Tuple(t) => t.unmark(),
            Self::Array(a) => a.unmark(),
            Self::Coroutine(c) => c.unmark(),
        }
    }

    /// Return whether the object is marked.
    #[must_use]
    pub fn is_marked(&self) -> bool {
        match self {
            Self::String(s) => s.is_marked(),
            Self::Instance(i) => i.is_marked(),
            Self::Enum(e) => e.is_marked(),
            Self::Library(l) => l.is_marked(),
            Self::Tuple(t) => t.is_marked(),
            Self::Array(a) => a.is_marked(),
            Self::Coroutine(c) => c.is_marked(),
        }
    }

    /// Mark direct heap references held by this object.
    pub fn mark_references(&self, grey_objects: &mut Vec<Self>) {
        match self {
            Self::String(_) => {}
            Self::Instance(i) => i.as_ref().fields.iter().for_each(|(k, v)| {
                k.mark();

                if let Member::Object(i) = v {
                    i.mark(grey_objects);
                }
            }),
            Self::Enum(e) => {
                for member in &e.as_ref().payload {
                    if let Member::Object(o) = member {
                        o.mark(grey_objects);
                    }
                }
            }
            Self::Library(_) => {}
            // Tuple/array/coroutine saved stacks are traced in `Machine::gc_collect`.
            Self::Tuple(_) => {}
            Self::Array(_) => {}
            Self::Coroutine(_) => {}
        }
    }

    /// Get the next object reference in the linked list.
    #[must_use]
    pub fn get_next(&self) -> Option<Self> {
        match self {
            Self::String(s) => s.get_next(),
            Self::Instance(i) => i.get_next(),
            Self::Enum(e) => e.get_next(),
            Self::Library(l) => l.get_next(),
            Self::Tuple(t) => t.get_next(),
            Self::Array(a) => a.get_next(),
            Self::Coroutine(c) => c.get_next(),
        }
    }

    /// Set the next object reference in the linked list.
    pub fn set_next(&self, next: Option<Self>) {
        match self {
            Self::String(s) => s.set_next(next),
            Self::Instance(i) => i.set_next(next),
            Self::Enum(e) => e.set_next(next),
            Self::Library(l) => l.set_next(next),
            Self::Tuple(t) => t.set_next(next),
            Self::Array(a) => a.set_next(next),
            Self::Coroutine(c) => c.set_next(next),
        }
    }

    #[must_use]
    pub fn addr(&self) -> u64 {
        match self {
            Self::String(s) => s.as_ptr() as u64,
            Self::Instance(i) => i.as_ptr() as u64,
            Self::Enum(e) => e.as_ptr() as u64,
            Self::Library(l) => l.as_ptr() as u64,
            Self::Tuple(t) => t.as_ptr() as u64,
            Self::Array(a) => a.as_ptr() as u64,
            Self::Coroutine(c) => c.as_ptr() as u64,
        }
    }
}

impl GcSized for Object {
    fn size(&self) -> usize {
        match self {
            Self::String(s) => s.size(),
            Self::Instance(i) => i.size(),
            Self::Enum(e) => e.size(),
            Self::Library(l) => l.size(),
            Self::Tuple(t) => t.size(),
            Self::Array(a) => a.size(),
            Self::Coroutine(c) => c.size(),
        }
    }
}

impl fmt::Display for Object {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::String(s) => write!(f, "{}", s.as_ref()),
            Self::Instance(_) => write!(f, "0x{:08x}", self.addr()),
            Self::Enum(_) => write!(f, "0x{:08x}", self.addr()),
            Self::Library(_) => write!(f, "0x{:08x}", self.addr()),
            Self::Tuple(t) => write!(f, "{}", t.as_ref()),
            Self::Array(a) => write!(f, "{}", a.as_ref()),
            Self::Coroutine(c) => write!(f, "{}", c.as_ref()),
        }
    }
}

impl Object {
    /// C string pointer for FFI; non-strings return null.
    pub fn as_cstr(&self) -> *const std::os::raw::c_char {
        match self {
            Self::String(s) => s.data.data.as_ptr() as *const std::os::raw::c_char,
            Self::Instance(_)
            | Self::Enum(_)
            | Self::Library(_)
            | Self::Tuple(_)
            | Self::Array(_)
            | Self::Coroutine(_) => std::ptr::null(),
        }
    }
}

#[derive(Clone, Copy)]
pub enum Member {
    Value(Value),
    Object(Object),
}

pub struct ObjInstance {
    fields: Table<Member>,
}

impl ObjInstance {
    #[must_use]
    pub fn default() -> Self {
        Self {
            fields: Table::default(),
        }
    }

    pub fn set(&mut self, key: RefString, value: Member) {
        self.fields.insert(key, value);
    }

    pub fn get(&self, key: RefString) -> Option<Member> {
        self.fields.get(key)
    }
}

impl GcSized for ObjInstance {
    fn size(&self) -> usize {
        // `Table` entry storage uses Rust's global allocator, not the VM heap.
        std::mem::size_of::<Self>()
    }
}

/// Heap-allocated enum variant (`tag` + flat `Member` payload).
pub struct ObjEnum {
    pub tag: u32,
    pub payload: Vec<Member>,
}

impl GcSized for ObjEnum {
    fn size(&self) -> usize {
        std::mem::size_of::<Self>() + self.payload.capacity() * std::mem::size_of::<Member>()
    }
}

/// The content of a heap-allocated string object.
pub struct ObjString {
    pub data: String,
    pub hash: u32,
}

impl ObjString {
    #[must_use]
    pub fn hash(s: &str) -> u32 {
        let mut hash = 2_166_136_261;
        for b in s.bytes() {
            hash = hash.bitxor(u32::from(b));
            hash = hash.wrapping_mul(16_777_619);
        }
        hash
    }
}

impl GcSized for ObjString {
    fn size(&self) -> usize {
        mem::size_of::<Self>() + mem::size_of_val(&*self.data)
    }
}

impl From<&str> for ObjString {
    fn from(value: &str) -> Self {
        let data = String::from(value);
        let hash = Self::hash(value);
        Self { data, hash }
    }
}

pub struct ObjTuple {
    pub elements: Vec<Value>,
}

pub struct ObjArray {
    pub elements: Vec<Value>,
}

/// Suspended async function state: saved stack segment + call frames.
pub struct ObjCoroutine {
    pub state: CoroState,
    pub resume_ip: usize,
    /// Stack segment (args + locals + operands) relative to segment base 0.
    pub saved_stack: Vec<Value>,
    /// `(ip, sp_offset)` pairs; `sp_offset` is relative to the coroutine segment base.
    pub saved_frames: Vec<(usize, usize)>,
    /// Value from the resumer's `resume h with v` (delivered at the next binding yield).
    pub pending_send: Value,
    /// Active `yield from` delegate, if any.
    pub yield_from: Option<RefCoroutine>,
    /// Outer continuation IP when the delegate completes.
    pub yield_from_resume_ip: usize,
}

impl GcSized for ObjTuple {
    fn size(&self) -> usize {
        mem::size_of::<Self>() + self.elements.capacity() * mem::size_of::<Value>()
    }
}

impl GcSized for ObjArray {
    fn size(&self) -> usize {
        mem::size_of::<Self>() + self.elements.capacity() * mem::size_of::<Value>()
    }
}

impl GcSized for ObjCoroutine {
    fn size(&self) -> usize {
        // `saved_stack` / `saved_frames` use Rust's allocator, not the VM
        // heap byte counter (same contract as `ObjInstance`).
        mem::size_of::<Self>()
    }
}

impl fmt::Display for ObjTuple {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "({})",
            self.elements
                .iter()
                .map(|v| format!("{}", v.as_int()))
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

impl fmt::Display for ObjArray {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "[{}]",
            self.elements
                .iter()
                .map(|v| format!("{}", v.as_int()))
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

impl fmt::Display for ObjCoroutine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<coroutine {:?}>", self.state)
    }
}

impl fmt::Display for ObjString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.data)
    }
}

/// Loaded shared library plus cached FFI signatures.
pub struct ObjLibrary {
    pub library: std::sync::Arc<crate::ffi::Library>,
    pub signatures: Vec<RegisteredFunction>,
    pub by_name: std::collections::HashMap<String, usize>,
    /// libffi closures registered for callbacks (keeps trampolines alive).
    pub closures: Vec<crate::ffi::OwnedClosure>,
}

/// C signature metadata for an FFI function.
#[derive(Clone, Debug)]
pub struct FunctionSig {
    pub name: String,
    pub arity: usize,
    pub arg_types: Vec<FfiType>,
    pub ret_type: FfiType,
}

impl FunctionSig {
    pub fn from_ffi_signature(sig: &crate::ffi::FfiSignature) -> Self {
        Self {
            name: sig.name.clone(),
            arity: sig.arity(),
            arg_types: sig.args.clone(),
            ret_type: sig.ret,
        }
    }
}

/// A declared FFI function with a prepared libffi call interface.
pub struct RegisteredFunction {
    pub sig: FunctionSig,
    pub prepared: crate::ffi::PreparedCall,
}

impl RegisteredFunction {
    pub fn ffi_signature(&self) -> crate::ffi::FfiSignature {
        crate::ffi::FfiSignature {
            name: self.sig.name.clone(),
            args: self.sig.arg_types.clone(),
            ret: self.sig.ret_type,
        }
    }
}

/// C ABI type tags for FFI marshalling.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FfiType {
    Int,
    Float,
    String,
    Void,
    Bool,
    Int8,
    Int16,
    Int32,
    UInt8,
    UInt16,
    UInt32,
    UInt64,
    Ptr,
    Callback(u32),
    Struct(u32),
}

impl FfiType {
    pub fn from_tag(tag: u32, aux: u32) -> Self {
        use common::tag as t;
        match tag {
            x if x == t::FLOAT => Self::Float,
            x if x == t::STRING => Self::String,
            x if x == t::VOID => Self::Void,
            x if x == t::BOOL => Self::Bool,
            x if x == t::INT8 => Self::Int8,
            x if x == t::INT16 => Self::Int16,
            x if x == t::INT32 => Self::Int32,
            x if x == t::UINT8 => Self::UInt8,
            x if x == t::UINT16 => Self::UInt16,
            x if x == t::UINT32 => Self::UInt32,
            x if x == t::UINT64 => Self::UInt64,
            x if x == t::PTR => Self::Ptr,
            x if x == t::CALLBACK => Self::Callback(aux),
            x if x == t::STRUCT => Self::Struct(aux),
            _ => Self::Int,
        }
    }

    pub fn tag(&self) -> u32 {
        use common::tag as t;
        match self {
            Self::Int => t::INT,
            Self::Float => t::FLOAT,
            Self::String => t::STRING,
            Self::Void => t::VOID,
            Self::Bool => t::BOOL,
            Self::Int8 => t::INT8,
            Self::Int16 => t::INT16,
            Self::Int32 => t::INT32,
            Self::UInt8 => t::UINT8,
            Self::UInt16 => t::UINT16,
            Self::UInt32 => t::UINT32,
            Self::UInt64 => t::UINT64,
            Self::Ptr => t::PTR,
            Self::Callback(_) => t::CALLBACK,
            Self::Struct(_) => t::STRUCT,
        }
    }

    pub fn aux(&self) -> u32 {
        match self {
            Self::Callback(id) | Self::Struct(id) => *id,
            _ => 0,
        }
    }

    pub fn is_void(self) -> bool {
        matches!(self, Self::Void)
    }
}

/// C-layout struct descriptor for pass-by-value FFI.
#[derive(Clone, Debug)]
pub struct CStructLayout {
    pub name: String,
    pub fields: Vec<(String, FfiType)>,
}

impl GcSized for ObjLibrary {
    fn size(&self) -> usize {
        mem::size_of::<Self>() + mem::size_of_val(&*self.library)
    }
}

impl fmt::Display for ObjLibrary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "<library at 0x{:x}, {} function(s)>",
            std::sync::Arc::as_ptr(&self.library) as u64,
            self.signatures.len()
        )
    }
}

pub trait GcSized {
    fn size(&self) -> usize;
}

pub struct GcData<T> {
    marked: Cell<bool>,
    next: Cell<Option<Object>>,
    data: T,
}

impl<T> GcData<T> {
    pub const fn new(next: Option<Object>, data: T) -> Self {
        Self {
            marked: Cell::new(false),
            next: Cell::new(next),
            data,
        }
    }

    pub const fn get_next(&self) -> Option<Object> {
        self.next.get()
    }

    pub fn set_next(&self, next: Option<Object>) {
        self.next.set(next);
    }

    pub const fn is_marked(&self) -> bool {
        self.marked.get()
    }

    pub fn mark(&self) -> bool {
        let is_not_marked = !self.marked.get();
        if is_not_marked {
            self.marked.set(true);
        }
        is_not_marked
    }

    pub fn unmark(&self) {
        self.marked.set(false);
    }
}

impl<T> AsRef<T> for GcData<T> {
    fn as_ref(&self) -> &T {
        &self.data
    }
}

impl<T> AsMut<T> for GcData<T> {
    fn as_mut(&mut self) -> &mut T {
        &mut self.data
    }
}

impl<T: GcSized> GcSized for GcData<T> {
    fn size(&self) -> usize {
        mem::size_of_val(&self.next) + mem::size_of_val(&self.marked) + self.data.size()
    }
}

impl<T: GcSized + Copy> GcSized for Cell<T> {
    fn size(&self) -> usize {
        self.get().size()
    }
}

pub struct Gc<T> {
    ptr: NonNull<GcData<T>>,
}

impl<T> Gc<T> {
    #[must_use]
    pub fn new(boxed: Box<GcData<T>>) -> Self {
        Self {
            ptr: NonNull::from(Box::leak(boxed)),
        }
    }

    pub fn release(self) {
        _ = unsafe { Box::from_raw(self.ptr.as_ptr()) };
    }

    #[must_use]
    pub fn ptr_eq(lhs: Self, rhs: Self) -> bool {
        lhs.ptr.eq(&rhs.ptr)
    }

    #[must_use]
    pub const fn as_ptr(&self) -> *const GcData<T> {
        self.ptr.as_ptr()
    }

    /// Mutable access to the inner payload (single-threaded VM only).
    pub fn payload_mut(&self) -> &mut T {
        unsafe {
            let ptr = self.ptr.as_ptr().cast::<GcData<T>>();
            (*ptr).as_mut()
        }
    }
}

impl<T: GcSized> GcSized for Gc<T> {
    fn size(&self) -> usize {
        self.deref().size()
    }
}

impl<T> ops::Deref for Gc<T> {
    type Target = GcData<T>;

    fn deref(&self) -> &Self::Target {
        unsafe { self.ptr.as_ref() }
    }
}

impl<T> ops::DerefMut for Gc<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { self.ptr.as_mut() }
    }
}

impl<T> Copy for Gc<T> {}
impl<T> Clone for Gc<T> {
    fn clone(&self) -> Self {
        *self
    }
}

// Open-addressing hash table keyed by interned strings.

use std::{alloc, cell::UnsafeCell, marker::PhantomData};

use common::Value;

pub struct Table<V>(UnsafeCell<Store<V>>);

impl<V> Default for Table<V> {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl<V> Table<V> {
    #[inline]
    #[must_use]
    pub const fn new() -> Self {
        Self(UnsafeCell::new(Store::new()))
    }

    #[inline]
    pub fn len(&self) -> usize {
        let store = unsafe { &*self.0.get() };
        store.lives
    }

    #[inline]
    pub fn capacity(&self) -> usize {
        let store = unsafe { &*self.0.get() };
        store.cap
    }

    #[inline]
    pub fn get(&self, key: RefString) -> Option<V>
    where
        V: Copy,
    {
        let store = unsafe { &*self.0.get() };
        store.get(key)
    }

    #[inline]
    pub fn find(&self, s: &str, hash: u32) -> Option<RefString> {
        let store = unsafe { &*self.0.get() };
        store.find(s, hash)
    }

    #[inline]
    pub fn insert(&self, key: RefString, val: V) -> Option<V> {
        let store = unsafe { &mut *self.0.get() };
        store.insert(key, val)
    }

    #[inline]
    pub fn remove(&self, key: RefString) -> Option<V> {
        let store = unsafe { &mut *self.0.get() };
        store.remove(key)
    }

    #[inline]
    pub fn iter(&self) -> Iter<'_, V>
    where
        V: Copy,
    {
        let store = unsafe { &*self.0.get() };
        store.into_iter()
    }
}

pub struct Iter<'store, V> {
    ptr: NonNull<Entry<V>>,
    idx: usize,
    cap: usize,
    marker: PhantomData<&'store Store<V>>,
}

impl<V> Iterator for Iter<'_, V>
where
    V: Copy,
{
    type Item = (RefString, V);

    fn next(&mut self) -> Option<Self::Item> {
        while self.idx < self.cap {
            let entry = unsafe { &*self.ptr.as_ptr().add(self.idx) };
            self.idx += 1;
            if let Entry::Live(x) = entry {
                return Some((x.key, x.val));
            }
        }
        None
    }
}

struct Store<V> {
    lives: usize,
    deads: usize,
    cap: usize,
    ptr: NonNull<Entry<V>>,
}

impl<V> Drop for Store<V> {
    fn drop(&mut self) {
        if self.cap > 0 {
            let entries = NonNull::slice_from_raw_parts(self.ptr, self.cap);
            unsafe {
                NonNull::drop_in_place(entries);
                Self::dealloc(self.ptr, self.cap);
            }
        }
    }
}

impl<'store, V> IntoIterator for &'store Store<V>
where
    V: Copy,
{
    type Item = (RefString, V);

    type IntoIter = Iter<'store, V>;

    fn into_iter(self) -> Self::IntoIter {
        Self::IntoIter {
            ptr: self.ptr,
            idx: 0,
            cap: self.cap,
            marker: PhantomData,
        }
    }
}

impl<V> Store<V> {
    const fn new() -> Self {
        Self {
            ptr: NonNull::dangling(),
            cap: 0,
            lives: 0,
            deads: 0,
        }
    }

    fn get(&self, key: RefString) -> Option<V>
    where
        V: Copy,
    {
        if self.lives == 0 {
            return None;
        }
        let entry_ptr = unsafe { Self::probe(self.cap, self.ptr, key) };
        let entry = unsafe { entry_ptr.as_ref() };
        let Entry::Live(e) = entry else {
            return None;
        };
        Some(e.val)
    }

    fn find(&self, s: &str, hash: u32) -> Option<RefString> {
        if self.lives == 0 {
            return None;
        }
        let mut index = hash as usize & (self.cap - 1);
        loop {
            let entry_ptr = unsafe { self.ptr.add(index) };
            let entry = unsafe { entry_ptr.as_ref() };
            match entry {
                Entry::Free => return None,
                Entry::Live(entry) if entry.key.as_ref().data == s => {
                    return Some(entry.key);
                }
                _ => {}
            }
            index = (index + 1) & (self.cap - 1);
        }
    }

    fn insert(&mut self, key: RefString, val: V) -> Option<V> {
        if self.lives + self.deads >= self.cap * 3 / 4 {
            self.resize();
        }
        let mut entry_ptr = unsafe { Self::probe(self.cap, self.ptr, key) };
        let entry = unsafe { entry_ptr.as_mut() };
        match mem::replace(entry, Entry::Live(EntryInner { key, val })) {
            Entry::Free => {
                self.lives += 1;
                None
            }
            Entry::Dead => {
                self.lives += 1;
                self.deads -= 1;
                None
            }
            Entry::Live(e) => Some(e.val),
        }
    }

    fn remove(&mut self, key: RefString) -> Option<V> {
        if self.lives == 0 {
            return None;
        }
        let mut entry_ptr = unsafe { Self::probe(self.cap, self.ptr, key) };
        let entry = unsafe { entry_ptr.as_mut() };
        let Entry::Live(entry_old) = mem::replace(entry, Entry::Dead) else {
            return None;
        };
        self.lives -= 1;
        self.deads += 1;
        Some(entry_old.val)
    }

    unsafe fn probe(cap: usize, ptr: NonNull<Entry<V>>, key: RefString) -> NonNull<Entry<V>> {
        let mut dead = None;
        let mut index = key.as_ref().hash as usize & (cap - 1);
        loop {
            let entry_ptr = unsafe { ptr.add(index) };
            match unsafe { entry_ptr.as_ref() } {
                Entry::Free => {
                    return dead.unwrap_or(entry_ptr);
                }
                Entry::Dead if dead.is_none() => {
                    dead = Some(entry_ptr);
                }
                Entry::Live(e) if Gc::ptr_eq(e.key, key) => {
                    return entry_ptr;
                }
                _ => {}
            }
            index = (index + 1) & (cap - 1);
        }
    }

    fn resize(&mut self) {
        let new_cap = self
            .cap
            .checked_mul(2)
            .expect("capacity does not overflow")
            .max(8);

        let new_ptr = Self::alloc(new_cap);
        if self.cap > 0 {
            for i in 0..self.cap {
                let old_entry_ptr = unsafe { self.ptr.add(i) };
                if let Entry::Live(e) = unsafe { old_entry_ptr.as_ref() } {
                    let new_entry_ptr = unsafe { Self::probe(new_cap, new_ptr, e.key) };
                    unsafe {
                        NonNull::swap(old_entry_ptr, new_entry_ptr);
                    }
                }
            }
            unsafe {
                Self::dealloc(self.ptr, self.cap);
            }
        }
        self.deads = 0;
        self.cap = new_cap;
        self.ptr = new_ptr;
    }

    fn layout(cap: usize) -> alloc::Layout {
        alloc::Layout::array::<Entry<V>>(cap).expect("a valid array layout")
    }

    fn alloc(cap: usize) -> NonNull<Entry<V>> {
        let layout = Self::layout(cap);
        let nullable = unsafe { alloc::alloc(layout) };
        let Some(ptr) = NonNull::new(nullable.cast()) else {
            alloc::handle_alloc_error(layout);
        };
        for i in 0..cap {
            unsafe {
                ptr.add(i).write(Entry::Free);
            }
        }
        ptr
    }

    unsafe fn dealloc(ptr: NonNull<Entry<V>>, cap: usize) {
        unsafe {
            std::alloc::dealloc(ptr.as_ptr().cast(), Self::layout(cap));
        }
    }
}

enum Entry<V> {
    Free,
    Dead,
    Live(EntryInner<V>),
}

struct EntryInner<V> {
    key: RefString,
    val: V,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live_object_addrs(heap: &Heap) -> std::collections::HashSet<u64> {
        let mut addrs = std::collections::HashSet::new();
        for obj in heap {
            addrs.insert(obj.addr());
        }
        addrs
    }

    #[test]
    fn enum_gc_marks_payload_pointers() {
        let mut heap = Heap::default();

        // 1. Allocate the inner object (a string).
        let (string_obj, string_ref) = heap.alloc(ObjString::from("inner"), Object::String);
        let string_addr = string_obj.addr();
        let string_member = Member::Object(string_obj);

        // 2. Allocate the enum with the string in its payload.
        let enum_value = ObjEnum {
            tag: 0,
            payload: vec![string_member],
        };
        let (enum_obj, _enum_ref) = heap.alloc(enum_value, Object::Enum);
        let enum_addr = enum_obj.addr();

        // 3. Mark the enum as a root and propagate the mark to its
        //    payload (which holds the string pointer).
        let mut gray = Vec::new();
        heap.trace(&[enum_addr]);
        enum_obj.mark_references(&mut gray);

        // 4. Sweep — anything not marked is deallocated.
        unsafe { heap.sweep() };

        // 5. Both objects must still be alive.
        let live = live_object_addrs(&heap);
        assert!(
            live.contains(&string_addr),
            "string at 0x{:x} was collected despite being reachable from enum payload",
            string_addr
        );
        assert!(
            live.contains(&enum_addr),
            "enum at 0x{:x} was collected despite being a GC root",
            enum_addr
        );
        // Sanity: the string ref is still dereferenceable.
        let _ = string_ref.as_ref();
    }

    #[test]
    fn enum_gc_marks_nested_enum_payloads() {
        let mut heap = Heap::default();

        // Inner enum: empty payload.
        let (inner_obj, _inner_ref) = heap.alloc(
            ObjEnum {
                tag: 1,
                payload: vec![],
            },
            Object::Enum,
        );
        let inner_addr = inner_obj.addr();

        // Outer enum: payload contains the inner enum as a
        // `Member::Object`.
        let outer = ObjEnum {
            tag: 0,
            payload: vec![Member::Object(inner_obj)],
        };
        let (outer_obj, _outer_ref) = heap.alloc(outer, Object::Enum);
        let outer_addr = outer_obj.addr();

        // Mark outer as root, propagate through its payload to mark
        // the inner enum.
        let mut gray = Vec::new();
        heap.trace(&[outer_addr]);
        outer_obj.mark_references(&mut gray);

        // Drain the grey stack — each newly-marked object should
        // also have its references traced. For the inner enum
        // (empty payload) this is a no-op, but we still call it to
        // exercise the arm.
        while let Some(obj) = gray.pop() {
            obj.mark_references(&mut gray);
        }

        // Sweep.
        unsafe { heap.sweep() };

        // Both must survive.
        let live = live_object_addrs(&heap);
        assert!(
            live.contains(&inner_addr),
            "inner enum at 0x{:x} was collected despite being reachable from outer enum payload",
            inner_addr
        );
        assert!(
            live.contains(&outer_addr),
            "outer enum at 0x{:x} was collected despite being a GC root",
            outer_addr
        );
    }
}
