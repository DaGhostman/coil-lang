// Minimal JSON encode/decode (userland).
// Values: null, bool, int, float, string, array, object (parallel key/value arrays).
//
// Supported string escapes: \" \\ \/ \b \f \n \r \t \uXXXX (BMP; no surrogate pairs).
// Float stringify uses fixed 6 fractional digits (trailing zeros trimmed).
// Objects are parallel key/value arrays — use `object_get` for lookup.
// Nesting capped by `#[max_depth(256)]` on parse/stringify.
use string::{to_bytes, from_bytes};
use bytes::{slice as bytes_slice};
use ascii::{is_space, digit_val, hex_val, hex_digit};
use conv::{int_to_dec};

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
    JsonArray(Vec<Json>),
    JsonObject(Vec<string>, Vec<Json>),
}

fn skip_ws(Vec<byte> b, int i) -> int {
    while i < len(b) {
        if !is_space(b[i]) {
            break;
        }
        i = i + 1;
    }
    return i;
}

/// Append every byte of `extra` onto `out` (in-place growth; avoids concat copies).
fn append_bytes(Vec<byte> out, Vec<byte> extra) -> Vec<byte> {
    let i = 0;
    while i < len(extra) {
        out.push(extra[i]);
        i = i + 1;
    }
    return out;
}

fn int_to_dec_pad6(int n) -> string {
    let s = int_to_dec(n);
    while len(s) < 6 {
        s = "0" + s;
    }
    return s;
}

/// Fixed-point float → decimal text (6 frac digits, trailing zeros trimmed).
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
    let frac_s = int_to_dec_pad6(scaled);
    let fb = to_bytes(frac_s);
    let end = len(fb);
    while end > 0 {
        if fb[end - 1] != "0" {
            break;
        }
        end = end - 1;
    }
    let body = int_to_dec(ip);
    if end > 0 {
        let trimmed = match from_bytes(bytes_slice(fb, 0, end)) {
            Result::Ok(t) => t,
            Result::Err(_) => frac_s,
        };
        body = body + "." + trimmed;
    }
    if neg == 1 {
        return "-" + body;
    }
    return body;
}

