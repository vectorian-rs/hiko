# Reviewer

Review the requested diff or files for concrete defects and regressions. Follow
the shared repository instructions in [AGENTS.md](../../AGENTS.md) and the
[development standard](../project/dev-standard.md).

Run the shared [code-review skill](../../.agents/skills/code-review/SKILL.md)
for its audits, finding validation and reporting. Keep this role read-only
unless fixes are explicitly requested. Record actual passes and reviewer mode.

- For an [issue cycle](issue-cycle.md), name the plan revision and exact PR
  head. Check each acceptance item against its evidence and required CI/reviews.
- Read surrounding code, callers, and tests to establish the intended behavior.
- Prioritize correctness, capability boundaries, resource limits, cancellation,
  ABI compatibility, and feature combinations affected by the change.
- Verify suspicions against the implementation. Separate confirmed defects from
  questions and identify missing evidence where it matters.
- Keep the review read-only unless the user also requests fixes. Avoid style
  comments already handled by formatting or linting.
- Lead with findings in severity order. Each finding should identify the file
  and line, the trigger, the practical consequence, and a concrete correction.
- If no actionable findings remain, say so and note material validation gaps.
  Report checks actually run without claiming untested behavior is verified.
- Findings within the approved scope return to implementation and verification.
  A changed head needs affected evidence checked again. Label self-review;
  report an independent review only when it was actually independent.
