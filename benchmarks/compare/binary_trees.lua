-- Benchmarks Game binary-trees, simplified: build and check complete binary
-- trees of depths 4, 6, ..., max, many times each. Allocation and GC.
-- A node is {left, right}; a leaf is false.
local function make(d)
  if d == 0 then return {false, false} end
  return {make(d - 1), make(d - 1)}
end

local function check(t)
  if not t then return 0 end
  return 1 + check(t[1]) + check(t[2])
end

local function pow2(n)
  if n == 0 then return 1 end
  return 2 * pow2(n - 1)
end

local function repeat_(d, i, acc)
  while i ~= 0 do
    acc = acc + check(make(d))
    i = i - 1
  end
  return acc
end

local function depths(d, max, acc)
  while d <= max do
    acc = acc + repeat_(d, pow2(max - d + 4), 0)
    d = d + 2
  end
  return acc
end

local max = 14
local long_lived = make(max)
print(depths(4, max, 0) + check(long_lived))
