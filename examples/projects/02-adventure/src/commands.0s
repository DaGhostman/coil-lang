// Command parse for the adventure (byte-line equality — no from_bytes).
//
// Cmd kind: 0=look 1=go 2=take 3=inv 4=save 5=load 6=help 7=quit 8=bad
// Dir: 0=north 1=south 2=east 3=west; unused sentinel = 99.

class Cmd {
    kind: int,
    dir: int,
}

fn bytes_eq([byte] a, [byte] b) -> int {
    if len(a) != len(b) {
        return 0;
    }
    let i = 0;
    let ok = 1;
    while i < len(a) {
        if a[i] != b[i] {
            ok = 0;
        }
        i = i + 1;
    }
    return ok;
}

fn parse_line([byte] line) -> Cmd {
    let look: [byte] = [108, 111, 111, 107];
    let inv: [byte] = [105, 110, 118, 101, 110, 116, 111, 114, 121];
    let take: [byte] = [116, 97, 107, 101];
    let take_key: [byte] = [116, 97, 107, 101, 32, 107, 101, 121];
    let save: [byte] = [115, 97, 118, 101];
    let load: [byte] = [108, 111, 97, 100];
    let help: [byte] = [104, 101, 108, 112];
    let quit: [byte] = [113, 117, 105, 116];
    let exit: [byte] = [101, 120, 105, 116];
    let go_n: [byte] = [103, 111, 32, 110, 111, 114, 116, 104];
    let go_s: [byte] = [103, 111, 32, 115, 111, 117, 116, 104];
    let go_e: [byte] = [103, 111, 32, 101, 97, 115, 116];
    let go_w: [byte] = [103, 111, 32, 119, 101, 115, 116];

    if bytes_eq(line, look) == 1 {
        return new Cmd(0, 99);
    }
    if bytes_eq(line, inv) == 1 {
        return new Cmd(3, 99);
    }
    if bytes_eq(line, take_key) == 1 {
        return new Cmd(2, 99);
    }
    if bytes_eq(line, take) == 1 {
        return new Cmd(2, 99);
    }
    if bytes_eq(line, save) == 1 {
        return new Cmd(4, 99);
    }
    if bytes_eq(line, load) == 1 {
        return new Cmd(5, 99);
    }
    if bytes_eq(line, help) == 1 {
        return new Cmd(6, 99);
    }
    if bytes_eq(line, quit) == 1 {
        return new Cmd(7, 99);
    }
    if bytes_eq(line, exit) == 1 {
        return new Cmd(7, 99);
    }
    if bytes_eq(line, go_n) == 1 {
        return new Cmd(1, 0);
    }
    if bytes_eq(line, go_s) == 1 {
        return new Cmd(1, 1);
    }
    if bytes_eq(line, go_e) == 1 {
        return new Cmd(1, 2);
    }
    if bytes_eq(line, go_w) == 1 {
        return new Cmd(1, 3);
    }
    return new Cmd(8, 99);
}

fn cmd_kind(Cmd c) -> int {
    return c.kind;
}

fn cmd_dir(Cmd c) -> int {
    return c.dir;
}
