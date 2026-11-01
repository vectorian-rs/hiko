-- N-queens, counting all solutions; the queens placed so far are a list
-- of cons cells {head, tail}.
local function safe(x, d, qs)
  while qs do
    local q = qs[1]
    if x == q or x == q + d or x == q - d then return false end
    d = d + 1
    qs = qs[2]
  end
  return true
end

local queens

local function place(n, k, qs, c, acc)
  while c ~= n do
    if safe(c, 1, qs) then acc = acc + queens(n, k + 1, {c, qs}) end
    c = c + 1
  end
  return acc
end

queens = function(n, k, qs)
  if k == n then return 1 end
  return place(n, k, qs, 0, 0)
end

print(queens(10, 0, nil))
