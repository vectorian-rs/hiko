# Modules in Hiko

## Current Status

Hiko now has a **minimal module system**.

Implemented:

- top-level `signature`
- top-level `structure`
- transparent ascription `:`
- opaque ascription `:>`
- qualified names like `List.fold` and `Queue.t`

Not implemented:

- `functor`
- `open`
- `where type`
- sharing constraints
- first-class modules
- nested modules inside `struct ... end`

This is deliberate. Hiko currently treats modules as a **single top-level namespace layer**, not as a full recursive module language.

## What The Current Model Is For

The module system is there to solve two practical problems:

- **namespacing**
- **representation hiding**

That is enough for APIs like:

```sml
signature LIST = sig
  val fold : ('a * 'b -> 'b) -> 'b -> 'a list -> 'b
end

structure List : LIST = struct
  fun fold f acc xs =
    case xs of
        [] => acc
      | x :: rest => fold f (f (x, acc)) rest
end
```

and for opaque abstractions like:

```sml
signature BOX = sig
  type t
  val make : int -> t
  val get : t -> int
end

structure Box :> BOX = struct
  datatype t = Box of int
  fun make x = Box x
  fun get (Box x) = x
end
```

Outside the structure:

- `Box.t` exists
- `Box.make` and `Box.get` are usable
- the concrete representation of `Box.t` is hidden

## Syntax

### Signature

```sml
signature LIST = sig
  val fold : ('a * 'b -> 'b) -> 'b -> 'a list -> 'b
end
```

### Structure

```sml
structure List = struct
  fun fold f acc xs =
    case xs of
        [] => acc
      | x :: rest => fold f (f (x, acc)) rest
end
```

### Transparent Ascription

```sml
structure List : LIST = struct
  fun fold f acc xs = acc
end
```

### Opaque Ascription

```sml
structure Box :> BOX = struct
  datatype t = Box of int
  fun make x = Box x
  fun get (Box x) = x
end
```

### Qualified Access

```sml
val sum = List.fold (fn (x, acc) => x + acc) 0 [1, 2, 3]
val q : Queue.t = Queue.empty
```

## Semantics

In Hiko, modules are **compile-time only**.

- they do not have a VM/runtime representation
- qualified names are resolved in the front end
- structures are flattened before inference and code generation

This keeps the runtime simple and matches Hiko’s general design: abstraction belongs in the language front end, not in the VM.

## Relationship To `use` And `import`

Hiko needs two distinct composition mechanisms:

- `use "./file.hml"` for **explicit local file inclusion**
- `import Std.List` for **named library/module imports**

They are deliberately different.

### `use`

`use` is for local source composition only:

```sml
use "./helpers.hml"
use "../shared/math.hml"
```

Properties:

- path-based
- explicit
- resolved relative to the importing file
- intended for local project source, not packages

### `import`

`import` is for named modules:

```sml
import Std.List
import Std.Prelude
import Http.Client
```

Properties:

- exactly two segments: `Package.Module`
- both segments are single identifiers
- logical name, not path syntax
- resolved through project metadata and lock data
- fetched over HTTP
- verified before use
- cached for reuse

So the intended split is:

- `use` = local file mechanism
- `import` = named library mechanism
- modules themselves still provide namespace and abstraction

## Deliberate Limits

### No Functors

Hiko does not need functors yet.

The practical use case for `MakeSet` / `MakeMap` can be handled with:

- ordinary modules
- opaque types
- explicit comparator-passing at the value level

That avoids importing higher-order module complexity.

### No Nested Modules

Nested modules are part of SML and OCaml, but Hiko does not need them yet.

For now, one top-level module layer is enough:

- `List`
- `Queue`
- `Http`
- `Generator`

If internal organization becomes painful later, this can be revisited. Right now it is better treated as **out of scope** than as an unfinished promise.

## HTTP-Backed Library Loading

The library system should load named modules over HTTP.

This applies equally to:

- the standard library
- the prelude
- third-party packages

There is no special local stdlib path. `Std.List` should use the same loader as any other named package module.

The prelude is not auto-injected. It is a normal package module and should be imported explicitly:

```sml
import Std.Prelude
```

### Source-Level Shape

Source code stays simple and versionless:

```sml
import Std.List
import Std.Prelude
```

The source does not embed:

- raw URLs
- cache paths
- integrity hashes
- package versions

Those belong in project metadata and lock data.

Nested module hierarchies are not supported for named imports. If something feels like it wants a third segment, use:

- a compound module name such as `ListExtras` or `ClientTls`
- a sibling module
- internal `structure` declarations inside the module body

The resolver only needs to split on the first `.`.

