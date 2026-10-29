# Architecture Documentation

Runtime, VM, crate structure, performance-sensitive layout, and architecture history.

| File | Status | Summary |
|---|---|---|
| [`system.md`](system.md) | Orientation | Crate map, dependency graph, compile pipeline, and a table pointing to the source-of-truth doc for each topic. |
| [`runtime.md`](runtime.md) | Current source of truth | Isolated-process runtime model, process boundaries, sendable values, effects, async I/O, scheduler/process ownership, and structured concurrency surface. |
| [`vm.md`](vm.md) | Current source of truth | VM ownership boundaries, execution/runtime transition contract, process creation path, current cost model, and benchmark command. |
| [`execution-bounds.md`](execution-bounds.md) | Current embedder contract | `VM::run` vs `run_slice`, cooperative scheduling boundaries, GC trigger/root behavior, non-preemptive sections, and latency/resource-limit guidance. |
| [`rust-memory-layout.md`](rust-memory-layout.md) | Current engineering note | Rust inline layout, enum sizing, boxing guidance, optional fields, serde/wire-format cautions, and checklist for Hiko hot-path changes. |
| [`structured-concurrency.md`](structured-concurrency.md) | Historical/design note | Early resource-lifetime and cancellation design. Useful for rationale; prefer [`runtime.md`](runtime.md) for current behavior. |

## Recommended use

- Runtime/scheduler/cancellation work: read [`runtime.md`](runtime.md) first, then [`vm.md`](vm.md).
- Execution bounds, latency, GC, or embedder resource limits: read [`execution-bounds.md`](execution-bounds.md).
- VM allocation or hot-path work: read [`vm.md`](vm.md), [`execution-bounds.md`](execution-bounds.md), and [`rust-memory-layout.md`](rust-memory-layout.md).
- Orientation: start with [`system.md`](system.md) to find the right detailed doc.
