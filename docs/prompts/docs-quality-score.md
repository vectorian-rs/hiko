# Documentation Quality Scoring Prompt

Use this prompt to review the Hiko `docs/` folder for agent and human usability.

```text
You are reviewing the `docs/` folder of the Hiko repository.

Goal: score the documentation as a working knowledge base for maintainers, contributors, and coding agents. Be strict, terse, and evidence-based. Do not reward volume. Reward docs that are accurate, navigable, concise, current, and useful for making correct changes.

Read enough to judge the documentation structure and the main source-of-truth docs. Start with:

- `docs/index.md`
- `docs/project/index.md`
- `docs/language/index.md`
- `docs/language/definition.md`
- `docs/architecture/index.md`
- `docs/builtins/index.md`
- `docs/verification/index.md`
- `docs/verification/verification.md`
- `docs/project/whitepaper.md`
- `docs/project/dev-standard.md`

Then sample topic docs as needed. Prefer implementation truth over prose truth when checking accuracy.

Score each axis from 0 to 10:

1. Concision
   - Is the documentation as short as it can be while preserving meaning?
   - Does it avoid repetition, stale history, and generic filler?

2. Correctness
   - Do docs match current implementation, tests, and repo structure?
   - Are current/proposed/historical claims clearly separated?
   - Are broken links, stale paths, or inaccurate status claims present?

3. Completeness
   - Can a maintainer find the necessary docs for language, runtime, VM, builtins, capabilities, verification, and workflow?
   - Are important gaps explicitly named rather than hidden?

4. Understandability
   - Can a new contributor or agent quickly understand what to read first and why?
   - Are terms introduced before use?
   - Are design boundaries clear?

5. Navigability
   - Do indexes route readers effectively?
   - Are folder names, file names, and cross-links predictable?
   - Can agents avoid obsolete or irrelevant docs?

6. Terseness / signal density
   - Is the prose direct and high-signal?
   - Are tables and summaries useful rather than decorative?
   - Are long docs justified by content?

7. Quality / maintainability
   - Is there a clear source-of-truth hierarchy?
   - Are update obligations clear?
   - Are docs easy to keep synchronized with code?

8. Agent usefulness
   - Would a coding agent know which docs to load for a task?
   - Are docs explicit about trust level, status, and verification coverage?
   - Are missing specs/gaps clear enough to prevent hallucinated files or features?

Output format:

## Summary

- Overall score: X/10
- Letter grade: A/B/C/D/F
- 3-5 sentence assessment
- State whether `docs/` is currently trustworthy for agents.

## Score table

| Axis | Score | Reason |
|---|---:|---|
| Concision | x/10 | ... |
| Correctness | x/10 | ... |
| Completeness | x/10 | ... |
| Understandability | x/10 | ... |
| Navigability | x/10 | ... |
| Terseness / signal density | x/10 | ... |
| Quality / maintainability | x/10 | ... |
| Agent usefulness | x/10 | ... |

## Findings

Group by severity:

### Critical
Issues that make agents likely to make wrong changes.

### High
Stale, misleading, or missing docs that affect common work.

### Medium
Navigation, duplication, clarity, or completeness issues.

### Low
Polish, wording, minor organization problems.

For each finding include:

- file/path
- problem
- why it matters
- concrete fix

## Strong points

List the strongest docs or structures and why they work.

## Missing or weak areas

List important missing docs/specs, especially if the gap could mislead agents.

## Recommended next edits

Give a prioritized checklist of 5-10 documentation edits. Prefer small, surgical fixes over broad rewrites.
```
