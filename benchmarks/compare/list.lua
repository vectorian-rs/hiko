-- Are We Fast Yet's List benchmark: Takeuchi on list lengths, with lists
-- compared by walking them. Repeated 300 times. An element is
-- {value, next}; nil ends a list.
local function make_list(n)
  if n == 0 then return nil end
  return {n, make_list(n - 1)}
end

local function length(l)
  if not l then return 0 end
  return 1 + length(l[2])
end

local function next_(l)
  if not l then return nil end
  return l[2]
end

local function is_shorter(x, y)
  while y do
    if not x then return true end
    x = x[2]
    y = y[2]
  end
  return false
end

local function tail(x, y, z)
  if is_shorter(y, x) then
    return tail(tail(next_(x), y, z), tail(next_(y), z, x), tail(next_(z), x, y))
  end
  return z
end

local function run(k, acc)
  while k ~= 0 do
    acc = acc + length(tail(make_list(15), make_list(10), make_list(6)))
    k = k - 1
  end
  return acc
end

print(run(300, 0))
