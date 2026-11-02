-- A list-based prime sieve: take the head, remove its multiples from the
-- rest, repeat. Counts the primes below 15,000. Lists are cons cells
-- {head, tail}; nil is the empty list.
local function range(lo, hi, acc)
  while hi >= lo do
    acc = {hi, acc}
    hi = hi - 1
  end
  return acc
end

local function reverse(xs, acc)
  while xs do
    acc = {xs[1], acc}
    xs = xs[2]
  end
  return acc
end

local function remove(p, xs, acc)
  while xs do
    local x = xs[1]
    if x % p ~= 0 then acc = {x, acc} end
    xs = xs[2]
  end
  return reverse(acc, nil)
end

local function sieve(xs, count)
  while xs do
    xs = remove(xs[1], xs[2], nil)
    count = count + 1
  end
  return count
end

print(sieve(range(2, 15000, nil), 0))
