# Host handles

Opaque native resources owned by the VM (AWS SDK configs, S3/SQS clients, future database connections/readers) are represented in Hiko values as `HeapObject::HostHandle { kind, id }`.

The handle is only a typed key. The actual Rust object lives in the owning heap's host-resource table and is dropped with that heap. Builtins must validate both:

1. the heap object's `HostHandleKind`, and
2. the host-resource table entry for the handle id.

Host handles are process-local. They are intentionally rejected by process-boundary serialization and must not be captured as child process results/messages unless a future API explicitly defines transfer semantics.

Each heap tracks its current and peak host-handle counts, in total and per kind. Operators can cap live handles with `limits.max_host_resources` and per-kind `limits.host_resources` entries such as `aws_config`, `aws_s3_client`, and `aws_sqs_client`. The caps apply to a whole process tree: a parent and everything it spawns share one counter per cap, so spawning cannot multiply them (see [execution-bounds.md](execution-bounds.md#budgets-across-process-boundaries)).

Current AWS API shape:

```sml
val cfg = Aws.Config.sso_profile "profile"
val cfg2 = Aws.Config.instance_profile ()
val s3 = Aws.S3.client cfg
val buckets = Aws.S3.list_buckets s3

val sqs = Aws.SQS.client cfg
val queues = Aws.SQS.list_queues sqs
```

`Aws.Config.sso_profile` is gated by the allowed SSO profile policy. `Aws.Config.instance_profile` is separately gated by `capabilities.aws.config.instance_profile`. Service `client` functions create reusable host resources; service operations accept those clients rather than rebuilding a client from raw config per call.