fn parse_literal(Vec<byte> b, int i, string lit, Json val) -> Result<(Json, int), JsonError> {
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

/// Encode a Unicode scalar (0..=0x10FFFF, non-surrogate) as UTF-8 into `out`.
fn append_utf8(Vec<byte> out, int cp) -> Result<Vec<byte>, JsonError> {
    if cp < 0 {
        raise JsonError::JsonBadEscape;
    }
    if cp <= 127 {
        let b: byte = cp as byte;
        out.push(b);
        return out;
    }
    if cp <= 2047 {
        let t0: int = 192 + (cp / 64);
        let t1: int = 128 + (cp % 64);
        out.push(t0 as byte);
        out.push(t1 as byte);
        return out;
    }
    if cp >= 55296 {
        if cp <= 57343 {
            raise JsonError::JsonBadEscape;
        }
    }
    if cp <= 65535 {
        let t0: int = 224 + (cp / 4096);
        let t1: int = 128 + ((cp / 64) % 64);
        let t2: int = 128 + (cp % 64);
        out.push(t0 as byte);
        out.push(t1 as byte);
        out.push(t2 as byte);
        return out;
    }
    if cp > 1114111 {
        raise JsonError::JsonBadEscape;
    }
    let t0: int = 240 + (cp / 262144);
    let t1: int = 128 + ((cp / 4096) % 64);
    let t2: int = 128 + ((cp / 64) % 64);
    let t3: int = 128 + (cp % 64);
    out.push(t0 as byte);
    out.push(t1 as byte);
    out.push(t2 as byte);
    out.push(t3 as byte);
    return out;
}

fn parse_hex4(Vec<byte> b, int i) -> Result<(int, int), JsonError> {
    if i + 4 > len(b) {
        raise JsonError::JsonUnexpectedEnd;
    }
    let v = 0;
    let j = 0;
    while j < 4 {
        let h = hex_val(b[i + j]);
        if h < 0 {
            raise JsonError::JsonBadEscape;
        }
        v = v * 16 + h;
        j = j + 1;
    }
    return (v, i + 4);
}

fn append_escape(Vec<byte> out, Vec<byte> b, int i) -> Result<(Vec<byte>, int), JsonError> {
    let q: byte = "\"";
    let bs: byte = "\\";
    let sl: byte = "/";
    let bch: byte = "b";
    let fch: byte = "f";
    let nch: byte = "n";
    let rch: byte = "r";
    let tch: byte = "t";
    let uch: byte = "u";
    if i >= len(b) {
        raise JsonError::JsonUnexpectedEnd;
    }
    let e = b[i];
    if e == q {
        out.push(34);
        return (out, i + 1);
    }
    if e == bs {
        out.push(92);
        return (out, i + 1);
    }
    if e == sl {
        out.push(47);
        return (out, i + 1);
    }
    if e == bch {
        out.push(8);
        return (out, i + 1);
    }
    if e == fch {
        out.push(12);
        return (out, i + 1);
    }
    if e == nch {
        out.push("\n");
        return (out, i + 1);
    }
    if e == rch {
        out.push("\r");
        return (out, i + 1);
    }
    if e == tch {
        out.push("\t");
        return (out, i + 1);
    }
    if e == uch {
        let hp = parse_hex4(b, i + 1)?;
        let (cp, after) = hp;
        out = append_utf8(out, cp)?;
        return (out, after);
    }
    raise JsonError::JsonBadEscape;
}

fn parse_string_raw(Vec<byte> b, int i) -> Result<(string, int), JsonError> {
    let q: byte = "\"";
    let bs: byte = "\\";
    if i >= len(b) {
        raise JsonError::JsonUnexpectedEnd;
    }
    if b[i] != q {
        raise JsonError::JsonBadString;
    }
    i = i + 1;
    let out: Vec<byte> = Vec::new();
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
            let ep = append_escape(out, b, i + 1)?;
            let (nout, ni) = ep;
            out = nout;
            i = ni;
        } else {
            let ctrl: byte = " ";
            if c < ctrl {
                raise JsonError::JsonBadString;
            }
            out.push(c);
            i = i + 1;
        }
    }
    raise JsonError::JsonUnexpectedEnd;
}

