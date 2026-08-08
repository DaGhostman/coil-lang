// Nested loops + float arithmetic; checksum of Mandelbrot escape iterations.
function mandelbrot(size, maxIter) {
    let sum = 0;
    for (let y = 0; y < size; y++) {
        for (let x = 0; x < size; x++) {
            const cr = (2.0 * x) / size - 1.5;
            const ci = (2.0 * y) / size - 1.0;
            let zr = 0.0;
            let zi = 0.0;
            let iter = 0;
            while (iter < maxIter) {
                const zr2 = zr * zr;
                const zi2 = zi * zi;
                if (zr2 + zi2 > 4.0) {
                    break;
                }
                const tr = zr2 - zi2 + cr;
                zi = 2.0 * zr * zi + ci;
                zr = tr;
                iter++;
            }
            sum += iter;
        }
    }
    return sum;
}

console.log(mandelbrot(160, 50));
