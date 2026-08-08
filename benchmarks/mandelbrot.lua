-- Nested loops + float arithmetic; checksum of Mandelbrot escape iterations.
local function mandelbrot(size, max_iter)
    local sum = 0
    for y = 0, size - 1 do
        for x = 0, size - 1 do
            local cr = (2.0 * x / size) - 1.5
            local ci = (2.0 * y / size) - 1.0
            local zr, zi = 0.0, 0.0
            local iter = 0
            while iter < max_iter do
                local zr2 = zr * zr
                local zi2 = zi * zi
                if zr2 + zi2 > 4.0 then
                    break
                end
                local tr = zr2 - zi2 + cr
                zi = 2.0 * zr * zi + ci
                zr = tr
                iter = iter + 1
            end
            sum = sum + iter
        end
    end
    return sum
end

print(mandelbrot(160, 50))
