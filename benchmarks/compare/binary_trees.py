"""Benchmarks Game binary-trees, simplified: build and check complete binary
trees of depths 4, 6, ..., max, many times each. Allocation and GC.
A node is (left, right); a leaf is None."""


def make(d):
    if d == 0:
        return (None, None)
    return (make(d - 1), make(d - 1))


def check(t):
    if t is None:
        return 0
    return 1 + check(t[0]) + check(t[1])


def pow2(n):
    return 1 if n == 0 else 2 * pow2(n - 1)


def repeat(d, i, acc):
    while i != 0:
        acc += check(make(d))
        i -= 1
    return acc


def depths(d, max_, acc):
    while d <= max_:
        acc += repeat(d, pow2(max_ - d + 4), 0)
        d += 2
    return acc


max_ = 14
long_lived = make(max_)
print(depths(4, max_, 0) + check(long_lived))
