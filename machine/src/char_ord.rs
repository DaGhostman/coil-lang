//! Prelude `ord` / `char` host helpers.

use common::Value;

use crate::io::{alloc_result_err, alloc_result_ok, value_as_string};
use crate::memory::Heap;

fn string_val(heap: &mut Heap, text: &str) -> Value {
    let gc = heap.intern(text.to_string());
    Value::from(gc.as_ptr() as *mut u8 as u64)
}

fn err_msg(heap: &mut Heap, text: &str) -> Value {
    let msg = string_val(heap, text);
    alloc_result_err(heap, msg)
}

/// `ord(string) -> Result<byte, string>` — UTF-8 code unit must fit in `byte`.
pub fn prelude_ord(heap: &mut Heap, args: &[Value]) -> Value {
    let s = match value_as_string(heap, args[0]) {
        Ok(s) => s,
        Err(_) => return err_msg(heap, "expected string"),
    };
    let mut chars = s.chars();
    let first = match chars.next() {
        Some(c) => c,
        None => return err_msg(heap, "empty string"),
    };
    if chars.next().is_some() {
        return err_msg(heap, "expected single character");
    }
    let code = first as u32;
    if code > 255 {
        return err_msg(heap, "character code out of byte range");
    }
    alloc_result_ok(heap, Value::from(code as i64))
}

/// `char(byte) -> string` — one code unit in 0..=255.
pub fn prelude_char(heap: &mut Heap, args: &[Value]) -> Value {
    let b = args[0].as_int();
    if !(0..=255).contains(&b) {
        return string_val(heap, "");
    }
    let ch = char::from_u32(b as u32).unwrap_or('\0');
    let mut buf = [0u8; 4];
    let encoded = ch.encode_utf8(&mut buf);
    string_val(heap, encoded)
}
