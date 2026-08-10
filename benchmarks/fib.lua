-- Plain naive fib recursion (fair cross-lang with examples/perf/fib.hy).
local function fib(n)
    if n <= 2 then
        return 1
    end
    return fib(n - 1) + fib(n - 2)
end

print(fib(32))
