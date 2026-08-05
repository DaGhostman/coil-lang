// Minimal JSON encode/decode (userland).
// Values: null, bool, int, float, string, array, object (parallel key/value arrays).
// String escapes: \" \\ \/ \n \r \t  (no \uXXXX in v1).
// Variant names are globally unique (coil ctor namespace is flat).
use string::{to_bytes, from_bytes};
use bytes::{slice as bytes_slice, concat as bytes_concat};

enum JsonError {
    JsonUnexpectedEnd,
    JsonUnexpectedChar,
    JsonBadNumber,
    JsonBadString,
    JsonBadEscape,
    JsonTrailingJunk,
}

enum Json {
    JsonNull,
    JsonBool(bool),
    JsonInt(int),
    JsonFloat(float),
    JsonStr(string),
    JsonArray([Json]),
    JsonObject([string], [Json]),
}

fn is_ws(byte c) -> bool {
    let sp: byte = 32;
    let tab: byte = 9;
    let lf: byte = 10;
    let cr: byte = 13;
    if c == sp {
        return true;
    }
    if c == tab {
        return true;
    }
    if c == lf {
        return true;
    }
    if c == cr {
        return true;
    }
    return false;
}

fn skip_ws([byte] b, int i) -> int {
    while i < len(b) {
        if is_ws(b[i]) {
            i = i + 1;
        }
        if i < len(b) {
            if is_ws(b[i]) == false {
                break;
            }
        }
        if i >= len(b) {
            break;
        }
    }
    return i;
}

fn digit_val(byte c) -> int {
    let zero: byte = 48;
    let nine: byte = 57;
    if c < zero {
        return 0 - 1;
    }
    if c > nine {
        return 0 - 1;
    }
    return (c as int) - 48;
}

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

fn int_to_dec(int n) -> string {
    if n == 0 {
        return "0";
    }
    let neg = 0;
    let x = n;
    if x < 0 {
        neg = 1;
        x = 0 - x;
    }
    let rev = "";
    while x > 0 {
        let d = x % 10;
        rev = digit_char(d) + rev;
        x = x / 10;
    }
    if neg == 1 {
        return "-" + rev;
    }
    return rev;
}

fn int_to_dec_pad6(int n) -> string {
    let s = int_to_dec(n);
    while len(s) < 6 {
        s = "0" + s;
    }
    return s;
}

fn float_to_dec(float f) -> string {
    let neg = 0;
    let x = f;
    if x < 0.0 {
        neg = 1;
        x = 0.0 - x;
    }
    let ip = x as int;
    let frac = x - (ip as float);
    let scaled = (frac * 1000000.0 + 0.5) as int;
    let body = int_to_dec(ip) + "." + int_to_dec_pad6(scaled);
    if neg == 1 {
        return "-" + body;
    }
    return body;
}

fn parse_literal([byte] b, int i, string lit, Json val) -> Result<(Json, int), JsonError> {
    let lb = to_bytes(lit);
    let n = len(lb);
    if i + n > len(b) {
        raise JsonError::JsonUnexpectedEnd;
    }
    let j = 0;
    while j < n {
        if b[i + j] != lb[j] {
            raise JsonError::JsonUnexpectedChar;
        }
        j = j + 1;
    }
    return (val, i + n);
}

fn append_escape([byte] out, byte e) -> Result<[byte], JsonError> {
    let q: byte = 34;
    let bs: byte = 92;
    let sl: byte = 47;
    let nch: byte = 110;
    let rch: byte = 114;
    let tch: byte = 116;
    if e == q {
        out[] = 34;
        return out;
    }
    if e == bs {
        out[] = 92;
        return out;
    }
    if e == sl {
        out[] = 47;
        return out;
    }
    if e == nch {
        out[] = 10;
        return out;
    }
    if e == rch {
        out[] = 13;
        return out;
    }
    if e == tch {
        out[] = 9;
        return out;
    }
    raise JsonError::JsonBadEscape;
}

fn parse_string_raw([byte] b, int i) -> Result<(string, int), JsonError> {
    let q: byte = 34;
    let bs: byte = 92;
    if i >= len(b) {
        raise JsonError::JsonUnexpectedEnd;
    }
    if b[i] != q {
        raise JsonError::JsonBadString;
    }
    i = i + 1;
    let out: [byte] = [];
    while i < len(b) {
        let c = b[i];
        if c == q {
            let s = match from_bytes(out) {
                Result::Ok(v) => v,
                Result::Err(_) => raise JsonError::JsonBadString,
            };
            return (s, i + 1);
        }
        if c == bs {
            i = i + 1;
            if i >= len(b) {
                raise JsonError::JsonUnexpectedEnd;
            }
            out = append_escape(out, b[i])?;
            i = i + 1;
        }
        if c != bs {
            if c != q {
                let ctrl: byte = 32;
                if c < ctrl {
                    raise JsonError::JsonBadString;
                }
                out[] = c;
                i = i + 1;
            }
        }
    }
    raise JsonError::JsonUnexpectedEnd;
}

