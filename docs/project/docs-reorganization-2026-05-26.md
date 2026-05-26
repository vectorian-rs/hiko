# Documentation Reorganization Summary — 2026-05-26

This note summarizes the documentation reorganization committed as `91a97ed`.

## What changed

The flat `docs/` directory was split into topic folders so agents and maintainers can find the right source-of-truth document faster.

| Area | New folder | Purpose |
|---|---|---|
| Project intent and workflow | [`project/`](./) | Whitepaper, development standards, documentation guidance, historical bootstrap, and evaluation prompts. |
| Language semantics | [`../language/`](../language/) | Effects, errors, modules, numeric policy, SML deltas, and grammar artifact. |
| Runtime and VM architecture | [`../architecture/`](../architecture/) | System snapshot, runtime/process model, VM/runtime seam, memory-layout notes, and historical structured-concurrency note. |
| Builtins and capabilities | [`../builtins/`](../builtins/) | Builtin reference, builtin-domain gating policy, example run config, and Parquet proposal. |
| Verification | [`../verification/`](../verification/) | Bytecode verifier status, formal-spec inventory, TLA+/Quint notes, and dated verification snapshots. |
| Agent prompts | [`../prompts/`](../prompts/) | Reusable prompts and skills for agents working on Hiko. |
| Research/design notes | [`../rnd/`](../rnd/) | Experimental ideas that are not necessarily implemented. |

## Navigation improvements

- Rewrote [`../index.md`](../index.md) as the top-level routing table.
- Added an `index.md` to each topic folder.
- Each folder index lists files, status, and short summaries.
- Reading routes now separate product orientation, public API changes, runtime/VM work, builtins/capabilities, and verification work.

## Link and path fixes

- Updated markdown links after moving files.
- Updated `README.md` documentation links.
- Updated `crates/hiko-vm/src/config.rs` so the config parsing test points at `docs/builtins/full-builtin-run-config.example.toml`.
- Updated the `Word32.hml` numeric-policy comment to point at `docs/language/numerics.md`.

## Validation

- Local markdown link check passed.
- `cargo test -p hiko-vm parse_full_builtin_example_config` passed.

## Follow-up guidance

When adding documentation:

1. Put the file in the nearest topic folder.
2. Update that folder's `index.md`.
3. Update [`../index.md`](../index.md) if the new doc changes top-level routing.
4. Keep historical/proposal docs clearly marked so agents do not treat them as implementation contracts.
