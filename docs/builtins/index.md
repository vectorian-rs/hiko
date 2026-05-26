# Builtins and Capability Documentation

Builtin surface, builtin-domain architecture, run-config policy, and proposed capability-backed features.

| File | Status | Summary |
|---|---|---|
| [`builtins.md`](builtins.md) | Current reference | Builtin surface grouped by domain: I/O, conversions, strings, math, filesystem, path, hashline, HTTP, JSON, bytes, RNG, system, process, and testing. |
| [`builtin-domains.md`](builtin-domains.md) | Current architecture policy | Builtin domain categories, compile-time Cargo feature gates, runtime `VMBuilder` policy gates, Rust layout conventions, and naming stability. |
| [`full-builtin-run-config.example.toml`](full-builtin-run-config.example.toml) | Example config | Example builtin-granular run config covering limits and capability policy. |
| [`parquet.md`](parquet.md) | Proposal/design-only | Proposed Parquet API, schema/value mapping, display surface, capability model, implementation shape, and open questions. |

## Recommended use

- Adding or changing a builtin: read [`builtins.md`](builtins.md) and [`builtin-domains.md`](builtin-domains.md).
- Changing run configs or capability gates: read [`builtin-domains.md`](builtin-domains.md) and [`full-builtin-run-config.example.toml`](full-builtin-run-config.example.toml).
- Parquet-specific work: read [`parquet.md`](parquet.md), then verify implementation status in code.
