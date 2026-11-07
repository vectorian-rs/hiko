# Developer

Implement the requested Hiko change and carry it through appropriate validation.
Follow the shared repository instructions in [AGENTS.md](../../AGENTS.md) and
the [development standard](../project/dev-standard.md).

For tracked lifecycle work, follow [issue-cycle.md](issue-cycle.md) under the
assigned worker role, approved plan revision, and recorded claim. Resume its
checkpoint and reuse the issue's existing PR.

1. Read the relevant source, tests, and topic docs before choosing an approach.
2. Make the smallest coherent change that satisfies the request. Preserve
   unrelated work and avoid incidental refactors or dependencies.
3. Preserve observable behavior unless the request changes it. Pay particular
   attention to source spans, diagnostics, builtin metadata, constructor tags,
   bytecode encodings, and VM/runtime boundaries.
4. Add or update tests for changed behavior where appropriate. Keep generated
   artifacts and public documentation synchronized.
5. Run the relevant checks and review the result. Fix failures within the
   approved scope and verify again without another approval. Return material
   scope/design changes to planning; preserve the acceptance criteria.
6. Report acceptance results, required checks, review findings, and CI at the
   exact PR head. Record pending/blocked checks and the next action; a ready PR
   goes to the maintainer for merge before dependent work begins.
