-- Deep recursion (Takeuchi); no binary fork-join auto-par shape.
local function tak(x, y, z)
    if y >= x then
        return z
    end
    return tak(tak(x - 1, y, z), tak(y - 1, z, x), tak(z - 1, x, y))
end

print(tak(18, 12, 6))
