# Host handles

Opaque native resources owned by the VM (future database connections, file readers or SDK clients) are represented in Hiko values as `HeapObject::HostHandle { kind, id }`.

No core builtin creates a host handle. Extensions do (see [extensions.md](extensions.md)): `HostHandleKind::Extension(kind)` names the kind, and `Heap::alloc_extension_resource` and `Heap::extension_resource` store and fetch the native object. The AWS extension's configs and clients are the current users. `HostHandleKind::Unsupported` is a kind no builtin creates; tests use it.

The handle is only a typed key. The actual Rust object lives in the owning heap's host-resource table and is dropped with that heap. Builtins must validate both:

1. the heap object's `HostHandleKind`, and
2. the host-resource table entry for the handle id.

Host handles are process-local. They are intentionally rejected by process-boundary serialization and must not be captured as child process results/messages unless a future API explicitly defines transfer semantics.

Each heap tracks its current and peak host-handle counts, in total and per kind. Operators can cap live handles with `limits.max_host_resources` and per-kind `limits.host_resources` entries keyed by kind name. The caps apply to a whole process tree: a parent and everything it spawns share one counter per cap, so spawning cannot multiply them (see [execution-bounds.md](execution-bounds.md#budgets-across-process-boundaries)).

A builtin that creates a host resource should take its capability from policy, and service operations should accept a reusable handle rather than rebuild the native object per call.
