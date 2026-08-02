//! Host-backed PCRE2 regex (`use regex::*`).
//!
//! See [`REGEX_WIRING`] for pipeline `HostInvoke` registry names and arities.

use pcre2::bytes::{Captures, Regex, RegexBuilder};

use common::{BUILTIN_REGEX_ERROR_VARIANTS, BUILTIN_RESULT_VARIANTS, Value};

use crate::io::{alloc_result_err, alloc_result_ok};
use crate::memory::{Heap, Member, ObjArray, ObjEnum, ObjTuple, Object};
use crate::regex_state::ObjRegex;

/// Tag indices for [`RegexError`](common::BUILTIN_REGEX_ERROR_ENUM).
#[repr(u32)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RegexErrorTag {
    Compile = 0,
    Runtime = 1,
    NoMatch = 2,
    Utf8 = 3,
}

fn alloc_enum(heap: &mut Heap, tag: u32, payload: Vec<Member>) -> Value {
    let (obj, _) = heap.alloc(ObjEnum { tag, payload }, Object::Enum);
    Value::from(obj.addr())
}

/// Allocate a unit-payload `RegexError` variant.
pub fn alloc_regex_error(heap: &mut Heap, tag: RegexErrorTag) -> Value {
    let _ = BUILTIN_REGEX_ERROR_VARIANTS;
    alloc_enum(heap, tag as u32, vec![])
}

fn alloc_result_regex_err(heap: &mut Heap, tag: RegexErrorTag) -> Value {
    let _ = BUILTIN_RESULT_VARIANTS;
    let err = alloc_regex_error(heap, tag);
    alloc_result_err(heap, err)
}

fn as_result_value(heap: &mut Heap, r: Result<Value, RegexErrorTag>) -> Value {
    match r {
        Ok(v) => alloc_result_ok(heap, v),
        Err(tag) => alloc_result_regex_err(heap, tag),
    }
}

fn heap_string(heap: &Heap, v: Value) -> Result<String, RegexErrorTag> {
    match heap.find_object_by_addr(v.raw() as u64) {
        Some(Object::String(gc)) => Ok(gc.as_ref().data.clone()),
        _ => Err(RegexErrorTag::Compile),
    }
}

fn intern_string(heap: &mut Heap, s: &str) -> Value {
    let gc = heap.intern(s.to_string());
    Value::from(gc.as_ptr() as *mut u8 as u64)
}

fn alloc_string_array(heap: &mut Heap, strings: &[String]) -> Value {
    let elements: Vec<Value> = strings.iter().map(|s| intern_string(heap, s)).collect();
    let (obj, _) = heap.alloc(ObjArray { elements }, Object::Array);
    Value::from(obj.addr())
}

fn alloc_span_tuple(heap: &mut Heap, start: i64, end: i64) -> Value {
    let (obj, _) = heap.alloc(
        ObjTuple {
            elements: vec![Value::from(start), Value::from(end)],
        },
        Object::Tuple,
    );
    Value::from(obj.addr())
}

fn bytes_to_string(bytes: &[u8]) -> Result<String, RegexErrorTag> {
    String::from_utf8(bytes.to_vec()).map_err(|_| RegexErrorTag::Utf8)
}

/// Map flags string letters onto [`RegexBuilder`] (`i`/`m`/`s`/`x`/`u` + always UTF).
fn builder_from_flags(flags: &str) -> Result<RegexBuilder, RegexErrorTag> {
    let mut b = RegexBuilder::new();
    b.utf(true);
    for ch in flags.chars() {
        match ch {
            'i' => {
                b.caseless(true);
            }
            'm' => {
                b.multi_line(true);
            }
            's' => {
                b.dotall(true);
            }
            'x' => {
                b.extended(true);
            }
            'u' => {
                b.ucp(true);
            }
            _ => return Err(RegexErrorTag::Compile),
        }
    }
    Ok(b)
}

