# Extensions

An extension adds builtins, opaque types and Hiko modules from outside the
core crates. The GitHub and AWS integrations are extensions
(`extensions/hiko-ext-github`, `extensions/hiko-ext-aws`); the core has no
code for either.

## What an extension is

`hiko_vm::extension::Extension` is a static description:

```rust
pub static GITHUB: Extension = Extension {
    name: "github",                      // [extensions.github] in a run config
    types: &[],                          // opaque types, such as "aws_config"
    builtins: &[ /* ExtensionBuiltin */ ],
    modules: &[("Github.Issue", include_str!("Issue.hml"))],
    requires: &[],                       // library modules they import, such as "Std.Result"
    configure,                           // fn(&toml::Value) -> Result<ExtensionState, String>
};
```

Each `ExtensionBuiltin` has a name, a Hiko signature in the same syntax as
core builtin signatures, and a function:

- `ExtensionFn::Sync(fn(&[Value], &mut Heap) -> Result<Value, String>)` runs
  on the VM thread, like a core builtin.
- `ExtensionFn::Async(fn(&[Value], &mut Heap) -> Result<AsyncJob, String>)`
  checks its arguments and policy on the VM thread and returns a job. Under
  the threaded runtime the job runs on the I/O pool and the process
  suspends, like `http_get`; otherwise it runs inline. The job gets no heap
  and returns a `SendableValue`, which is charged as I/O.

## Turning one on

A host links the extensions it supports. The CLI links them with cargo
features:

```sh
cargo build --release -p hiko-cli --features ext-github,ext-aws
```

A run config turns a linked extension on with its table, which `configure`
turns into the extension's state:

```toml
[extensions.github]
list = ["vectorian-rs/hiko"]
```

The extension is fail-closed:

- Without a table, the extension is not registered. The compiler does not
  know its builtins or modules, so a script that uses them does not compile.
- A table for an extension the binary does not link is an error.
- A `configure` error stops the VM from being built. Both bundled extensions
  reject unknown keys and default every permission to off.

`hiko build-vm` links the extensions a run config turns on into the
generated crate: it depends on their crates, embeds the `[extensions]` tables
in `src/main.rs`, and registers each with `VMBuilder::with_extension`. When
the `hiko` running `build-vm` links an extension too, its table is checked
with `configure` at generation time; a `--script` program that uses
extensions needs that, so it can be compiled with them. `build-vm` knows the
bundled `github` and `aws` extensions.

## Library modules an extension needs

Library modules come from one place: the project's lockfile, or the
embedded sources of a `build-vm` artifact. An extension does not carry its
own copies. When its modules import one, such as `Std.Result`, it lists it
in `requires`. For a command that takes a script, the project is the
`hiko.toml` in or above the script's directory; `--config` selects policy
only.

While an extension module is compiled, each library module it imports
directly is checked first. A library module's own imports are not checked
against `requires`. The errors are:

- not in `requires`: `module Aws.S3 of extension 'aws' imports Std.List,
  which the extension does not list in its requires; add it to requires in
  the extension's Extension`;
- no project: `module Aws.S3 of extension 'aws' needs Std.Result, which
  comes from a project's lockfile, but <script> has no hiko.toml in or above
  its directory; create one that declares Std with its version and registry
  (see docs/language/imports.md), then run 'hiko install'`, where `<script>`
  is the script's path, or `this program` when the compile has no entry
  file;
- a lockfile without it: `module Aws.S3 of extension 'aws' needs
  Std.Result, which <lockfile> does not pin; if hiko.toml does not declare
  Std, add it with its version and registry (see docs/language/imports.md),
  then run 'hiko install'`, where `<lockfile>` is the resolved path;
- a project with no lockfile, a lockfile that lacks a declared Std, or a
  lockfile that pins Std while `[dependencies]` declares other packages but
  not Std: the existing cause, then `; if hiko.toml does not declare Std,
  add it with its version and registry (see docs/language/imports.md), then
  run 'hiko install'`;
- a library module's own import that is not pinned: `module 'Option' in
  package 'Std' for named import 'Std.Option' not found in <lockfile>; if
  hiko.toml does not declare Std, add it with its version and registry (see
  docs/language/imports.md), then run 'hiko install'`;
- a `build-vm` artifact that does not embed it: `module Aws.S3 of extension
  'aws' needs Std.Result, which this artifact does not embed; build it with
  --lock and a lockfile that pins Std.Result`. For `build-vm --script`, this
  fails when the artifact is generated, since the script is compiled then,
  and the message starts `the script does not compile with only the embedded
  sources: `.

The check is per module: `import Aws.Config`, which imports nothing, needs
no lockfile. A project that uses `Aws.S3`, `Aws.SQS` or `Aws.CloudWatch`
declares `Std` in `[dependencies]` and runs `hiko install`.

An extension module cannot `use` a file, because that would load code from
outside the extension, unchecked. The compiler refuses it with `module
<module> of extension '<extension>' declares use './helper.hml'; an
extension module cannot use a file`.

Each extension's tests call `Extension::undeclared_imports`, so `requires`
stays complete. It finds direct imports, including those in `let` bodies,
and reports a `use` as `<module> uses <path>; an extension module cannot use
a file`.

A policy governs the running program, not loading. Until #187 lands, the
loader can fetch from a registry the script-side manifest names; the bytes
are still checked against the lockfile.

## What the core provides

| Hook | Where |
| --- | --- |
| Opaque types and builtin signatures for the type checker | `InferCtx::add_opaque_type`, `InferCtx::bind_extension_builtin` |
| Modules resolved by import name, before the lockfile | `ModuleLoaderOptions::extensions` (`ExtensionSurface`), built by `Extension::compile_surface` |
| Registration of sync and async builtins | `VMBuilder::with_extension`, `VM::register_async_builtin` |
| Configuration | `RunConfig::extensions`, `configured_extensions`, `build_vm_with_extensions` |
| Per-extension state, shared with child processes | `Heap::extension_state::<T>(name)` |
| Native objects behind handles | `Heap::alloc_extension_resource`, `Heap::extension_resource::<T>`, `HostHandleKind::Extension(kind)` |
| Async work on the I/O pool | `IoRequest::Extension`, `BuiltinKind::Async` |

Extension host resources count under `max_host_resources` and under a
per-kind limit keyed by the kind name, for example:

```toml
[limits.host_resources]
aws_s3_client = 4
```

## Writing one

1. Define the builtins and their signatures. Check the policy in the
   builtin, using the state from `Heap::extension_state`, before any side
   effect.
2. Put the Hiko modules next to the crate and include them with
   `include_str!`. A module can use the extension's builtins directly. If
   the modules import library modules, list them in `requires` (the AWS
   modules list `Std.Result`); see
   [Library modules an extension needs](#library-modules-an-extension-needs). Test that the list is complete
   with `Extension::undeclared_imports`.
3. Make `configure` reject unknown keys and default to no permissions.
4. Test the policy without the external service: every refused call fails
   before any I/O.

`crates/hiko-vm/tests/extension.rs` is a small complete example.

## Cost

- **Dispatch:** a builtin call dispatches on a kind stored at registration,
  so extension builtins cost the same as core ones (#147).
- **Default binary:** the extension hooks add about 104 KB to the default
  `hiko-cli` (6,449,888 to 6,554,080 bytes after #148, macOS arm64), mostly
  the generic run-config table parsing. The default build links no
  extension, and its dependency tree is unchanged (97 crates).
