-- Integer loops + array mutation; Sieve of Eratosthenes (n = 2^14).
local function nsieve(n)
    local flags = {}
    for i = 0, n - 1 do
        flags[i] = 1
    end
    local count = 0
    for p = 2, n - 1 do
        if flags[p] == 1 then
            count = count + 1
            for k = p + p, n - 1, p do
                flags[k] = 0
            end
        end
    end
    return count
end

print(nsieve(1 << 14))
