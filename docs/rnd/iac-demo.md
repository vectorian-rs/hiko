# R&D: Hiko Infrastructure-as-Code Demo

This document sketches a possible showcase application for Hiko: an infrastructure-as-code tool that uses Dhall for typed configuration, Hiko for planning and apply logic, and capability-gated provider builtins for cloud operations.

The goal is not to clone Terraform directly. The goal is to explore whether Hiko can model infrastructure changes as explicit, reviewable, committed state transitions.

## Core idea

A Hiko-based IaC tool could use three first-class artifacts:

1. **Desired configuration**: a Dhall file such as `my-infra.dhall`.
2. **ChangeSet**: a durable diff/transition artifact produced from desired config and current state.
3. **StateCommit**: a committed snapshot of infrastructure state after applying a ChangeSet.

Conceptually:

```text
Dhall desired config + current StateCommit -> ChangeSet
ChangeSet + provider apply operations          -> new StateCommit
```

The important distinction is that the tool applies a previously generated ChangeSet, not whatever configuration happens to exist at apply time.

## Why this is interesting

Terraform state tends to force an awkward tradeoff:

- Large state files preserve more of the dependency graph, but plans become slow and risky.
- Small state files are faster, but cross-state dependencies become remote-state references with weak typing and weak global graph knowledge.

A Hiko demo could explore a different model:

- small or sharded state
- explicit cross-stack imports and exports
- a global dependency/index layer
- durable ChangeSet artifacts
- committed state progression with history
- provider access controlled by Hiko capabilities

This would make state progression part of the system rather than an opaque side effect of apply.

## Roles

### Dhall

Dhall is the human-authored desired configuration language.

It can provide:

- typed stack inputs
- reusable components
- defaults and composition
- normalized configuration
- importable Dhall libraries
- typed cross-stack contracts

Example shape:

```dhall
let Aws = ./lib/aws/package.dhall
let S3 = Aws.S3

in  { stack = "prod"
    , region = "us-east-1"
    , resources =
      [ S3.bucket
          { name = "my-prod-logs"
          , versioning = True
          , publicAccess = S3.PublicAccess.BlockAll
          }
      ]
    }
```

Dhall should describe desired infrastructure. It should not perform provider calls or mutate state.

### Hiko

Hiko is the tool implementation language.

The Hiko program/tool owns:

- loading and normalizing Dhall config
- decoding desired resources
- loading the current committed state
- deciding what provider observations are needed
- computing a ChangeSet
- running policy checks
- applying an approved ChangeSet
- committing the new state
- writing history/audit information

### Provider builtins

Cloud/provider operations should be implemented as capability-gated Rust builtins with thin Hiko wrappers.

For an AWS MVP, the provider layer might expose operations such as:

- `aws_sts_get_caller_identity`
- `aws_s3_head_bucket`
- `aws_s3_create_bucket`
- `aws_s3_delete_bucket`
- `aws_s3_get_bucket_versioning`
- `aws_s3_put_bucket_versioning`
- `aws_s3_get_public_access_block`
- `aws_s3_put_public_access_block`

Hiko should own planning and orchestration. Provider builtins should own safe access to external APIs.

## ChangeSet

A ChangeSet is an applyable state transition.

It should include enough information to answer:

- Which state commit was it planned from?
- Which desired config hash produced it?
- Which provider observations were used?
- What resource changes are proposed?
- What order should changes apply in?
- Which policy checks passed or failed?
- What new state is expected after apply?

Conceptual fields:

```text
ChangeSet
  id
  from_state_hash
  desired_config_hash
  generated_at
  generated_by
  observations_used
  changes
  dependency_order
  policy_results
  expected_state_hash
```

Apply should verify that the current state head still matches `from_state_hash`. If the state has advanced since planning, apply should reject the ChangeSet and require a replan or explicit rebase.

## StateCommit

State should be committed as a progression of snapshots, not treated as a single mutable blob.

