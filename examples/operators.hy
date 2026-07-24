fn main() {
    let x = 5;
    x += 3;
    print "%i", x;

    let y = 0;
    print "%i", y++;
    print "%i", y;

    let z = 0;
    print "%i", ++z;

    let arr = [10, 20, 30];
    arr[1] += 5;
    print "%i", arr[1];

    let d = { val: 1 };
    d.val += 41;
    print "%i", d.val;

    print "%i", 2 ** 3;
    print "%z", true && false;
    print "%z", 5 != 4;
    print "%i", 7 & 3;
}
