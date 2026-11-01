"""A list-based prime sieve: take the head, remove its multiples from the
rest, repeat. Counts the primes below 15,000. Lists are cons cells
(head, tail); None is the empty list."""


def range_(lo, hi, acc):
    while hi >= lo:
        acc = (hi, acc)
        hi -= 1
    return acc


def reverse(xs, acc):
    while xs is not None:
        x, xs = xs
        acc = (x, acc)
    return acc


def remove(p, xs, acc):
    while xs is not None:
        x, xs = xs
        if x % p != 0:
            acc = (x, acc)
    return reverse(acc, None)


def sieve(xs, count):
    while xs is not None:
        p, rest = xs
        xs = remove(p, rest, None)
        count += 1
    return count


print(sieve(range_(2, 15000, None), 0))