A StateCommit should record:

- parent state hash
- applied ChangeSet hash
- author or actor
- timestamp
- resulting state hash
- resources
- outputs
- imports
- provider references

The history should look like:

```text
state_0
  -> changeset_1 applied by alice at T1
    -> state_1
      -> changeset_2 applied by CI at T2
        -> state_2
```

This gives the system native answers to “who changed what when?”

## State store

The state store can be local or remote, but the interface should stay consistent.

Abstract operations:

```text
get_head(stack) -> state_commit_id
read_commit(id) -> StateCommit
write_changeset(changeset) -> changeset_id
commit_apply(parent, changeset, new_state) -> state_commit_id
compare_and_swap_head(stack, parent, new_commit) -> ok | conflict
```

A local prototype could use a content-addressed directory:

```text
.hiko-infra/
  objects/
    ab/cd/...
  refs/
    stacks/prod
  journal.jsonl
```

A remote backend could later use object storage for immutable objects and a database or lock service for refs.

## Command flow

### Plan

```sh
hiko-infra plan my-infra.dhall --stack prod
```

High-level steps:

1. Evaluate and normalize Dhall.
2. Hash the desired config.
3. Load the current state head.
4. Observe provider resources as needed.
5. Compute a ChangeSet.
6. Run policy checks.
7. Store the ChangeSet artifact.
8. Print a human-readable summary.

### Apply

```sh
hiko-infra apply changeset_abc123
```

High-level steps:

1. Load the ChangeSet.
2. Verify the current state head equals `from_state_hash`.
3. Optionally revalidate observations or policy checks.
4. Execute provider operations in dependency order.
5. Build the new state snapshot.
6. Commit the new StateCommit.
7. Advance the stack head atomically.
8. Append audit/history events.

### History

```sh
hiko-infra history prod
```

Possible output:

```text
state_004  CI      2026-04-26  applied changeset_def456  add logs bucket
state_003  alice   2026-04-25  applied changeset_abc123  enable versioning
state_002  bob     2026-04-24  imported existing network
```

## Cross-stack contracts

Instead of Terraform-style remote state as an untyped data source, this system could use typed imports and exports.

A producing stack exports a contract:

```text
prod-network exports Network.v1
```

A consuming stack imports it:

```text
prod-app imports prod-network.Network.v1
```

The state/index layer can then record explicit dependency edges. That enables impact queries and safer destroy behavior.

Example questions the tool should answer:

```sh
hiko-infra graph impact prod-network.vpc
hiko-infra graph dependents prod-network
```

## MVP demo

A useful first demo should be intentionally small:

- one Dhall config file
- one stack
- local content-addressed state store
- one provider: AWS
- one resource type: S3 bucket
- one Hiko tool that can plan, apply, and show history

Initial commands:

```sh
hiko-infra plan examples/infra/my-infra.dhall --stack dev
hiko-infra apply changeset_abc123
hiko-infra history dev
```

Initial resource behavior:

- create bucket if missing
- update versioning/public-access settings
- refuse destructive changes unless policy allows them
- commit state after successful apply

## Open questions

- Should Dhall emit a generic resource graph, or should Hiko map Dhall config into resources?
- What serialization format should ChangeSet and StateCommit use?
- Should local state be Git-backed, content-addressed, or both?
- How much provider observation data should be embedded in the ChangeSet?
- Can a ChangeSet be applied without re-reading the Dhall file?
- How are approvals attached to ChangeSets?
- How should secrets be represented so they do not leak into config, diffs, or state?
- How should typed cross-stack output schemas evolve over time?

## Design thesis

The demo should showcase Hiko as a language for controlled effects and state transitions.

Dhall provides typed desired configuration. Hiko computes and applies explicit ChangeSets. Provider builtins perform capability-gated external operations. State advances through committed StateCommits with history.

This is not “Terraform with different syntax.” It is an experiment in making infrastructure changes explicit, reviewable, and auditable as first-class values.
