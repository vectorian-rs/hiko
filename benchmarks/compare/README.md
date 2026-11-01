# Cross-language benchmarks

The same programs in Hiko, Lua and Python, to compare Hiko's interpreter with
other implementations. Run them with `scripts/compare.sh`; results are in
[`docs/architecture/performance.md`](../../docs/architecture/performance.md).

| Program | From | What it exercises |
|---|---|---|
| `fib` | classic | Calls, integer arithmetic |
| `tak` | Manticore / MLton | Calls with three arguments |
| `queens` | classic | Recursion over a list of placed queens |
| `binary_trees` | Benchmarks Game | Allocation and garbage collection |
| `mandelbrot` | Benchmarks Game | Float arithmetic in loops |
| `nbody` | Benchmarks Game | Float arithmetic on tuples |
| `sieve` | classic | Building and filtering lists |
| `list` | Are We Fast Yet | Recursion over linked lists |

## Rules

The rules follow Are We Fast Yet (Marr, Daloze and Mössenböck, DLS 2016):
compare implementations, not programs.

- **Same algorithm** in every language, with the same operation order, so
  float results are bit-identical. Every implementation prints the same
  checksum line, and `scripts/compare.sh` fails if they differ.
- **Same data structures.** Hiko has no mutable references or arrays, so the
  other languages do not use them either. A list is a chain of cons cells:
  `{head, tail}` tables in Lua, `(head, tail)` tuples in Python. A record is
  a tuple (Python) or a table that is never modified (Lua).
- **Loops.** A tail-recursive function in Hiko is a loop after compilation,
  so it is a `while` loop in Lua and Python. Non-tail recursion stays
  recursion in every language.
- **No libraries** beyond each language's core: no NumPy, no FFI.
- **Inputs** are sized so that each Hiko program runs for 0.2 to 3 seconds.

Programs that need mutation or arrays (Richards, DeltaBlue, Havlak from Are We
Fast Yet) are not included.

## Annotations

Hiko's type inference does not yet resolve `=` or arithmetic on a type
variable that later code would make `int` or `float`, so some parameters are
annotated (`(x : int, d, qs)`, `body list`).
