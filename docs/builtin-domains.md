# Builtin Domains and Gating

This document records how Hiko classifies builtin modules and how those modules
are gated at compile time and at runtime. It is an architecture guide for adding
or moving builtins; the user-facing builtin reference remains
[builtins.md](./builtins.md).

## Goals

- Keep public Hiko module names stable and understandable.
- Keep Rust implementation paths close to public Hiko module paths where that
  helps navigation.
- Separate compile-time availability from runtime permission decisions.
- Avoid feature churn until there is a concrete need for finer granularity.
- Make directory moves mechanical by deciding domain ownership before moving
  files.

## Domain Categories

Builtins fall into three broad categories.

### 1. Pure core builtins

Pure core builtins are deterministic functions that do not read host state, write
host state, perform network I/O, or depend on provider configuration. They may
still allocate, parse, encode, or use ordinary Rust library code.

Examples:

- string operations
- numeric conversion and math
- bytes operations
- JSON parsing/serialization
- regular expressions
- concrete hash algorithms such as `Hash.Blake3`
- deterministic, seed-driven pseudo-random APIs

Pure core builtins usually need only Cargo feature gating. They normally do not
need `VMBuilder` runtime policy because enabling the code does not grant access
to host resources.

### 2. Host capability builtins

Host capability builtins access local machine state or local operating-system
capabilities. They are not necessarily networked, but they can observe or mutate
state outside the VM heap.

Examples:

- filesystem reads/writes and directory operations
- environment variables
- process metadata
- command execution
- wall-clock time and sleeping
- stdio

Host capability builtins require two decisions:

1. Is the code compiled into this build? This is controlled by a Cargo feature.
2. Is this VM instance allowed to use the capability? This is controlled by
   `VMBuilder` policy.

Some host capabilities can have empty Cargo features because they add API
surface without adding external dependencies. Examples include:

```toml
builtin-filesystem = []
builtin-env = []
builtin-exec = []
```

The empty feature is still valuable: it controls whether the code, public Rust
builder methods, and policy types are present in a build.

### 3. External dependency / provider builtins

External dependency or provider builtins talk to remote services, depend on
provider SDKs, or require provider-style configuration and credentials. They may
also perform network I/O.

Examples:

- `Http.Client`
- future `Aws.Config`
- future `Aws.S3`
- future `Dhall`
- future IaC/provider modules
- future database, queue, object-store, or cloud-control-plane modules

These builtins usually need both Cargo feature gating and runtime policy. Cargo
features control dependencies and compiled API. `VMBuilder` policy controls
which operations, hosts, accounts, buckets, regions, or providers are available
to a specific VM instance.

Provider SDKs may also impose runtime-shape requirements. For example, the Rust
AWS SDK for S3 is async and expects a Tokio runtime. `Aws.Config` and `Aws.S3`
should therefore be designed as async/provider integrations rather than as purely
synchronous builtin functions. The VM/runtime bridge should own how async
provider calls are started, suspended, cancelled, and resumed.

## Two-Tier Gate Model

Hiko uses two gates for builtins that expose optional code or capabilities.

### Cargo features: compile-time gate

Cargo features answer: "Can this build contain this builtin?"

A feature controls:

- module declarations
- builtin registration calls
- dependencies
- public Rust policy types
- public Rust builder methods
- tests that require the builtin

Feature gates should appear at every layer that exposes the optional item:

```rust
#[cfg(feature = "builtin-http")]
mod http;

#[cfg(feature = "builtin-http")]
_entries.extend(http::entries());

#[cfg(feature = "builtin-http")]
pub fn with_http(...);
```

This keeps `--no-default-features` builds small and warning-clean, and it makes
absence of a feature a compile-time contract rather than a runtime surprise.

### `VMBuilder` policy: runtime gate

`VMBuilder` policy answers: "May this VM instance use this compiled builtin?"

Runtime policy should be used when a builtin can observe or affect the host or
external world. Policy can be coarse, such as enabling a builtin, or fine-grained,
such as restricting filesystem paths or HTTP hosts.

Examples:

- `allow_filesystem_builtin("read_file", folders)`
- `allow_http_builtin("http_get", allowed_hosts)`
- `with_exec(policy)`

A builtin may be compiled in and still unavailable to a specific VM if the
builder policy does not allow it.

## Public Module Ownership

Public Hiko module names should describe the semantic domain, not the historical
Rust file where a builtin happened to live.

Current raw builtins are still globally registered, and some stdlib modules wrap
those raw functions. Future public module structure should move toward explicit,
stable domains.

Recommended ownership:

