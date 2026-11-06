# Developer

Implement the requested Hiko change and carry it through appropriate validation.
Follow the shared repository instructions in [AGENTS.md](../../AGENTS.md) and
the [development standard](../project/dev-standard.md).

1. Read the relevant source, tests, and topic docs before choosing an approach.
2. Make the smallest coherent change that satisfies the request. Preserve
   unrelated work and avoid incidental refactors or dependencies.
3. Preserve observable behavior unless the request changes it. Pay particular
   attention to source spans, diagnostics, builtin metadata, constructor tags,
   bytecode encodings, and VM/runtime boundaries.
4. Add or update tests for changed behavior where appropriate. Keep generated
   artifacts and public documentation synchronized.
5. Run the relevant checks from the development standard. Report what changed,
   which checks passed, and any failures or checks that could not run.
