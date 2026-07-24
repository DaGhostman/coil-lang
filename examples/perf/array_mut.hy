// CPU: StoreIndex + compound update hot loop.
fn main() {
    let arr = [0, 0, 0, 0, 0, 0, 0, 0];
    let i = 0;
    while (i < 2000) {
        arr[i % 8] += 1;
        i = i + 1;
    }
    print "%i", arr[0] + arr[1] + arr[2] + arr[3] + arr[4] + arr[5] + arr[6] + arr[7];
}
