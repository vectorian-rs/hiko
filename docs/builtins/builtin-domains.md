# Builtin Domains and Gating

This document records how Hiko classifies builtin modules and how those modules
are gated at compile time and at runtime. It is an architecture guide for adding
or moving builtins; the user-facing builtin reference remains
[builtins.md](builtins.md).

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
- future `Dhall`
- future IaC/provider modules
- future database, queue, object-store, or cloud-control-plane modules

These builtins usually need both Cargo feature gating and runtime policy. Cargo
features control dependencies and compiled API. `VMBuilder` policy controls
which operations, hosts, accounts, buckets, regions, or providers are available
to a specific VM instance.

Provider SDKs may also impose runtime-shape requirements; cloud SDKs are often
async and expect their own runtime. Such modules should be designed as
async/provider integrations rather than as purely synchronous builtin functions. The VM/runtime bridge should own how async
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
|---|---|---|
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

Cloud providers follow the same pattern. Hiko had an AWS integration
(`Aws.Config`, `Aws.S3`, `Aws.SQS`) until plan U (#140) removed it as out of
focus. A future provider should be a top-level family, not nested under `Std`,
behind optional Cargo features, with policy that is service-aware rather than
only host-aware, and with credential sources chosen explicitly by policy rather
than taken from an SDK's default chain.

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

The same rule applies to provider families: add a provider's feature when its
module exists, and do not add placeholder features before code or API exists.

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

Status: partly done.

- Done: provider and algorithm directories exist for `hash/` (`blake3.rs`) and
  `http/` (`client.rs`).
- Not done: `builtins/std/` does not exist. Standard-library domains such as
  `string.rs`, `bytes.rs`, `json.rs`, `time.rs`, and `filesystem.rs` still sit
  directly under `builtins/`.

The remaining move is mechanical: create `builtins/std/`, move the std domains
into it, keep feature names, and change only module paths in root
registration.
