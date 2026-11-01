-- Benchmarks Game n-body, functional: five bodies as tables that are never
-- modified, each step builds new bodies. Float arithmetic; prints the final
-- energy as an int (energy * 1e9, floored), so every language prints the
-- same line. A list is cons cells {head, tail}.
local pi = 3.141592653589793
local solar_mass = 4.0 * pi * pi
local days = 365.24

local function body(x, y, z, vx, vy, vz, m)
  return {x, y, z, vx * days, vy * days, vz * days, m * solar_mass}
end

local function list(...)
  local items, l = {...}, nil
  for i = #items, 1, -1 do l = {items[i], l} end
  return l
end

local initial = list(
  body(0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0),
  body(4.841431442464721, -1.1603200440274284, -0.10362204447112311, 0.001660076642744037, 0.007699011184197404, -0.0000690460016972063, 0.0009547919384243266),
  body(8.34336671824458, 4.124798564124305, -0.4035234171143214, -0.002767425107268624, 0.004998528012349172, 0.000023041729757376393, 0.0002858859806661308),
  body(12.894369562139131, -15.111151401698631, -0.22330757889265573, 0.002964601375647616, 0.0023784717395948095, -0.000029658956854023756, 0.00004366244043351563),
  body(15.379697114850917, -25.919314609987964, 0.17925877295037118, 0.0026806777249038932, 0.001628241700382423, -0.00009515922545197159, 0.000051513890204661145)
)

local function reverse(xs, acc)
  while xs do acc = {xs[1], acc}; xs = xs[2] end
  return acc
end

local function momentum(bs, px, py, pz)
  while bs do
    local b = bs[1]
    px, py, pz = px + b[4] * b[7], py + b[5] * b[7], pz + b[6] * b[7]
    bs = bs[2]
  end
  return px, py, pz
end

local function offset(bs)
  local s = bs[1]
  local px, py, pz = momentum(bs, 0.0, 0.0, 0.0)
  return {{s[1], s[2], s[3], 0.0 - px / solar_mass, 0.0 - py / solar_mass, 0.0 - pz / solar_mass, s[7]}, bs[2]}
end

local function pull(b, others, i, j, dt)
  local x, y, z, vx, vy, vz, m = b[1], b[2], b[3], b[4], b[5], b[6], b[7]
  while others do
    local o = others[1]
    if i ~= j then
      local dx = x - o[1]
      local dy = y - o[2]
      local dz = z - o[3]
      local d2 = dx * dx + dy * dy + dz * dz
      local mag = dt / (d2 * math.sqrt(d2))
      local om = o[7]
      vx, vy, vz = vx - dx * om * mag, vy - dy * om * mag, vz - dz * om * mag
    end
    others = others[2]
    j = j + 1
  end
  return {x, y, z, vx, vy, vz, m}
end

local function move(b, all, i, dt)
  local p = pull(b, all, i, 0, dt)
  return {p[1] + dt * p[4], p[2] + dt * p[5], p[3] + dt * p[6], p[4], p[5], p[6], p[7]}
end

local function step(bs, all, i, dt, acc)
  while bs do
    acc = {move(bs[1], all, i, dt), acc}
    bs = bs[2]
    i = i + 1
  end
  return reverse(acc, nil)
end

local function advance(bs, n)
  while n ~= 0 do
    bs = step(bs, bs, 0, 0.01, nil)
    n = n - 1
  end
  return bs
end

local function potential(b, others, e)
  local x, y, z, m = b[1], b[2], b[3], b[7]
  while others do
    local o = others[1]
    local dx = x - o[1]
    local dy = y - o[2]
    local dz = z - o[3]
    e = e - m * o[7] / math.sqrt(dx * dx + dy * dy + dz * dz)
    others = others[2]
  end
  return e
end

local function energy(bs, e)
  while bs do
    local b = bs[1]
    e = potential(b, bs[2], e + 0.5 * b[7] * (b[4] * b[4] + b[5] * b[5] + b[6] * b[6]))
    bs = bs[2]
  end
  return e
end

local final = advance(offset(initial), 20000)
print(math.floor(energy(final, 0.0) * 1000000000.0))
