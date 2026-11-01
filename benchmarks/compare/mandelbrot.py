"""Count the points of a 300 x 300 grid that stay bounded for 50 iterations
of z = z^2 + c. Float arithmetic."""

n = 300
size = float(n)


def iter_(cr, ci, zr, zi, i):
    while i != 50:
        zr2 = zr * zr
        zi2 = zi * zi
        if zr2 + zi2 > 4.0:
            return 0
        zr, zi = zr2 - zi2 + cr, 2.0 * zr * zi + ci
        i += 1
    return 1


def row(y, x, acc):
    while x != n:
        cr = 2.0 * float(x) / size - 1.5
        ci = 2.0 * float(y) / size - 1.0
        acc += iter_(cr, ci, 0.0, 0.0, 0)
        x += 1
    return acc


def rows(y, acc):
    while y != n:
        acc = row(y, 0, acc)
        y += 1
    return acc


print(rows(0, 0))
