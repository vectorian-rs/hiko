# Research and Design Notes

Experimental ideas and design sketches that are not necessarily implemented.

| File | Status | Summary |
|---|---|---|
| [`agent-scripting.md`](agent-scripting.md) | R&D/design-only | Sketch for using Hiko as a typed, policy-limited scripting layer for coding agents. |
| [`effects-design.md`](effects-design.md) | R&D/design-only | Effects as capability requests, proposed syntax and effect rows, implementation strategies A to F, comparisons with OCaml 5, Koka, Eff, Frank, Links, and Haskell, staging plan, open questions. |
| [`iac-demo.md`](iac-demo.md) | R&D/design-only | Infrastructure-as-code demo sketch covering Dhall, Hiko, provider builtins, changesets, state commits, state store, and command flow. |
| [`2026-06-vm-performance-investigation.md`](2026-06-vm-performance-investigation.md) | Dated investigation (2026-06) | Filesystem benchmark vs Python: per-file I/O policy overhead and VM bytecode density in list pattern matching, with proposed fixes. |

## Recommended use

Open these only when exploring the specific experiment or mining design ideas. Verify implementation status in code before treating any R&D note as current behavior.
