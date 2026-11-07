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
Hash = { version = "0.1.0", registry = "local" }
```

`defaults.lockfile` is resolved relative to the directory containing
`hiko.toml`. Named imports search upward from the entry file for `hiko.toml` and
then use that one configured project lockfile. Entry-file-relative lockfiles are
not part of the model.

A project is the nearest `hiko.toml` above the canonical script path. A symlink
to a script keeps the target's project identity. Without `--config`, `run` and
`check` use that project's policy registry and require the canonical working
directory to be inside that project. Running from elsewhere requires an explicit
manifest, for example `hiko run --config /path/to/hiko.toml --policy core script.hml`.
`--config` selects policy authority; imports still use the script project's
lockfile. `hiko install` and `hiko lock verify` take no script, so `--config`
names their project.

`run` requires a policy even for programs using only core builtins. Register a
named policy such as `policies/core.policy.toml` in `hiko.toml`. The repository's
`core` policy explicitly grants the former core surface, including stdio,
environment access and process operations; an empty policy grants no optional
capabilities. `check` can type-check without a policy and never
executes the program. Policy paths, like lockfiles, must remain inside their
manifest directory, including resolved symlinks. A malformed policy reports its
path, line, column and reason without printing its source line. Relative policy
entries and filesystem capability folders still use the working directory.

The lockfile must stay inside the directory that holds `hiko.toml`, so
subprojects cannot share `../hiko.lock.toml`. Each command checks this when it
reads the manifest, even for a script with no import: `hiko run`, `hiko check`,
`hiko inspect-work`, `hiko install` (also with `--check`), `hiko lock verify`
and `hiko build-vm --lock`. `hiko install` then writes nothing. A path that
leaves the directory is rejected:

```text
project manifest '<manifest>' sets lockfile to '<value>', which is outside its directory; use a path inside it
```

This covers a relative or absolute path outside the directory, a `..` below the
deepest directory that exists, such as `new/../../planted`, and a final symlink
that leaves the directory, also at the default name `hiko.lock.toml`. A
dangling symlink cannot be shown to stay inside, so it is rejected too, with its
own error. A missing path inside the directory passes this check; a program that
needs that lockfile fails with the missing-lockfile error. A script's `hiko.toml`
that cannot be read or parsed fails the command, even for a script with no
import. `hiko build-vm --script` checks the script's project as well.

A `hiko.toml` or lockfile that does not parse gives its path, line, column and
reason, and no source line, so an error cannot print a line of a file the path
reaches. The reason can still quote a value, as in `invalid type: string "x"`.

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

### Installing

`hiko install` writes the lockfile from the manifest. It is the only command
that changes `hiko.lock.toml`; compiling never does.

```sh
hiko install            # the hiko.toml found upward from the current directory
hiko install --check    # write nothing; fail if the lockfile is out of date
```

For each `[dependencies]` entry it:

1. fetches `<registry url>/<Package>-v<version>/package.toml` and checks that
   it names that package and version;
2. downloads every module the package lists and checks each against the
   package's BLAKE3 hash;
3. stores each module in the module cache, where the loader finds it.

Then it writes every module of every dependency to the lockfile, and prints
one line per difference from the lockfile it replaces (`+` added, `-` removed,
`~` changed). If any package or module fails to download or verify, nothing is
written. If storing a module in the cache fails, the lockfile is not written;
modules stored before the failure stay in the cache, which is safe because the
loader checks every cached module against its hash when it reads it. Packages
are locked whole, as `package.toml` lists them; there is no per-module
selection. A package removed from `[dependencies]` is removed from the
lockfile, and with no dependencies left the lockfile lists no packages. An
existing lockfile that does not read (a merge conflict, an unknown schema) is
replaced, with a line saying so. Package names must be identifiers and versions
may hold only letters, digits, `.`, `-` and `+`, since both become part of the
package URL.

A module already in the cache is not downloaded again if it still matches its
hash; a cached copy that does not match is downloaded again, as the loader
does.

`--check` is for CI: it reads only the package manifests, compares the
lockfile they give with the one on disk, writes nothing, and exits nonzero if
they differ. `hiko lock verify` checks the modules themselves.

### Cache Model

Remote modules are cached locally for reuse.

Default cache location:

```text
~/.hiko/lib-cache/
```

A project can choose its own directory in `hiko.toml`. A relative path is
resolved against the directory that holds `hiko.toml`. Unlike the lockfile, the
directory can sit outside the project and be shared: every cached module is
checked against its lockfile hash on load.

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

Pinned modules cannot contain local `use` declarations, including inside `let`,
`local`, functions, or structures. Their tokens are checked before parsing,
type inference, or reuse of an inferred module, so a separate parse/type error
does not hide the refusal. Comments and string contents do not count as declarations.
This rule also applies to transitive imports, verified local package sources,
and modules embedded by `build-vm`. Ordinary project files can still use local files.

Verified modules have a separate compiler-cache identity from local `use`
files, even at the same canonical path. Loading a cache file as project source
cannot make a later named import reuse that unverified program.

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

## Generated Artifacts

`hiko build-vm` can seal a program's modules into the binary it generates
(#59), so the artifact reads no lockfile, cache or network at run time:

```sh
hiko build-vm run.toml --lock hiko.lock.toml                    # scripts from the command line
hiko build-vm run.toml --lock hiko.lock.toml --script main.hml  # exactly this program
```

- `--lock` embeds every module in the lockfile. Each is read through the
  normal loader and checked against its BLAKE3 hash. In the artifact, named
  imports resolve only from these modules, and each is checked again when it
  is imported.
- `--script` also embeds the entry script and the local files it `use`s. The
  binary runs only that program and refuses any argument. `build-vm` compiles
  the program once against exactly the embedded sources, so a missing module
  or file fails at generation time, not in the artifact.
- The generated `src/embedded.rs` lists every module with its hash, and the
  generated `build.rs` refuses to build if a module no longer matches it.
- The generated crate uses `hiko-compile` without the HTTP loader
  (`loader-embedded` only), so the artifact has no code to fetch modules.

Sources are embedded, not compiled bytecode: the generated crate stays
readable, and there is no serialized bytecode format to trust.

## Extensions

An extension's modules import library modules through the project's
lockfile too. See "Library modules an extension needs" in
[extensions](../architecture/extensions.md).

## What Is Next

The remaining library/import work is operational hardening rather than deeper
module theory:

- make `local_package_root` (used by `build-vm` and tests) exclusive: today a
  module missing from it falls through to the cache and the network
- embed only the modules a `--script` program imports, not the whole lockfile
