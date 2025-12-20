-- local function fib(n)
--     if (n < 2) then
--         return n
--     end
--
--     return fib(n - 1) + fib(n - 2)
-- end

local function fib(n)
    local a = 0;
    local b = 1;

    for i=1,n do
        a, b = b, a + b
    end

    return b

end

print(fib(32));
