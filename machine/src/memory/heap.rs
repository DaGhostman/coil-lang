// ----------- HEAP
/// The default GC threshold when initialize.
const GC_NEXT_THRESHOLD: usize = 1024 * 1024;

/// The default GC threshold growth factor. Each time a GC is performed, we set
/// the next GC threshold to `GC_GROWTH_FACTOR * <current_allocated_bytes>`.
const GC_GROWTH_FACTOR: usize = 2;

/// A managed heap.
///
/// Objects are linked together using an intrusive linked-list, so the heap can
/// traverse all allocated objects.
///
/// In our current design, the heap does not own the objects that it allocated.
/// Instead, the references that we hand out provide shared read/write access to
/// the object. Because we control how the VM is run, we know exactly when an
/// object can be deallocated. Thus, in the context of the VM,  `Gc<T>` is
/// similar to a smart pointer that deallocates itself when it's no longer used.
pub struct Heap {
    // The total number of bytes used by allocated objects.
    alloc_bytes: usize,
    // The byte threshold where a GC should be done.
    gc_next_threshold: usize,
    // The factor by which the byte threshold should grow.
    gc_growth_factor: usize,
    // The table of interned strings.
    strings: Table<()>,
    // The head of the linked list of heap-allocated objects.
    head: Option<Object>,
}

impl Default for Heap {
    fn default() -> Self {
        Self {
            alloc_bytes: 0,
            gc_next_threshold: GC_NEXT_THRESHOLD,
            gc_growth_factor: GC_GROWTH_FACTOR,
            strings: Table::default(),
            head: None,
        }
    }
}

impl Heap {
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
        }
    }

    pub fn trace(&mut self, values: &[u64]) {
        let mut current = self.head;

        let mut gray = Vec::with_capacity(values.len());
        while let Some(reference) = current {
            if !reference.is_marked() && values.contains(&{ reference.addr() }) {
                reference.mark(&mut gray);
            }

            current = reference.get_next();
        }
    }
}

impl Drop for Heap {
    fn drop(&mut self) {
        // Safety: If the heap is drop, both the compiler and VM are no longer
        // in use so.
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
// ----------- OBJECT
use std::{
    cell::Cell,
    error, fmt, mem,
    ops::{self, BitXor, Deref},
    ptr::NonNull,
};

/// A type alias for a heap-allocated string.
pub type RefString = Gc<ObjString>;
pub type RefInstance = Gc<ObjInstance>;

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

/// A numeration of all object types.
#[derive(Clone, Copy)]
pub enum Object {
    /// A string object
    String(RefString),
    Instance(RefInstance),
}

impl Object {
    /// Mark the current object reference and put it in `grey_objects` if its has not been marked.
    pub fn mark(&self, grey_objects: &mut Vec<Self>) {
        let marked = match self {
            Self::String(s) => s.mark(),
            Self::Instance(i) => i.mark(),
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
        }
    }

    /// Return whether the object is marked.
    #[must_use] 
    pub fn is_marked(&self) -> bool {
        match self {
            Self::String(s) => s.is_marked(),
            Self::Instance(i) => i.is_marked(),
        }
    }

    /// Mark all object references that can be directly access by the current object and put them
    /// in `grey_objects` if they have not been marked.
    pub fn mark_references(&self, grey_objects: &mut Vec<Self>) {
        match self {
            Self::String(_) => {}
            Self::Instance(i) => i.as_ref().fields.iter().for_each(|(k, v)| {
                k.mark();

                if let Member::Object(i) = v {
                    i.mark(grey_objects);
                }
            }),
        }
    }

    /// Get the next object reference in the linked list.
    #[must_use] 
    pub fn get_next(&self) -> Option<Self> {
        match self {
            Self::String(s) => s.get_next(),
            Self::Instance(i) => i.get_next(),
        }
    }

    /// Set the next object reference in the linked list.
    pub fn set_next(&self, next: Option<Self>) {
        match self {
            Self::String(s) => s.set_next(next),
            Self::Instance(i) => i.set_next(next),
        }
    }

    #[must_use] 
    pub fn addr(&self) -> u64 {
        match self {
            Self::String(s) => s.as_ptr() as u64,
            Self::Instance(i) => i.as_ptr() as u64,
        }
    }
}

impl GcSized for Object {
    fn size(&self) -> usize {
        match self {
            Self::String(s) => s.size(),
            Self::Instance(i) => i.size(),
        }
    }
}

impl fmt::Display for Object {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::String(s) => write!(f, "{}", s.as_ref()),
            Self::Instance(_) => write!(f, "0x{:08x}", self.addr()),
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
        std::mem::size_of::<Self>() + self.fields.capacity()
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

impl fmt::Display for ObjString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.data)
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

// ----------- TABLE
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