fn with_regex<R>(
    heap: &mut Heap,
    handle: Value,
    f: impl FnOnce(&Regex) -> Result<R, RegexErrorTag>,
) -> Result<R, RegexErrorTag> {
    heap.with_regex(handle.raw() as u64, |obj| f(&obj.re))
        .ok_or(RegexErrorTag::Runtime)?
}

fn expand_replacement(template: &str, caps: &Captures<'_>) -> Result<String, RegexErrorTag> {
    let mut out = String::new();
    let bytes = template.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'$' {
            out.push(bytes[i] as char);
            i += 1;
            continue;
        }
        i += 1;
        if i >= bytes.len() {
            out.push('$');
            break;
        }
        if bytes[i] == b'$' {
            out.push('$');
            i += 1;
            continue;
        }
        if bytes[i] == b'{' {
            i += 1;
            let start = i;
            while i < bytes.len() && bytes[i] != b'}' {
                i += 1;
            }
            if i >= bytes.len() {
                return Err(RegexErrorTag::Runtime);
            }
            let name = std::str::from_utf8(&bytes[start..i]).map_err(|_| RegexErrorTag::Utf8)?;
            i += 1;
            if let Ok(idx) = name.parse::<usize>() {
                if let Some(m) = caps.get(idx) {
                    out.push_str(&bytes_to_string(m.as_bytes())?);
                }
            } else if let Some(m) = caps.name(name) {
                out.push_str(&bytes_to_string(m.as_bytes())?);
            }
            continue;
        }
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            let idx: usize = std::str::from_utf8(&bytes[start..i])
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(usize::MAX);
            if let Some(m) = caps.get(idx) {
                out.push_str(&bytes_to_string(m.as_bytes())?);
            }
            continue;
        }
        // Unknown `$` escape — emit literally.
        out.push('$');
        out.push(bytes[i] as char);
        i += 1;
    }
    Ok(out)
}

fn capture_row(caps: &Captures<'_>) -> Result<Vec<String>, RegexErrorTag> {
    let mut row = Vec::with_capacity(caps.len());
    for i in 0..caps.len() {
        match caps.get(i) {
            Some(m) => row.push(bytes_to_string(m.as_bytes())?),
            None => row.push(String::new()),
        }
    }
    Ok(row)
}

pub fn host_regex_compile(heap: &mut Heap, args: &[Value]) -> Value {
    let r = try_compile(heap, args);
    as_result_value(heap, r)
}

fn try_compile(heap: &mut Heap, args: &[Value]) -> Result<Value, RegexErrorTag> {
    let pattern = heap_string(heap, args[0])?;
    let flags = heap_string(heap, args[1])?;
    let builder = builder_from_flags(&flags)?;
    let re = builder
        .build(&pattern)
        .map_err(|_| RegexErrorTag::Compile)?;
    let (obj, _) = heap.alloc(ObjRegex::new(re), Object::Regex);
    Ok(Value::from(obj.addr()))
}

pub fn host_regex_is_match(heap: &mut Heap, args: &[Value]) -> Value {
    let r = try_is_match(heap, args);
    as_result_value(heap, r)
}

fn try_is_match(heap: &mut Heap, args: &[Value]) -> Result<Value, RegexErrorTag> {
    let subject = heap_string(heap, args[1])?;
    let matched = with_regex(heap, args[0], |re| {
        re.is_match(subject.as_bytes())
            .map_err(|_| RegexErrorTag::Runtime)
    })?;
    Ok(Value::from(matched))
}

pub fn host_regex_find(heap: &mut Heap, args: &[Value]) -> Value {
    let r = try_find(heap, args);
    as_result_value(heap, r)
}

