# Substitution chains in type inference (2026-11)

Why some programs made Hiko's type checker super-linear even with type
levels, the fix that merged with #168, and the general fix to use if a
shape ever defeats it. Measured cases are in
`crates/hiko-types/tests/growth.rs` (#169, extended by #168).

**Status:** fix 1 below is on `main` (`InferCtx::link_vars`, in `unify`'s
variable/variable case and in `unify_rows`' same-tail case). No measured
shape defeats it. Fix 2 is not implemented.

## The problem

Before #168, the checker kept a substitution, `subst: HashMap<u32, Type>`, from type
variables to types. Unifying two unbound variables binds one to the other
(`bind_var`), and resolving a type (`apply`) follows the bindings to their end
and rebuilds the type. Nothing ever shortens a chain of bindings, so a program
that keeps linking variables built a long chain, and every later `apply` of
a type in it paid for the whole chain. This is still how `apply` works; what
changed is which way two variables are linked.

Example (#169, `many_local_values_sharing_a_type_variable`):

```sml
fun f y =
  let
    val u0 = y
    val u1 = y
    (* ... *)
  in u0 end
```

Each `val u_i = y` unifies `y`'s type with a fresh pattern variable `p_i`
and bound the older variable to the newer one, so `y`'s type became the
chain `a -> p0 -> p1 -> ... -> pN`. Each binding then applied `y`'s type:
O(N) per binding, O(N^2) in all. Before levels, the environment walk at
every generalization added a further factor:

| N | before #168 | #168 with levels only (`b546481`) | `main` (`10b2daa`), levels and fix 1 |
|---|---|---|---|
| 100 | 183,403 | 5,977 | 725 |
| 200 | 1,396,303 | 21,927 | 1,425 |
| 400 | 10,912,103 (over the 10M default limit) | 83,827 | 2,825 |
| 800 | | 327,627 | 5,625 |

Type work. Before #168 it grew about 8x per doubling and rejected the
program at N = 400; levels alone brought it to about 4x, still quadratic;
with fix 1 it grows 2x per doubling.

The other three shapes in `growth.rs` are linear on `main` too (type work at
N = 100, 200, 400, 800):

| Shape | `main` (`10b2daa`) |
|---|---|
| chained (`val u_i = u_(i-1)`) | 626, 1,226, 2,426, 4,826 |
| through calls (a variable passed through function arguments) | 3,652, 7,252, 14,452, 28,852 |
| through tuples (built and matched through tuple patterns) | 1,827, 3,627, 7,227, 14,427 |

## Three fixes

| Fix | What it does | Covers | Cost |
|---|---|---|---|
| 1. Orient variable bindings (**merged**, #168) | When unifying two unbound variables, bind the newer one (by level, then id) to the older | All four measured shapes | Small |
| 2. Union-find with path compression and union by rank | `find` re-points every variable on a path straight at its representative; linking puts the smaller tree under the larger, and the merged variable keeps the lower level | Every chain shape: near-constant time per operation | Moderate |
| 3. Mutable type graph (OCaml) | No global substitution: type variables are shared cells updated in place, and a resolved copy is built only to report or export a type | Everything above, and no repeated rebuilding of whole types | A rewrite of the checker's core |

Fix 1 is in place, and every measured shape, including a variable passed
through function arguments and one matched through tuple patterns, is
linear with it. It is not a proof: nothing yet shows that every route by
which unification links two variables goes through `link_vars`, and the
shapes listed below are not measured.

## Fix 2: when to do it

Do fix 2 if a growth-test shape or a fuzz timeout shows a chain. Until then
it is a guarantee, not a measured need. Reasons to prefer it over further
special cases:

- It keeps the substitution design, so it is a contained change.
- It fits type levels: "the merged variable keeps the lower level" is what
  generalization by levels needs, and the rank rule decides the direction
  that fix 1 chooses by age.
- It turns "linear on the shapes we tested" into a guarantee for every
  shape, which matters because the checker's input is untrusted.
- With it, the direction rule of fix 1 is subsumed by union by rank plus
  "the merged variable keeps the lower level".

### Implementation notes

- `find(v)` must update links while resolving, but `apply` takes `&self`.
  Either give the substitution interior mutability (a `RefCell<Vec<...>>`
  of parent links, or `Cell` per variable) or make `apply` take `&mut self`.
  Variable ids are dense `u32`s, so a `Vec` indexed by id is cheaper than
  the `HashMap`.
- Keep variable-to-variable links (the union-find forest) separate from
  variable-to-type bindings: a representative either is unbound or holds a
  type. `apply` resolves a variable with `find`, then applies its type if it
  has one.
- `type_work` should charge each step of `find`, so the growth test still
  measures it.
- The check-generalization feature (#168) compares levels against the
  environment walk; it continues to apply unchanged.

## How to look for a chain

`growth.rs` has the shared, chained, through-calls and through-tuples
shapes; all pass on `main`. Shapes not yet covered: chains through
datatype constructor patterns, and through `case` branches that unify
their results. A new shape that grows more than about 2x per
doubling, or a `fuzz_typecheck` input that times out, is the signal to do
fix 2.
