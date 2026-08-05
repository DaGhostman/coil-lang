// Expected: compile failure — userland wildcard import (E0124).
use missing_mod::*;

fn main() {}