fn parse_int_bytes([byte] b) -> Result<int, JsonError> {
    let minus: byte = 45;
    let i = 0;
    let neg = 0;
    if len(b) == 0 {
        raise JsonError::JsonBadNumber;
    }
    if b[0] == minus {
        neg = 1;
        i = 1;
    }
    if i >= len(b) {
        raise JsonError::JsonBadNumber;
    }
    let n = 0;
    while i < len(b) {
        let d = digit_val(b[i]);
        if d == (0 - 1) {
            raise JsonError::JsonBadNumber;
        }
        n = n * 10 + d;
        i = i + 1;
    }
    if neg == 1 {
        return 0 - n;
    }
    return n;
}

fn parse_float_bytes([byte] b) -> Result<float, JsonError> {
    let minus: byte = 45;
    let dot: byte = 46;
    let e_lo: byte = 101;
    let e_up: byte = 69;
    let plus: byte = 43;
    let i = 0;
    let sign = 1.0;
    if len(b) == 0 {
        raise JsonError::JsonBadNumber;
    }
    if b[0] == minus {
        sign = 0.0 - 1.0;
        i = 1;
    }
    let int_part = 0.0;
    let saw = 0;
    while i < len(b) {
        let d = digit_val(b[i]);
        if d == (0 - 1) {
            break;
        }
        int_part = int_part * 10.0 + (d as float);
        saw = 1;
        i = i + 1;
    }
    let frac = 0.0;
    let place = 0.1;
    if i < len(b) {
        if b[i] == dot {
            i = i + 1;
            while i < len(b) {
                let d = digit_val(b[i]);
                if d == (0 - 1) {
                    break;
                }
                frac = frac + (d as float) * place;
                place = place / 10.0;
                saw = 1;
                i = i + 1;
            }
        }
    }
    if saw == 0 {
        raise JsonError::JsonBadNumber;
    }
    let val = sign * (int_part + frac);
    let exp_sign = 1;
    let exp_v = 0;
    let has_exp = 0;
    if i < len(b) {
        if b[i] == e_lo {
            has_exp = 1;
        }
        if b[i] == e_up {
            has_exp = 1;
        }
    }
    if has_exp == 1 {
        i = i + 1;
        if i < len(b) {
            if b[i] == plus {
                i = i + 1;
            }
            if i < len(b) {
                if b[i] == minus {
                    exp_sign = 0 - 1;
                    i = i + 1;
                }
            }
        }
        if i >= len(b) {
            raise JsonError::JsonBadNumber;
        }
        while i < len(b) {
            let d = digit_val(b[i]);
            if d == (0 - 1) {
                break;
            }
            exp_v = exp_v * 10 + d;
            i = i + 1;
        }
    }
    let e = 0;
    while e < exp_v {
        if exp_sign > 0 {
            val = val * 10.0;
        }
        if exp_sign < 0 {
            val = val / 10.0;
        }
        e = e + 1;
    }
    return val;
}

fn parse_number([byte] b, int i) -> Result<(Json, int), JsonError> {
    let minus: byte = 45;
    let dot: byte = 46;
    let e_lo: byte = 101;
    let e_up: byte = 69;
    let plus: byte = 43;
    let start = i;
    if i < len(b) {
        if b[i] == minus {
            i = i + 1;
        }
    }
    if i >= len(b) {
        raise JsonError::JsonBadNumber;
    }
    if digit_val(b[i]) == (0 - 1) {
        raise JsonError::JsonBadNumber;
    }
    while i < len(b) {
        if digit_val(b[i]) == (0 - 1) {
            break;
        }
        i = i + 1;
    }
    let is_float = 0;
    if i < len(b) {
        if b[i] == dot {
            is_float = 1;
            i = i + 1;
            if i >= len(b) {
                raise JsonError::JsonBadNumber;
            }
            if digit_val(b[i]) == (0 - 1) {
                raise JsonError::JsonBadNumber;
            }
            while i < len(b) {
                if digit_val(b[i]) == (0 - 1) {
                    break;
                }
                i = i + 1;
            }
        }
    }
    if i < len(b) {
        let ec = b[i];
        if ec == e_lo {
            is_float = 1;
            i = i + 1;
            if i < len(b) {
                if b[i] == plus {
                    i = i + 1;
                }
                if i < len(b) {
                    if b[i] == minus {
                        i = i + 1;
                    }
                }
            }
            if i >= len(b) {
                raise JsonError::JsonBadNumber;
            }
            while i < len(b) {
                if digit_val(b[i]) == (0 - 1) {
                    break;
                }
                i = i + 1;
            }
        }
        if ec == e_up {
            is_float = 1;
            i = i + 1;
            if i < len(b) {
                if b[i] == plus {
                    i = i + 1;
                }
                if i < len(b) {
                    if b[i] == minus {
                        i = i + 1;
                    }
                }
            }
            if i >= len(b) {
                raise JsonError::JsonBadNumber;
            }
            while i < len(b) {
                if digit_val(b[i]) == (0 - 1) {
                    break;
                }
                i = i + 1;
            }
        }
    }
    let slice = bytes_slice(b, start, i);
    if is_float == 1 {
        let f = parse_float_bytes(slice)?;
        return (Json::JsonFloat(f), i);
    }
    let n = parse_int_bytes(slice)?;
    return (Json::JsonInt(n), i);
}