| Public Hiko domain | Category | Notes |
| ------------------ | -------- | ----- |
| `Std.String` | pure core | String processing and formatting helpers. |
| `Std.Bytes` | pure core | Byte buffers and UTF-8 conversion. |
| `Std.Json` | pure core | JSON ADT helpers, parse, serialize, object/array helpers. |
| `Std.Regex` | pure core | Regex matching and replacement helpers. |
| `Std.Math` | pure core | General mathematical operations. |
| `Std.Convert` | pure core | Primitive conversions. |
| `Std.Path` | pure core / host-adjacent | Path string manipulation only; no filesystem access. |
| `Std.Filesystem` | host capability | Local filesystem access; runtime path policy required. |
| `Std.Env` | host capability | Environment-variable access; runtime policy may restrict names. |
| `Std.Process` | host capability | Local process metadata. |
| `Std.Exec` | host capability | Command execution; runtime policy required. |
| `Std.Stdio` | host capability | Standard input/output. |
| `Std.Time` | host capability | Wall-clock time, monotonic time, and sleeping. |
| `Std.Random` | mixed | Seeded PRNG APIs are pure core; host entropy APIs are host capability. |
| `Hash` | pure core family | Top-level algorithm family with concrete modules such as `Hash.Blake3` and future `Hash.Sha256`. |
| `Http` | external/provider family | Top-level protocol family; current client APIs belong under `Http.Client`. |
| `Aws` | external/provider family | Top-level cloud-provider family; shared SDK configuration lives in `Aws.Config`, and the first planned service module is `Aws.S3`. |
| `Dhall` | external/provider | Future config-language integration; feature and import/security policy depend on implementation. |

### Date and time

`date` should be treated as part of the time domain unless it becomes a purely
calendar-arithmetic module.

- Formatting/parsing fixed timestamps can live under `Std.Time` or a
  `Std.DateTime` submodule.
- Reading the current date/time is a host capability because it observes the
  host clock.
- Sleeping is a host capability because it affects scheduling and elapsed time.

Avoid creating a broad top-level `Date` domain unless the language grows a large,
standalone date/time library that is not primarily host-clock access.

### JSON

JSON should remain a standard-library data-format module, preferably `Std.Json`,
not a top-level provider domain. It is deterministic and local. A future
networked JSON API belongs under its network/provider domain, not under JSON.

### Random

Random APIs should distinguish deterministic generators from host entropy.

- Seeded PRNG construction and stepping are pure core if all entropy comes from
  explicit Hiko values.
- APIs such as `random_bytes` that read operating-system entropy are host
  capability builtins and may need runtime policy if embedders need fully
  deterministic or hermetic execution.

A future public module can still be `Std.Random`, but the implementation should
keep the host-entropy boundary visible.

### Hashing

`Hash` should be a top-level algorithm family. Concrete hash modules should
name concrete algorithms:

- `Hash.Blake3`
- future `Hash.Sha256`
- future `Hash.Sha512`

Avoid a vague `Std.Hash` bucket for algorithm-specific APIs. Generic traits or
convenience wrappers can be considered later, but the stable public surface
should make the algorithm explicit.

### HTTP and cloud providers

`Http` should be a top-level protocol family because it is not a standard pure
helper; it performs network I/O and needs runtime policy. The current client
APIs should belong under `Http.Client`, leaving room for a future `Http.Server`
domain rather than placing everything directly under `Http` or under `Std.Http`.

`Aws` should be a top-level provider family. It should not be nested under
`Std`, and policy should be service-aware rather than only host-aware. Hiko's
public AWS modules should map closely to the Rust AWS SDK crate split:

- `aws-config` maps to `Aws.Config`.
- `aws-sdk-s3` maps to `Aws.S3`.
- Future service crates should map the same way, such as `aws-sdk-dynamodb` to
  `Aws.DynamoDB`.

`Aws.Config` is the shared provider/session layer. It owns profile selection,
region, behavior version, credential-source policy, SSO support, HTTP client and
runtime integration, and the loaded AWS SDK config. Service modules such as
`Aws.S3` consume an approved `Aws.Config` and add only service-specific client
creation, operations, and policy such as allowed buckets, prefixes, actions, and
body-size limits.

The Rust implementation should expect the AWS client path to be async. The
standard AWS Rust S3 crate (`aws-sdk-s3`) exposes a Tokio runtime integration via
`rt-tokio`, so `Aws.Config` and `Aws.S3` should be planned around a
Tokio-backed runtime bridge, request cancellation, credential loading policy, and
bounded response/body handling instead of blocking a VM worker thread. Because
AWS SDK versions can also affect MSRV and dependency size, AWS support should
stay behind optional Cargo features.

A good initial dependency shape for Hiko is to disable AWS SDK defaults and opt
into the runtime/client features explicitly. For Hiko's preferred human workflow,
SSO profile support should be enabled deliberately on `aws-config`:

```toml
aws-config = { version = "...", default-features = false, features = [
  "default-https-client",
  "behavior-version-latest",
  "rt-tokio",
  "sso",
] }
aws-sdk-s3 = { version = "...", default-features = false, features = [
  "default-https-client",
  "behavior-version-latest",
  "rt-tokio",
] }
```

