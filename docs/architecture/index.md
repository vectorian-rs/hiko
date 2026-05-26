# Architecture Documentation

Runtime, VM, crate structure, performance-sensitive layout, and architecture history.

| File | Status | Summary |
|---|---|---|
| [`system.md`](system.md) | Snapshot/orientation | Repo-wide system description: crate map, dependency graph, compilation pipeline, type system, runtime representation, builtins, configs, harness, dependencies, and future work. Exact counts may be stale. |
| [`runtime.md`](runtime.md) | Current source of truth | Isolated-process runtime model, process boundaries, sendable values, effects, async I/O, scheduler/process ownership, and structured concurrency surface. |
| [`vm.md`](vm.md) | Current source of truth | VM ownership boundaries, execution/runtime transition contract, process creation path, current cost model, and benchmark command. |
| [`rust-memory-layout.md`](rust-memory-layout.md) | Current engineering note | Rust inline layout, enum sizing, boxing guidance, optional fields, serde/wire-format cautions, and checklist for Hiko hot-path changes. |
| [`structured-concurrency.md`](structured-concurrency.md) | Historical/design note | Early resource-lifetime and cancellation design. Useful for rationale; prefer [`runtime.md`](runtime.md) for current behavior. |

## Recommended use

- Runtime/scheduler/cancellation work: read [`runtime.md`](runtime.md) first, then [`vm.md`](vm.md).
- VM allocation or hot-path work: read [`vm.md`](vm.md) and [`rust-memory-layout.md`](rust-memory-layout.md).
- Orientation only: read [`system.md`](system.md), but verify counts and code paths against the current tree.