### Published Shape

The intended publishing model is close to the Dhall Prelude store:

- a **versioned package root**
- a **module-per-file layout**
- a **package manifest/index** for browsing and tooling

Example shape:

```text
https://modules.example.com/Std-v0.1.0/
https://modules.example.com/Std-v0.1.0/modules/List.hml
https://modules.example.com/Std-v0.1.0/modules/Prelude.hml
https://modules.example.com/Std-v0.1.0/package.toml
```

This means:

- packages have visible release versions
- modules remain individually addressable
- tooling can browse a package root without downloading everything eagerly

Module names do not nest. Published modules live in one flat directory:

- `Std.List` -> `Std-v0.1.0/modules/List.hml`
- `Std.Map` -> `Std-v0.1.0/modules/Map.hml`
- `Std.ListExtras` -> `Std-v0.1.0/modules/ListExtras.hml`

Listing the `modules/` directory should show the entire package surface at a glance.

### Manifest, Locking, And Integrity

Named imports resolve through the project manifest and lockfile.

The project manifest (`hiko.toml`) declares dependency intent:

```toml
[defaults]
lockfile = "hiko.lock.toml"

[registries.local]
url = "http://127.0.0.1:8000"

[dependencies]
Std = { version = "0.1.0", registry = "local" }
Aws = { version = "0.1.0", registry = "local" }
```

`defaults.lockfile` is resolved relative to the directory containing
`hiko.toml`. Named imports search upward from the entry file for `hiko.toml` and
then use that one configured project lockfile. Entry-file-relative lockfiles are
not part of the model.

The lockfile (`hiko.lock.toml`) records exact resolved package roots and module
hashes:

```toml
schema_version = 1

[packages.Std]
version = "0.1.0"
base_url = "https://modules.example.com/Std-v0.1.0"

[packages.Std.modules]
Prelude = "blake3:..."
List = "blake3:..."
ListExtras = "blake3:..."
```

Resolution is **strict**:

- if a named import is missing from `hiko.lock.toml`, compilation fails
- if `hiko.toml` declares dependencies, every locked package must be declared
- dependency versions must match between `hiko.toml` and `hiko.lock.toml`
- dependency registries must exist in `hiko.toml`
- the locked package `base_url` must match `<registry-url>/<Package>-v<version>`
- the compiler does not silently resolve, rewrite, or update the lockfile during ordinary builds

Important points:

- **`hiko.toml` is intent**: package names, versions, registries, defaults, and policies
- **`hiko.lock.toml` is exact identity**: resolved package roots and per-module hashes
- **BLAKE3 is the actual byte-level authority**

Version tells us what release we intended to use. The package base URL tells us
where the release lives. The BLAKE3 hash tells us whether the bytes of an
individual module are exactly the bytes we locked.

### Cache Model

Remote modules are cached locally for reuse.

Default cache location:

```text
~/.hiko/lib-cache/
```

Cache entries should be keyed by the locked identity of the module:

- resolved module URL
- integrity hash

### Verification Model

The loader must verify integrity twice:

1. when a module is fetched over HTTP
2. again when a cached module is loaded later

That means cached content is not trusted just because it already exists locally.

Failure behavior should be explicit:

- network down + valid cache hit -> proceed
- network down + no cache entry -> fail
- fetched bytes do not match locked BLAKE3 -> fail hard
- cached bytes do not match locked BLAKE3 -> discard and refetch

There should be no silent lockfile update or automatic hash rewrite on mismatch.

The intended security model is:

- **non-keyed BLAKE3 for integrity**
- optional **signatures** later for publisher authenticity

We do **not** want keyed hashes as the primary package trust model. Shared-secret MACs are the wrong primitive for public library distribution.

The cache should be treated as **untrusted storage**. Verification on reuse is not an optimization detail; it is part of the security model.

## Local `use` Versus Named `import`

Local source files are included with `use`:

```hiko
use "./support.hml"
```

The path is resolved relative to the file containing the `use`. Local `use`
files are project source, not locked package dependencies, so they are not
verified through `hiko.lock.toml` today.

Named package modules are imported with `import`:

```hiko
import Std.Option
import Aws.S3
```

Named imports are resolved through `hiko.toml` and `hiko.lock.toml`, fetched from
package roots, cached, and BLAKE3-verified before compiling.

## What Is Next

The remaining library/import work is operational hardening rather than deeper
module theory:

- add a lock update/verify command for proactive remote drift checks
- make the remote module cache location configurable
- support generated artifacts embedding exact locked content and selected policy
- optionally include local `use` source hashes in generated artifacts for release reproducibility