fn try_find(heap: &mut Heap, args: &[Value]) -> Result<Value, RegexErrorTag> {
    let subject = heap_string(heap, args[1])?;
    let span = with_regex(heap, args[0], |re| {
        match re
            .find(subject.as_bytes())
            .map_err(|_| RegexErrorTag::Runtime)?
        {
            Some(m) => Ok((m.start() as i64, m.end() as i64)),
            None => Err(RegexErrorTag::NoMatch),
        }
    })?;
    Ok(alloc_span_tuple(heap, span.0, span.1))
}

pub fn host_regex_find_all(heap: &mut Heap, args: &[Value]) -> Value {
    let r = try_find_all(heap, args);
    as_result_value(heap, r)
}

fn try_find_all(heap: &mut Heap, args: &[Value]) -> Result<Value, RegexErrorTag> {
    let subject = heap_string(heap, args[1])?;
    let spans = with_regex(heap, args[0], |re| {
        let mut out = Vec::new();
        for m in re.find_iter(subject.as_bytes()) {
            let m = m.map_err(|_| RegexErrorTag::Runtime)?;
            out.push((m.start() as i64, m.end() as i64));
        }
        Ok(out)
    })?;
    let elements: Vec<Value> = spans
        .into_iter()
        .map(|(s, e)| alloc_span_tuple(heap, s, e))
        .collect();
    let (obj, _) = heap.alloc(ObjArray { elements }, Object::Array);
    Ok(Value::from(obj.addr()))
}

pub fn host_regex_captures(heap: &mut Heap, args: &[Value]) -> Value {
    let r = try_captures(heap, args);
    as_result_value(heap, r)
}

fn try_captures(heap: &mut Heap, args: &[Value]) -> Result<Value, RegexErrorTag> {
    let subject = heap_string(heap, args[1])?;
    let row = with_regex(heap, args[0], |re| {
        match re
            .captures(subject.as_bytes())
            .map_err(|_| RegexErrorTag::Runtime)?
        {
            Some(caps) => capture_row(&caps),
            None => Err(RegexErrorTag::NoMatch),
        }
    })?;
    Ok(alloc_string_array(heap, &row))
}

pub fn host_regex_captures_all(heap: &mut Heap, args: &[Value]) -> Value {
    let r = try_captures_all(heap, args);
    as_result_value(heap, r)
}

fn try_captures_all(heap: &mut Heap, args: &[Value]) -> Result<Value, RegexErrorTag> {
    let subject = heap_string(heap, args[1])?;
    let rows = with_regex(heap, args[0], |re| {
        let mut out = Vec::new();
        for caps in re.captures_iter(subject.as_bytes()) {
            let caps = caps.map_err(|_| RegexErrorTag::Runtime)?;
            out.push(capture_row(&caps)?);
        }
        Ok(out)
    })?;
    let elements: Vec<Value> = rows
        .into_iter()
        .map(|row| alloc_string_array(heap, &row))
        .collect();
    let (obj, _) = heap.alloc(ObjArray { elements }, Object::Array);
    Ok(Value::from(obj.addr()))
}

pub fn host_regex_split(heap: &mut Heap, args: &[Value]) -> Value {
    let r = try_split(heap, args);
    as_result_value(heap, r)
}

fn try_split(heap: &mut Heap, args: &[Value]) -> Result<Value, RegexErrorTag> {
    let subject = heap_string(heap, args[1])?;
    let parts = with_regex(heap, args[0], |re| {
        let bytes = subject.as_bytes();
        let mut parts = Vec::new();
        let mut last = 0usize;
        for m in re.find_iter(bytes) {
            let m = m.map_err(|_| RegexErrorTag::Runtime)?;
            parts.push(bytes_to_string(&bytes[last..m.start()])?);
            last = m.end();
        }
        parts.push(bytes_to_string(&bytes[last..])?);
        Ok(parts)
    })?;
    Ok(alloc_string_array(heap, &parts))
}

pub fn host_regex_replace(heap: &mut Heap, args: &[Value]) -> Value {
    let r = try_replace(heap, args, false);
    as_result_value(heap, r)
}

