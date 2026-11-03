# Research and Design Notes

Experimental ideas and design sketches that are not necessarily implemented.

| File | Status | Summary |
|---|---|---|
| [`agent-scripting.md`](agent-scripting.md) | R&D/design-only | Sketch for using Hiko as a typed, policy-limited scripting layer for coding agents. |
| [`effects-design.md`](effects-design.md) | R&D/design-only | Effects as capability requests, proposed syntax and effect rows, implementation strategies A to F, comparisons with OCaml 5, Koka, Eff, Frank, Links, and Haskell, staging plan, open questions. |
| [`iac-demo.md`](iac-demo.md) | R&D/design-only | Infrastructure-as-code demo sketch covering Dhall, Hiko, provider builtins, changesets, state commits, state store, and command flow. |
| [`2026-06-vm-performance-investigation.md`](2026-06-vm-performance-investigation.md) | Dated investigation (2026-06) | Filesystem benchmark vs Python: per-file I/O policy overhead and VM bytecode density in list pattern matching, with proposed fixes. |
| [`2026-11-perceus-reuse.md`](2026-11-perceus-reuse.md) | Dated analysis (2026-11) | Perceus-style reference counting with reuse for Hiko (#154): cycles, processes, compiler and verifier, cost model; recommends no-go for now and a narrow reuse experiment after #153. |
| [`2026-11-register-vm-plan.md`](2026-11-register-vm-plan.md) | Plan (2026-11) | Register VM for Hiko (#155), the last task of the performance series: expected gain, cost, staging by load-time translation of verified stack bytecode with equivalence fuzzing, and work-limit accounting. |
| [`2026-11-vm-performance-review.md`](2026-11-vm-performance-review.md) | Dated review (2026-11) | Profiles of the cross-language suite: interpretive overhead, not GC, is the gap to Lua; ranked changes (release profile, constructor building, helper inlining, cached `ip`, allocation fixes) and where SIMD applies. |
| [`2026-11-interpreter-research.md`](2026-11-interpreter-research.md) | Dated survey (2026-11) | Published interpreter and memory techniques mapped to Hiko: tail-call dispatch and stack caching, superinstructions and specialization, register VMs (wasmi, BEAM, Lua), verifier side tables, Perceus-style reuse, BOLT; with a suggested order. |

## Recommended use

Open these only when exploring the specific experiment or mining design ideas. Verify implementation status in code before treating any R&D note as current behavior.
