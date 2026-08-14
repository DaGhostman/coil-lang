//! Reserved HostInvoke slots for the removed virtual `regex` module.
//!
//! Stale bytecode that still calls these ids must panic instead of shifting
//! later native registrations (thread, packed LA, …).

use common::Value;

use crate::memory::Heap;

fn host_regex_removed(_heap: &mut Heap, _args: &[Value]) -> Value {
    panic!("regex HostInvoke removed; add coil-regex to [module].roots (see docs/references/regex.md)");
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

/// Same names and arities as the former [`super::regex::REGEX_WIRING`].
pub const REGEX_STUB_WIRING: &[(&str, usize, fn(&mut Heap, &[Value]) -> Value)] = &[
    (REGEX_COMPILE, 2, host_regex_removed),
    (REGEX_IS_MATCH, 2, host_regex_removed),
    (REGEX_FIND, 2, host_regex_removed),
    (REGEX_FIND_ALL, 2, host_regex_removed),
    (REGEX_CAPTURES, 2, host_regex_removed),
    (REGEX_CAPTURES_ALL, 2, host_regex_removed),
    (REGEX_SPLIT, 2, host_regex_removed),
    (REGEX_REPLACE, 3, host_regex_removed),
    (REGEX_REPLACE_ALL, 3, host_regex_removed),
];