With `default-features = false`, the `aws-config` `sso` feature is required for
profiles that use `sso_session`, `sso_account_id`, and `sso_role_name`. The S3
service crate does not need an SSO feature; it receives credentials through the
loaded AWS config.

This shape keeps SSO explicit while still avoiding other defaults unless Hiko
chooses to support and policy them. In particular, keep `credentials-process` out
by default because it executes a host command, and avoid service-crate default
extras such as `sigv4a` until a concrete use case requires them.

Hiko should not delegate AWS authentication to an unrestricted SDK default
credential chain. Auth source selection is an `Aws.Config` policy concern, not an
`Aws.S3` concern. `Aws.Config` should expose an explicit enum of supported auth
methods. Initially, the only supported method should be SSO profile auth:

```rust
pub enum AwsConfigAuthMethod {
    SsoProfile { profile: String },
}
```

Other credential sources should only be added when Hiko has a concrete runtime
story and policy checks for them. Sensitive sources such as environment
variables, shared credentials files, and `credential_process` should be disabled
unless explicitly allowed by future runtime policy. `Aws.Config` should validate
the requested auth method, profile, account, and region before constructing the
AWS SDK config; `Aws.S3` should then validate service-specific resource policy
before creating or using an S3 client.

The Cargo feature shape should follow the same split. `builtin-aws-config` owns
`aws-config`; each AWS service feature depends on it and adds its own service
crate:

```toml
builtin-aws-config = ["dep:aws-config"]
builtin-aws-s3 = ["builtin-aws-config", "dep:aws-sdk-s3"]
```

This validates the idea that every AWS service feature requires `aws-config`:
service crates need a loaded AWS SDK config to construct clients, and centralizing
that requirement keeps auth/profile policy in one place instead of duplicating it
inside each service module.

## Rust Path Convention

Rust paths should mirror public Hiko module paths where practical. This makes it
easier to navigate from a Hiko module to its builtin implementation.

Preferred future shape:

```text
crates/hiko-vm/src/builtins/std/string.rs
crates/hiko-vm/src/builtins/std/bytes.rs
crates/hiko-vm/src/builtins/std/json.rs
crates/hiko-vm/src/builtins/std/time.rs
crates/hiko-vm/src/builtins/std/filesystem.rs
crates/hiko-vm/src/builtins/hash/blake3.rs
crates/hiko-vm/src/builtins/http/client.rs
crates/hiko-vm/src/builtins/aws/config.rs
crates/hiko-vm/src/builtins/aws/s3.rs
```

Implementation-only helpers should remain private support modules and do not
need to mirror public Hiko names exactly:

```text
crates/hiko-vm/src/builtins/support.rs
crates/hiko-vm/src/builtins/http_args.rs
crates/hiko-vm/src/builtins/json_value.rs
```

Domain modules should expose local registration functions:

```rust
pub(super) fn entries() -> Vec<(&'static str, BuiltinFn)> {
    vec![("bytes_length", bytes_length)]
}
```

The root builtin registry should only collect feature-gated domain entries.

## Feature Naming Policy

Feature names are semantic contracts. Do not split or rename features solely to
make the tree look tidy.

Current broad features are acceptable until there is a concrete need for finer
granularity. For example, keep:

```toml
builtin-hash = []
```

until a second hash algorithm lands. When that happens, split deliberately:

```toml
builtin-hash-blake3 = []
builtin-hash-sha256 = []
builtin-hash = ["builtin-hash-blake3", "builtin-hash-sha256"]
```

The same rule applies to provider families. Add `builtin-aws-config` when
`Aws.Config` exists and add `builtin-aws-s3` when `Aws.S3` exists; do not add
placeholder features before code or API exists.

## Adding a Builtin Checklist

When adding a new builtin domain or moving an existing one:

1. Choose the public Hiko domain first.
2. Classify it as pure core, host capability, or external/provider.
3. Add or reuse a Cargo feature for compile-time gating.
4. If it is host/provider capability, add `VMBuilder` policy and config support.
5. Put `#[cfg(feature = "...")]` at every layer:
   - module declaration
   - registry `entries()` call
   - builder policy type and methods
   - tests that require the feature
6. Keep helpers private and item-level gated if they are only used by optional
   domains.
7. Ensure both default and no-default builds are warning-clean:

```sh
cargo clippy -p hiko-vm --all-targets -- -D warnings
cargo clippy -p hiko-vm --no-default-features -- -D warnings
```

## Directory Move Plan

The next directory hierarchy refactor should be mechanical:

1. Create `builtins/std/` for standard-library domains.
2. Move pure std and host std domains under `builtins/std/` according to this
   document.
3. Create algorithm/provider directories only when needed, such as
   `builtins/hash/blake3.rs` or `builtins/aws/s3.rs`.
4. Preserve existing feature names unless a new API requires a split.
5. Keep root registration behavior unchanged except for module path updates.
