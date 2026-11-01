"""Benchmarks Game n-body, functional: five bodies as tuples, each step
builds new bodies. Float arithmetic; prints the final energy as an int
(energy * 1e9, floored), so every language prints the same line. A list is
cons cells (head, tail)."""

import math

pi = 3.141592653589793
solar_mass = 4.0 * pi * pi
days = 365.24


def body(x, y, z, vx, vy, vz, m):
    return (x, y, z, vx * days, vy * days, vz * days, m * solar_mass)


def cons_list(*items):
    l = None
    for item in reversed(items):
        l = (item, l)
    return l


initial = cons_list(
    body(0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0),
    body(4.841431442464721, -1.1603200440274284, -0.10362204447112311, 0.001660076642744037, 0.007699011184197404, -0.0000690460016972063, 0.0009547919384243266),
    body(8.34336671824458, 4.124798564124305, -0.4035234171143214, -0.002767425107268624, 0.004998528012349172, 0.000023041729757376393, 0.0002858859806661308),
    body(12.894369562139131, -15.111151401698631, -0.22330757889265573, 0.002964601375647616, 0.0023784717395948095, -0.000029658956854023756, 0.00004366244043351563),
    body(15.379697114850917, -25.919314609987964, 0.17925877295037118, 0.0026806777249038932, 0.001628241700382423, -0.00009515922545197159, 0.000051513890204661145),
)


def reverse(xs, acc):
    while xs is not None:
        x, xs = xs
        acc = (x, acc)
    return acc


def momentum(bs, px, py, pz):
    while bs is not None:
        (_, _, _, vx, vy, vz, m), bs = bs
        px, py, pz = px + vx * m, py + vy * m, pz + vz * m
    return px, py, pz


def offset(bs):
    (x, y, z, _, _, _, m), rest = bs
    px, py, pz = momentum(bs, 0.0, 0.0, 0.0)
    return ((x, y, z, 0.0 - px / solar_mass, 0.0 - py / solar_mass, 0.0 - pz / solar_mass, m), rest)


def pull(b, others, i, j, dt):
    x, y, z, vx, vy, vz, m = b
    while others is not None:
        (ox, oy, oz, _, _, _, om), others = others
        if i != j:
            dx = x - ox
            dy = y - oy
            dz = z - oz
            d2 = dx * dx + dy * dy + dz * dz
            mag = dt / (d2 * math.sqrt(d2))
            vx, vy, vz = vx - dx * om * mag, vy - dy * om * mag, vz - dz * om * mag
        j += 1
    return (x, y, z, vx, vy, vz, m)


def move(b, all_, i, dt):
    x, y, z, vx, vy, vz, m = pull(b, all_, i, 0, dt)
    return (x + dt * vx, y + dt * vy, z + dt * vz, vx, vy, vz, m)


def step(bs, all_, i, dt, acc):
    while bs is not None:
        b, bs = bs
        acc = (move(b, all_, i, dt), acc)
        i += 1
    return reverse(acc, None)


def advance(bs, n):
    while n != 0:
        bs = step(bs, bs, 0, 0.01, None)
        n -= 1
    return bs


def potential(b, others, e):
    x, y, z, _, _, _, m = b
    while others is not None:
        (ox, oy, oz, _, _, _, om), others = others
        dx = x - ox
        dy = y - oy
        dz = z - oz
        e = e - m * om / math.sqrt(dx * dx + dy * dy + dz * dz)
    return e


def energy(bs, e):
    while bs is not None:
        (x, y, z, vx, vy, vz, m), rest = bs
        e = potential((x, y, z, vx, vy, vz, m), rest, e + 0.5 * m * (vx * vx + vy * vy + vz * vz))
        bs = rest
    return e


final = advance(offset(initial), 20000)
print(math.floor(energy(final, 0.0) * 1000000000.0))