pub fn host_regex_replace_all(heap: &mut Heap, args: &[Value]) -> Value {
    let r = try_replace(heap, args, true);
    as_result_value(heap, r)
}

fn try_replace(heap: &mut Heap, args: &[Value], all: bool) -> Result<Value, RegexErrorTag> {
    let subject = heap_string(heap, args[1])?;
    let template = heap_string(heap, args[2])?;
    let replaced = with_regex(heap, args[0], |re| {
        let bytes = subject.as_bytes();
        let mut out = Vec::new();
        let mut last = 0usize;
        let mut replaced_once = false;
        for caps in re.captures_iter(bytes) {
            let caps = caps.map_err(|_| RegexErrorTag::Runtime)?;
            let m = caps.get(0).ok_or(RegexErrorTag::Runtime)?;
            out.extend_from_slice(&bytes[last..m.start()]);
            let piece = expand_replacement(&template, &caps)?;
            out.extend_from_slice(piece.as_bytes());
            last = m.end();
            replaced_once = true;
            if !all {
                break;
            }
        }
        if !replaced_once && !all {
            // replace with no match returns the original subject
            return Ok(subject.clone());
        }
        out.extend_from_slice(&bytes[last..]);
        bytes_to_string(&out)
    })?;
    Ok(intern_string(heap, &replaced))
}

pub const REGEX_COMPILE: &str = "regex_compile";
pub const REGEX_IS_MATCH: &str = "regex_is_match";
pub const REGEX_FIND: &str = "regex_find";
pub const REGEX_FIND_ALL: &str = "regex_find_all";
pub const REGEX_CAPTURES: &str = "regex_captures";
pub const REGEX_CAPTURES_ALL: &str = "regex_captures_all";
pub const REGEX_SPLIT: &str = "regex_split";
pub const REGEX_REPLACE: &str = "regex_replace";
pub const REGEX_REPLACE_ALL: &str = "regex_replace_all";

