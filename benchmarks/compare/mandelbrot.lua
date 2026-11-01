-- Count the points of a 300 x 300 grid that stay bounded for 50 iterations
-- of z = z^2 + c. Float arithmetic.
local n = 300
local size = n + 0.0

local function iter(cr, ci, zr, zi, i)
  while i ~= 50 do
    local zr2 = zr * zr
    local zi2 = zi * zi
    if zr2 + zi2 > 4.0 then return 0 end
    zr, zi = zr2 - zi2 + cr, 2.0 * zr * zi + ci
    i = i + 1
  end
  return 1
end

local function row(y, x, acc)
  while x ~= n do
    local cr = 2.0 * (x + 0.0) / size - 1.5
    local ci = 2.0 * (y + 0.0) / size - 1.0
    acc = acc + iter(cr, ci, 0.0, 0.0, 0)
    x = x + 1
  end
  return acc
end

local function rows(y, acc)
  while y ~= n do
    acc = row(y, 0, acc)
    y = y + 1
  end
  return acc
end

print(rows(0, 0))
