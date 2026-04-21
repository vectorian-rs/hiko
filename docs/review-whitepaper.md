 You are reviewing `docs/whitepaper.md` in the Hiko repository.

  Your job is to grade it as a language-design whitepaper for humans and agents who need to understand what Hiko means today, what is intentionally
  simplified, and what is still provisional.

  Important review stance:
  - Prefer implementation truth over prose truth.
  - Prefer current maintained docs over historical notes.
  - Do not reward length.
  - Do not reward elegant wording if the claim is inaccurate, vague, or ungrounded.
  - Do not penalize unsettled surface syntax by itself, but do penalize presenting unsettled syntax as if it were final.
  - Treat this as a design-and-accuracy review, not a marketing review.

  Read only enough context to verify claims in `docs/whitepaper.md`, especially from:
  - `README.md`
  - `docs/runtime.md`
  - `docs/vm.md`
  - `docs/modules.md`
  - `docs/error-handling.md`
  - `docs/system.md`
  - `docs/sml-deltas.md`
  - `libraries/Std-v0.1.0/modules/Fiber.hml`
  - `libraries/Std-v0.1.0/modules/Result.hml`
  - `libraries/Std-v0.1.0/modules/Option.hml`
  - `libraries/Std-v0.1.0/modules/Either.hml`
  - `crates/hiko-vm/src/process.rs`
  - `crates/hiko-vm/src/runtime.rs`
  - `crates/hiko-vm/src/runtime_ops.rs`
  - `crates/hiko-vm/src/threaded.rs`
  - `crates/hiko-vm/src/sendable.rs`
  - `crates/hiko-vm/src/vm/mod.rs`
  - `crates/hiko-vm/src/vm/runtime_bridge.rs`

  Grade the whitepaper with this weighting:

  1. Semantic accuracy and implementation fidelity — 35%
  - Are claims true for the current repo?
  - Are runtime/process/effect/Fiber/cancellation/module claims accurate?
  - Are future ideas clearly marked as future rather than current?

  2. Language-design completeness — 20%
  - Does it explain the important semantic choices?
  - Does it cover result/error handling, spawn/process semantics, local heaps, modules, effects, async model, stdlib Fibers, cancellation, numeric policy, and
  pipeline/operator rationale?

  3. Clarity and navigability — 15%
  - Can a new contributor or agent quickly answer “what does this feature mean?”
  - Are terms defined before use?
  - Are sections well structured and easy to scan?

  4. Design rationale and tradeoff quality — 15%
  - Does it explain why Hiko chose this design rather than just listing features?
  - Are comparisons to alternatives precise and not sloppy?

  5. Current-vs-proposed discipline — 10%
  - Does it clearly distinguish implemented behavior, intended direction, and open questions?
  - Does it avoid implying that unsettled syntax or future numeric/module features are already stable?

  6. Operational usefulness — 5%
  - After reading it, can someone make or review changes without loading half the repo?
  - Does it capture the meanings that matter for implementation and review?

  Scoring:
  - Give each category a score from 0 to 5.
  - Convert to a weighted score out of 100.
  - Then assign a letter grade:
    - A: 93-100
    - A-: 90-92
    - B+: 87-89
    - B: 83-86
    - B-: 80-82
    - C+: 77-79
    - C: 73-76
    - C-: 70-72
    - D: 60-69
    - F: below 60

  Output format:

  1. Summary
  - Overall grade
  - Weighted score
  - 3-6 sentence assessment
  - State explicitly whether the whitepaper is currently trustworthy as a source-of-truth design doc

  2. Score breakdown
  - One line per category with score and short reason

  3. Findings
  - Ordered by severity: Critical, High, Medium, Low
  - For each finding include:
    - why it matters
    - exact claim/section that is wrong, weak, missing, or misleading
    - what the repo currently does instead, if relevant
    - a concrete improvement

  4. Strong sections
  - Call out the sections that are especially clear or reusable
  - Mention if parts should be copied into `README.md` or other docs

  5. Current/proposed boundary check
  - List every place where the doc blurs:
    - implemented today
    - intended direction
    - open design question

  6. Final verdict
  - Answer:
    - “Would you merge this as-is?”
    - “What must change before merge?”
    - “What can wait?”

  Review standards:
  - Be strict.
  - Use file references when possible.
  - Prefer actionable criticism over generic praise.
  - If a claim is unverifiable from the repo, say that clearly.
  - If the whitepaper is good, say so, but do not stop at compliments.