fn parse_int_bytes(Vec<byte> b) -> Result<int, JsonError> {
    let minus: byte = "-";
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
        if d < 0 {
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

fn parse_float_bytes(Vec<byte> b) -> Result<float, JsonError> {
    let minus: byte = "-";
    let dot: byte = ".";
    let e_lo: byte = "e";
    let e_up: byte = "E";
    let plus: byte = "+";
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
        if d < 0 {
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
                if d < 0 {
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
                    exp_sign = -1;
                    i = i + 1;
                }
            }
        }
        if i >= len(b) {
            raise JsonError::JsonBadNumber;
        }
        while i < len(b) {
            let d = digit_val(b[i]);
            if d < 0 {
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

fn parse_number(Vec<byte> b, int i) -> Result<(Json, int), JsonError> {
    let minus: byte = "-";
    let dot: byte = ".";
    let e_lo: byte = "e";
    let e_up: byte = "E";
    let plus: byte = "+";
    let start = i;
    if i < len(b) {
        if b[i] == minus {
            i = i + 1;
        }
    }
    if i >= len(b) {
        raise JsonError::JsonBadNumber;
    }
    if digit_val(b[i]) < 0 {
        raise JsonError::JsonBadNumber;
    }
    while i < len(b) {
        if digit_val(b[i]) < 0 {
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
            if digit_val(b[i]) < 0 {
                raise JsonError::JsonBadNumber;
            }
            while i < len(b) {
                if digit_val(b[i]) < 0 {
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
                if digit_val(b[i]) < 0 {
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
                if digit_val(b[i]) < 0 {
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
fn parse_value(Vec<byte> b, int i) -> Result<(Json, int), JsonError> {
    let q: byte = "\"";
    let lbr: byte = "[";
    let rbr: byte = "]";
    let lbrace: byte = "{";
    let rbrace: byte = "}";
    let comma: byte = ",";
    let colon: byte = ":";
    let minus: byte = "-";
    let nch: byte = "n";
    let tch: byte = "t";
    let fch: byte = "f";
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
    if c >= "0" {
        if c <= "9" {
            return parse_number(b, i)?;
        }
    }
    if c == lbr {
        i = skip_ws(b, i + 1);
        let items: Vec<Json> = Vec::new();
        if i < len(b) {
            if b[i] == rbr {
                return (Json::JsonArray(items), i + 1);
            }
        }
        let done = 0;
        while done == 0 {
            let pair = parse_value(b, i)?;
            let (jv, ni) = pair;
            items.push(jv);
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
        let keys: Vec<string> = Vec::new();
        let vals: Vec<Json> = Vec::new();
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
            keys.push(key);
            vals.push(jv);
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

fn append_u_escape(Vec<byte> out, int cp) -> Vec<byte> {
    out.push(92);
    out.push(117);
    out.push(hex_digit((cp / 4096) % 16));
    out.push(hex_digit((cp / 256) % 16));
    out.push(hex_digit((cp / 16) % 16));
    out.push(hex_digit(cp % 16));
    return out;
}

fn escape_string(string s) -> Vec<byte> {
    let b = to_bytes(s);
    let out: Vec<byte> = Vec::new();
    out.push(34);
    let i = 0;
    let q: byte = "\"";
    let bs: byte = "\\";
    let lf: byte = "\n";
    let cr: byte = "\r";
    let tab: byte = "\t";
    while i < len(b) {
        let c = b[i];
        let special = 0;
        if c == q {
            out.push(92);
            out.push(34);
            special = 1;
        }
        if c == bs {
            out.push(92);
            out.push(92);
            special = 1;
        }
        if c == lf {
            out.push(92);
            out.push(110);
            special = 1;
        }
        if c == cr {
            out.push(92);
            out.push(114);
            special = 1;
        }
        if c == tab {
            out.push(92);
            out.push(116);
            special = 1;
        }
        if special == 0 {
            let ci = c as int;
            if ci < 32 {
                out = append_u_escape(out, ci);
            } else {
                out.push(c);
            }
        }
        i = i + 1;
    }
    out.push(34);
    return out;
}

#[max_depth(256)]
fn stringify_bytes(Json v) -> Vec<byte> {
    let out: Vec<byte> = Vec::new();
    let kind = 0;
    let bool_v = false;
    let int_v = 0;
    let float_v = 0.0;
    let str_v = "";
    let arr: Vec<Json> = Vec::new();
    let keys: Vec<string> = Vec::new();
    let vals: Vec<Json> = Vec::new();
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
        out.push(91);
        let i = 0;
        while i < len(arr) {
            if i > 0 {
                out.push(44);
            }
            out = append_bytes(out, stringify_bytes(arr[i]));
            i = i + 1;
        }
        out.push(93);
        return out;
    }
    out.push(123);
    let i = 0;
    while i < len(keys) {
        if i > 0 {
            out.push(44);
        }
        out = append_bytes(out, escape_string(keys[i]));
        out.push(":");
        out = append_bytes(out, stringify_bytes(vals[i]));
        i = i + 1;
    }
    out.push(125);
    return out;
}

/// Serialize `v` to a JSON string.
fn stringify(Json v) -> Result<string, JsonError> {
    return match from_bytes(stringify_bytes(v)) {
        Result::Ok(s) => s,
        Result::Err(_) => raise JsonError::JsonBadString,
    };
}

/// First value for `key` in a `JsonObject`, or `None`.
fn object_get(Json obj, string key) -> Option<Json> {
    match obj {
        Json::JsonObject(keys, vals) => {
            let i = 0;
            while i < len(keys) {
                if keys[i] == key {
                    return Option::Some(vals[i]);
                }
                i = i + 1;
            }
            return Option::None;
        },
        _ => {
            return Option::None;
        },
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

fn json_array(Vec<Json> items) -> Json {
    return Json::JsonArray(items);
}

fn json_object(Vec<string> keys, Vec<Json> vals) -> Json {
    return Json::JsonObject(keys, vals);
}
