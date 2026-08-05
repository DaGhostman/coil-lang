// Pure userland helpers for ASCII byte classification and decimal digits.

/// True for ASCII space, tab, line feed, or carriage return.
fn is_space(byte c) -> bool {
    return c == " " || c == "\t" || c == "\n" || c == "\r";
}

/// True for an ASCII decimal digit (`0` through `9`).
fn is_digit(byte c) -> bool {
    return c >= "0" && c <= "9";
}

/// True for an ASCII letter (`A` through `Z` or `a` through `z`).
fn is_alpha(byte c) -> bool {
    if c >= "A" && c <= "Z" {
        return true;
    }
    return c >= "a" && c <= "z";
}

/// Numeric value of an ASCII decimal digit, or `-1` for another byte.
fn digit_val(byte c) -> int {
    if !is_digit(c) {
        return -1;
    }
    return (c as int) - (("0" as byte) as int);
}

/// Decimal digit as a one-byte string. Callers must pass `0..=9`.
fn digit_char(int d) -> string {
    if d == 0 { return "0"; }
    if d == 1 { return "1"; }
    if d == 2 { return "2"; }
    if d == 3 { return "3"; }
    if d == 4 { return "4"; }
    if d == 5 { return "5"; }
    if d == 6 { return "6"; }
    if d == 7 { return "7"; }
    if d == 8 { return "8"; }
    return "9";
}