/// Registry name, arity, and host fn for pipeline wiring.
pub const REGEX_WIRING: &[(&str, usize, fn(&mut Heap, &[Value]) -> Value)] = &[
    (REGEX_COMPILE, 2, host_regex_compile),
    (REGEX_IS_MATCH, 2, host_regex_is_match),
    (REGEX_FIND, 2, host_regex_find),
    (REGEX_FIND_ALL, 2, host_regex_find_all),
    (REGEX_CAPTURES, 2, host_regex_captures),
    (REGEX_CAPTURES_ALL, 2, host_regex_captures_all),
    (REGEX_SPLIT, 2, host_regex_split),
    (REGEX_REPLACE, 3, host_regex_replace),
    (REGEX_REPLACE_ALL, 3, host_regex_replace_all),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::Heap;

    fn ok_payload(heap: &Heap, result: Value) -> Value {
        match heap.find_object_by_addr(result.raw() as u64) {
            Some(Object::Enum(gc)) => {
                assert_eq!(gc.as_ref().tag, 0, "expected Ok");
                match &gc.as_ref().payload[0] {
                    Member::Value(v) => *v,
                    Member::Object(o) => Value::from(o.addr()),
                }
            }
            _ => panic!("expected Result enum"),
        }
    }

    fn err_tag(heap: &Heap, result: Value) -> u32 {
        match heap.find_object_by_addr(result.raw() as u64) {
            Some(Object::Enum(gc)) => {
                assert_eq!(gc.as_ref().tag, 1, "expected Err");
                match &gc.as_ref().payload[0] {
                    Member::Object(Object::Enum(inner)) => inner.as_ref().tag,
                    Member::Value(v) => match heap.find_object_by_addr(v.raw() as u64) {
                        Some(Object::Enum(inner)) => inner.as_ref().tag,
                        _ => panic!("expected RegexError"),
                    },
                    _ => panic!("expected RegexError object"),
                }
            }
            _ => panic!("expected Result enum"),
        }
    }

    fn str_val(heap: &mut Heap, s: &str) -> Value {
        intern_string(heap, s)
    }

    #[test]
    fn compile_bad_flags_is_compile_error() {
        let mut heap = Heap::default();
        let pat = str_val(&mut heap, "a");
        let flags = str_val(&mut heap, "Z");
        let r = host_regex_compile(&mut heap, &[pat, flags]);
        assert_eq!(err_tag(&heap, r), RegexErrorTag::Compile as u32);
    }

    #[test]
    fn compile_bad_pattern_is_compile_error() {
        let mut heap = Heap::default();
        let pat = str_val(&mut heap, "(");
        let flags = str_val(&mut heap, "");
        let r = host_regex_compile(&mut heap, &[pat, flags]);
        assert_eq!(err_tag(&heap, r), RegexErrorTag::Compile as u32);
    }

    #[test]
    fn is_match_respects_caseless_flag() {
        let mut heap = Heap::default();
        let pat = str_val(&mut heap, "abc");
        let flags = str_val(&mut heap, "i");
        let compiled = host_regex_compile(&mut heap, &[pat, flags]);
        let re = ok_payload(&heap, compiled);
        let subj = str_val(&mut heap, "ABC");
        let r = host_regex_is_match(&mut heap, &[re, subj]);
        let v = ok_payload(&heap, r);
        assert!(v.as_int() != 0);
    }

    #[test]
    fn find_all_and_split_and_replace_all() {
        let mut heap = Heap::default();
        let pat = str_val(&mut heap, r"(\w+)=(\d+)");
        let flags = str_val(&mut heap, "");
        let compiled = host_regex_compile(&mut heap, &[pat, flags]);
        let re = ok_payload(&heap, compiled);

        let subj = str_val(&mut heap, "a=1 b=2");
        let spans_r = host_regex_find_all(&mut heap, &[re, subj]);
        let spans = ok_payload(&heap, spans_r);
        match heap.find_object_by_addr(spans.raw() as u64) {
            Some(Object::Array(gc)) => assert_eq!(gc.as_ref().elements.len(), 2),
            _ => panic!("expected array"),
        }

        let sep_pat = str_val(&mut heap, ",");
        let sep_flags = str_val(&mut heap, "");
        let sep_compiled = host_regex_compile(&mut heap, &[sep_pat, sep_flags]);
        let sep = ok_payload(&heap, sep_compiled);
        let csv = str_val(&mut heap, "a,b,c");
        let parts_r = host_regex_split(&mut heap, &[sep, csv]);
        let parts = ok_payload(&heap, parts_r);
        match heap.find_object_by_addr(parts.raw() as u64) {
            Some(Object::Array(gc)) => assert_eq!(gc.as_ref().elements.len(), 3),
            _ => panic!("expected array"),
        }

        let subj2 = str_val(&mut heap, "a=1 b=2");
        let tmpl = str_val(&mut heap, "$1->$2");
        let out_r = host_regex_replace_all(&mut heap, &[re, subj2, tmpl]);
        let out = ok_payload(&heap, out_r);
        match heap.find_object_by_addr(out.raw() as u64) {
            Some(Object::String(gc)) => assert_eq!(gc.as_ref().data, "a->1 b->2"),
            _ => panic!("expected string"),
        }
    }

    #[test]
    fn find_no_match_is_nomatch() {
        let mut heap = Heap::default();
        let pat = str_val(&mut heap, "xyz");
        let flags = str_val(&mut heap, "");
        let compiled = host_regex_compile(&mut heap, &[pat, flags]);
        let re = ok_payload(&heap, compiled);
        let subj = str_val(&mut heap, "abc");
        let r = host_regex_find(&mut heap, &[re, subj]);
        assert_eq!(err_tag(&heap, r), RegexErrorTag::NoMatch as u32);
    }
}