/// Recursive descent entry — arrays/objects call back into this function.
#[max_depth(256)]
fn parse_value([byte] b, int i) -> Result<(Json, int), JsonError> {
    let q: byte = 34;
    let lbr: byte = 91;
    let rbr: byte = 93;
    let lbrace: byte = 123;
    let rbrace: byte = 125;
    let comma: byte = 44;
    let colon: byte = 58;
    let minus: byte = 45;
    let nch: byte = 110;
    let tch: byte = 116;
    let fch: byte = 102;
    let zero: byte = 48;
    let nine: byte = 57;
    i = skip_ws(b, i);
    if i >= len(b) {
        raise JsonError::JsonUnexpectedEnd;
    }
    let c = b[i];
    if c == nch {
        return parse_literal(b, i, "null", Json::JsonNull)?;
    }
    if c == tch {
        return parse_literal(b, i, "true", Json::JsonBool(true))?;
    }
    if c == fch {
        return parse_literal(b, i, "false", Json::JsonBool(false))?;
    }
    if c == q {
        let sp = parse_string_raw(b, i)?;
        let (ss, after) = sp;
        return (Json::JsonStr(ss), after);
    }
    if c == minus {
        return parse_number(b, i)?;
    }
    let ci = c as int;
    if ci >= 48 {
        if ci <= 57 {
            // Inline non-negative int parse (array recursion + parse_number is flaky).
            let start = i;
            while i < len(b) {
                if digit_val(b[i]) == (0 - 1) {
                    break;
                }
                i = i + 1;
            }
            let n = parse_int_bytes(bytes_slice(b, start, i))?;
            return (Json::JsonInt(n), i);
        }
    }
    if c == lbr {
        i = skip_ws(b, i + 1);
        let items: [Json] = [];
        if i < len(b) {
            if b[i] == rbr {
                return (Json::JsonArray(items), i + 1);
            }
        }
        let done = 0;
        while done == 0 {
            let pair = parse_value(b, i)?;
            let (jv, ni) = pair;
            items[] = jv;
            i = skip_ws(b, ni);
            if i >= len(b) {
                raise JsonError::JsonUnexpectedEnd;
            }
            let sep = b[i];
            if sep == comma {
                i = skip_ws(b, i + 1);
            }
            if sep == rbr {
                done = 1;
                i = i + 1;
            }
            if done == 0 {
                if sep != comma {
                    raise JsonError::JsonUnexpectedChar;
                }
            }
        }
        return (Json::JsonArray(items), i);
    }
    if c == lbrace {
        i = skip_ws(b, i + 1);
        let keys: [string] = [];
        let vals: [Json] = [];
        if i < len(b) {
            if b[i] == rbrace {
                return (Json::JsonObject(keys, vals), i + 1);
            }
        }
        let done = 0;
        while done == 0 {
            let sk = parse_string_raw(b, i)?;
            let (key, after_key) = sk;
            i = skip_ws(b, after_key);
            if i >= len(b) {
                raise JsonError::JsonUnexpectedEnd;
            }
            if b[i] != colon {
                raise JsonError::JsonUnexpectedChar;
            }
            let vp = parse_value(b, i + 1)?;
            let (jv, after_val) = vp;
            keys[] = key;
            vals[] = jv;
            i = skip_ws(b, after_val);
            if i >= len(b) {
                raise JsonError::JsonUnexpectedEnd;
            }
            let sep = b[i];
            if sep == comma {
                i = skip_ws(b, i + 1);
            }
            if sep == rbrace {
                done = 1;
                i = i + 1;
            }
            if done == 0 {
                if sep != comma {
                    raise JsonError::JsonUnexpectedChar;
                }
            }
        }
        return (Json::JsonObject(keys, vals), i);
    }
    raise JsonError::JsonUnexpectedChar;
}

