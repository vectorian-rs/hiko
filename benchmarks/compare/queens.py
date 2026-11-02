"""N-queens, counting all solutions; the queens placed so far are a list of
cons cells (head, tail)."""


def safe(x, d, qs):
    while qs is not None:
        q, qs = qs
        if x == q or x == q + d or x == q - d:
            return False
        d += 1
    return True


def queens(n, k, qs):
    if k == n:
        return 1
    return place(n, k, qs, 0, 0)


def place(n, k, qs, c, acc):
    while c != n:
        if safe(c, 1, qs):
            acc += queens(n, k + 1, (c, qs))
        c += 1
    return acc


print(queens(10, 0, None))
