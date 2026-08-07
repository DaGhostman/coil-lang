// Integer loops + array mutation; Sieve of Eratosthenes (n = 2^14).
function nsieve(n) {
    const flags = new Array(n);
    for (let i = 0; i < n; i++) {
        flags[i] = 1;
    }
    let count = 0;
    for (let p = 2; p < n; p++) {
        if (flags[p] === 1) {
            count++;
            for (let k = p + p; k < n; k += p) {
                flags[k] = 0;
            }
        }
    }
    return count;
}

console.log(nsieve(1 << 14));
