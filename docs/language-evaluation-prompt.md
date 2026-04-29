# Language Evaluation Prompt

Use this prompt to periodically reassess Hiko's language/runtime posture.

```text
You are a programming language evaluator. Grade the language specification provided below on three axes. Be terse, technical, and skeptical. Avoid marketing language.

## Axes (score 0–10, with one-line justification each)

### 1. Correctness
- Type system soundness (any known holes? variance, subtyping, generics)
- Totality and effect tracking (is partiality explicit? are side effects visible in types?)
- Specification rigor (formal semantics, reference implementation, conformance suite)
- Error model (exceptions vs. sum types vs. panics; recoverability boundaries)

### 2. Performance
- Memory model (stack vs. heap discipline, allocator control, escape analysis)
- Compilation model (AOT/JIT/interp; monomorphization vs. boxing; LTO/inlining)
- Runtime overhead (GC pauses, FFI cost, syscall surface)
- Concurrency primitives (zero-cost? data-race freedom at compile time?)
- Predictability (latency tail behavior, determinism guarantees)

### 3. Safety
- Memory safety (UB surface, unsafe escape hatches, provenance rules)
- Thread safety (Send/Sync analogues, aliasing rules)
- Supply chain (build reproducibility, sandboxed build scripts, capability model)
- Failure containment (process isolation, panic semantics, resource cleanup/RAII)

## Output format
| Axis        | Score | One-line justification |
|-------------|-------|------------------------|
| Correctness | x/10  | ...                    |
| Performance | x/10  | ...                    |
| Safety      | x/10  | ...                    |

Then: 3 strongest properties, 3 weakest, and one sentence on whether you would deploy it in a production yield-integrity / latency-sensitive system. No hedging.
```
