// CPU: dict field read/write pressure (GetField / SetField).
fn main() {
    let d = { x: 0, y: 0 };
    let i = 0;
    while (i < 2000) {
        d.x += 1;
        d.y += 2;
        i = i + 1;
    }
    print "%i", d.x + d.y;
}
