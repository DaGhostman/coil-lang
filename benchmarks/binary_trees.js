// Recursive alloc + walk; binary trees checksum (max depth 10).
function bottomUp(depth) {
    if (depth === 0) {
        return null;
    }
    return { left: bottomUp(depth - 1), right: bottomUp(depth - 1) };
}

function itemCheck(tree) {
    if (tree === null) {
        return 1;
    }
    return 1 + itemCheck(tree.left) + itemCheck(tree.right);
}

const n = 10;
let sum = itemCheck(bottomUp(n + 1));
const longLived = bottomUp(n);
for (let depth = 4; depth <= n; depth += 2) {
    const iterations = 1 << (n - depth + 4);
    let c = 0;
    for (let i = 0; i < iterations; i++) {
        c += itemCheck(bottomUp(depth));
    }
    sum += c;
}
sum += itemCheck(longLived);
console.log(sum);
