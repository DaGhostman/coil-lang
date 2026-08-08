-- Recursive alloc + walk; binary trees checksum (max depth 10).
local function bottom_up(depth)
    if depth == 0 then
        return nil
    end
    return { left = bottom_up(depth - 1), right = bottom_up(depth - 1) }
end

local function item_check(tree)
    if tree == nil then
        return 1
    end
    return 1 + item_check(tree.left) + item_check(tree.right)
end

local n = 10
local sum = item_check(bottom_up(n + 1))
local long_lived = bottom_up(n)
for depth = 4, n, 2 do
    local iterations = 1 << (n - depth + 4)
    local c = 0
    for _ = 1, iterations do
        c = c + item_check(bottom_up(depth))
    end
    sum = sum + c
end
sum = sum + item_check(long_lived)
print(sum)
