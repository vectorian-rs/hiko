# Imports And Packages

How Hiko programs pull in local files and named library modules, and how named
modules are resolved, fetched, locked, cached, and verified. For `structure`
and `signature`, see [modules.md](modules.md).

## `use` And `import`

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
- not locked or BLAKE3-verified through `hiko.lock.toml` today

### `import`

`import` is for named modules:

```sml
import Std.List
import Std.Prelude
import Std.Option (Some, None)
```

The parenthesized form also brings the listed names into scope unqualified.

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

A project can choose its own directory in `hiko.toml`. A relative path is
resolved against the directory that holds `hiko.toml`:

```toml
[defaults]
lockfile = "hiko.lock.toml"
lib_cache = ".hiko-cache"
```

An embedder that sets `ModuleLoaderOptions::cache_dir` overrides both. The
order is: `ModuleLoaderOptions::cache_dir`, then `[defaults] lib_cache`, then
`~/.hiko/lib-cache/`.

Cache entries are keyed by the locked identity of the module, the file name
being `{blake3(url)}-{hash}.hml`:

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

Use the lock verifier to proactively check every locked remote module, not just
modules imported by the current program:

```sh
hiko lock verify --config hiko.toml
```

The command fetches every locked module, recomputes BLAKE3, and exits nonzero on
any remote drift or manifest/lockfile inconsistency.

The intended security model is:

- **non-keyed BLAKE3 for integrity**
- optional **signatures** later for publisher authenticity

We do **not** want keyed hashes as the primary package trust model. Shared-secret MACs are the wrong primitive for public library distribution.

The cache should be treated as **untrusted storage**. Verification on reuse is not an optimization detail; it is part of the security model.

## What Is Next

The remaining library/import work is operational hardening rather than deeper
module theory:

- add a lock update command for intentional dependency upgrades
- support generated artifacts embedding exact locked content and selected policy
- optionally include local `use` source hashes in generated artifacts for release reproducibility
