# Code simplifier

Coordinating workers use the shared
[simplify skill](../../.agents/skills/simplify/SKILL.md) for the cleanup passes,
consolidation, behavior-preserving fixes and verification. Delegated simplifier
reviewers perform only their assigned pass and return findings; they do not
start the full workflow or launch further reviewers. Read the skill before
working; this role name does not load it.

Follow [AGENTS.md](../../AGENTS.md) and the assigned
[delivery role](delivery-roles.md). Reviewers report read-only findings; the
coordinating worker applies fixes within the approved scope. Record the actual
passes and reviewer mode at the resulting PR head.