/// Parse a full JSON document (trailing whitespace allowed).
fn parse(string s) -> Result<Json, JsonError> {
    let b = to_bytes(s);
    let pair = parse_value(b, 0)?;
    let (v, after) = pair;
    let i = skip_ws(b, after);
    if i != len(b) {
        raise JsonError::JsonTrailingJunk;
    }
    return v;
}

fn escape_string(string s) -> [byte] {
    let b = to_bytes(s);
    let out: [byte] = [];
    out[] = 34;
    let i = 0;
    let q: byte = 34;
    let bs: byte = 92;
    let lf: byte = 10;
    let cr: byte = 13;
    let tab: byte = 9;
    while i < len(b) {
        let c = b[i];
        let special = 0;
        if c == q {
            out[] = 92;
            out[] = 34;
            special = 1;
        }
        if c == bs {
            out[] = 92;
            out[] = 92;
            special = 1;
        }
        if c == lf {
            out[] = 92;
            out[] = 110;
            special = 1;
        }
        if c == cr {
            out[] = 92;
            out[] = 114;
            special = 1;
        }
        if c == tab {
            out[] = 92;
            out[] = 116;
            special = 1;
        }
        if special == 0 {
            out[] = c;
        }
        i = i + 1;
    }
    out[] = 34;
    return out;
}

#[max_depth(256)]
fn stringify_bytes(Json v) -> [byte] {
    let out: [byte] = [];
    let kind = 0;
    let bool_v = false;
    let int_v = 0;
    let float_v = 0.0;
    let str_v = "";
    let arr: [Json] = [];
    let keys: [string] = [];
    let vals: [Json] = [];
    match v {
        Json::JsonNull => {
            kind = 0;
            0
        },
        Json::JsonBool(bv) => {
            kind = 1;
            bool_v = bv;
            0
        },
        Json::JsonInt(n) => {
            kind = 2;
            int_v = n;
            0
        },
        Json::JsonFloat(f) => {
            kind = 3;
            float_v = f;
            0
        },
        Json::JsonStr(s) => {
            kind = 4;
            str_v = s;
            0
        },
        Json::JsonArray(a) => {
            kind = 5;
            arr = a;
            0
        },
        Json::JsonObject(k, vs) => {
            kind = 6;
            keys = k;
            vals = vs;
            0
        },
    };
    if kind == 0 {
        return to_bytes("null");
    }
    if kind == 1 {
        if bool_v {
            return to_bytes("true");
        }
        return to_bytes("false");
    }
    if kind == 2 {
        return to_bytes(int_to_dec(int_v));
    }
    if kind == 3 {
        return to_bytes(float_to_dec(float_v));
    }
    if kind == 4 {
        return escape_string(str_v);
    }
    if kind == 5 {
        out[] = 91;
        let i = 0;
        while i < len(arr) {
            if i > 0 {
                out[] = 44;
            }
            out = bytes_concat(out, stringify_bytes(arr[i]));
            i = i + 1;
        }
        out[] = 93;
        return out;
    }
    out[] = 123;
    let i = 0;
    while i < len(keys) {
        if i > 0 {
            out[] = 44;
        }
        out = bytes_concat(out, escape_string(keys[i]));
        out[] = 58;
        out = bytes_concat(out, stringify_bytes(vals[i]));
        i = i + 1;
    }
    out[] = 125;
    return out;
}

/// Serialize `v` to a JSON string.
fn stringify(Json v) -> Result<string, JsonError> {
    return match from_bytes(stringify_bytes(v)) {
        Result::Ok(s) => s,
        Result::Err(_) => raise JsonError::JsonBadString,
    };
}

fn json_null() -> Json {
    return Json::JsonNull;
}

fn json_bool(bool b) -> Json {
    return Json::JsonBool(b);
}

fn json_int(int n) -> Json {
    return Json::JsonInt(n);
}

fn json_float(float f) -> Json {
    return Json::JsonFloat(f);
}

fn json_str(string s) -> Json {
    return Json::JsonStr(s);
}

fn json_array([Json] items) -> Json {
    return Json::JsonArray(items);
}

fn json_object([string] keys, [Json] vals) -> Json {
    return Json::JsonObject(keys, vals);
}
