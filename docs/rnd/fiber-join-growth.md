# Fiber join growth

Findings for #183 (D5, A8), measured on `main` at `e299c91`. `Fiber.all` is
quadratic, and no change in Hiko alone can make it linear.

## Measurements

Release build on macOS, fastest of three runs. Each fiber returns at once. The
policy is `software-developer-role` with `cancel` and `wait_any` added, and
100,000,000 fuel. `join` is a join in list order, with no fail-fast. `wait` is
the `wait_any` loop of `join_all`, without `in_order`.

| Fibers | `Fiber.all` | `wait` | `join` |
|---|---|---|---|
| 500 | 48 ms | 15 ms | 14 ms |
| 1,000 | 153 ms | 31 ms | 17 ms |
| 2,000 | 567 ms | 90 ms | 20 ms |
| 4,000 | fuel exhausted | 313 ms | 28 ms |
| 8,000 | fuel exhausted | 1,183 ms | 44 ms |

Doubling n multiplies `Fiber.all` by 3 to 4, and fuel runs out by 4,000 fibers.
All three scripts hit `stack overflow` at 16,000 or fail earlier; the script's
own `range` and `spawn_all` are not tail recursive, so that limit is not yet
assigned to `Std.Fiber`.

## Causes

Each costs O(n) per finished fiber, so O(n²) in all:

1. `in_order` runs `value_of`, a list search, once per fiber. This is the
   largest cost: compare `Fiber.all` with `wait`.
2. `remove_first` walks the pending list.
3. `wait_any` takes the pending list each round. The runtime copies it, removes
   duplicates, checks each child, and registers and removes a waiter for each.

## Options

| Option | Result | Cost |
|---|---|---|
| Join in list order | linear | breaks fail-fast: a hung fiber hides a later failure; contradicts Q2 and A7 |
| Hiko only | quadratic | cause 3 stays; Hiko has no array or map for causes 1 and 2 |
| `wait_any` with no list, plus sorting by index | O(n log n) | new runtime queue, and fails A8 as written |
| New builtin, `await_all` | linear | new runtime state, policy key and scheduler tests |

**Recommendation:** `await_all : 'a pid list -> ...`. The runtime blocks the
parent once, on a count of live children, and wakes it on the last success or
the first failure, with values in list order. `Fiber.join_all` becomes a thin
wrapper, and `remove_first` stays only for `Fiber.any`. Not built or measured.

## Concerns

- **#178 merged early.** `e299c91` has no Gate 1 and no lens review, including
  `soundness` and `concurrency`. Sections 2 and 3 of #183 are not done.
- **The builtin touches the scheduler.** It needs the `concurrency` lens, and
  loom or a model if a new block reason changes the wake paths.
- **`Std.Fiber` does not load under the repository's policies.**
  `benchmark` and `software-developer-role` allow `spawn` but not `cancel` or
  `wait_any`, so `import Std.Fiber` fails with `builtin 'cancel' was not
  registered`. This predates #178.
- **Naming.** A new policy key adds to the rename in #186. Settle its name
  under the #185 rule first.
- **Limit.** One machine; these fibers do no work, so a real workload may show
  a different share for each cause.
