"""Are We Fast Yet's List benchmark: Takeuchi on list lengths, with lists
compared by walking them. Repeated 300 times. An element is (value, next);
None ends a list."""


def make_list(n):
    return None if n == 0 else (n, make_list(n - 1))


def length(l):
    return 0 if l is None else 1 + length(l[1])


def next_(l):
    return None if l is None else l[1]


def is_shorter(x, y):
    while y is not None:
        if x is None:
            return True
        x = x[1]
        y = y[1]
    return False


def tail(x, y, z):
    if is_shorter(y, x):
        return tail(tail(next_(x), y, z), tail(next_(y), z, x), tail(next_(z), x, y))
    return z


def run(k, acc):
    while k != 0:
        acc += length(tail(make_list(15), make_list(10), make_list(6)))
        k -= 1
    return acc


print(run(300, 0))
