//! Extensions: builtins, opaque types and Hiko modules defined outside the
//! core, such as the GitHub and AWS crates under `extensions/`.
//!
//! An [`Extension`] is a static description. A host links the extension
//! crates it wants, and a run config turns one on with an
//! `[extensions.<name>]` table, which [`Extension::configure`] turns into the
//! extension's state. An extension without a table is not registered: the
//! compiler does not know its builtins or modules, and the VM does not
//! register its functions.
//!
//! See `docs/architecture/extensions.md`.

use std::any::Any;
use std::fmt;
use std::sync::{Arc, Mutex};

use hiko_builtin_meta::BuiltinTypeSignature;
use hiko_compile::compiler::{ExtensionRequires, ExtensionSurface};

use crate::heap::Heap;
use crate::sendable::SendableValue;
use crate::value::{BuiltinFn, Value};

/// What [`Extension::configure`] returns, and what builtins read back with
/// [`Heap::extension_state`]. Child processes share it.
pub type ExtensionState = Arc<dyn Any + Send + Sync>;

/// Work an async builtin hands to the I/O pool. It runs off the VM thread,
/// so it gets no heap; it returns plain data.
pub type AsyncJob = Box<dyn FnOnce() -> Result<SendableValue, String> + Send>;

/// The VM-thread half of an async builtin: check the arguments and the
/// policy, then return the job to run.
pub type AsyncBuiltinFn = fn(&[Value], &mut Heap) -> Result<AsyncJob, String>;

#[derive(Clone, Copy)]
pub enum ExtensionFn {
    /// Runs on the VM thread, like a core builtin.
    Sync(BuiltinFn),
    /// Prepares a job that runs on the I/O pool when the VM runs under the
    /// threaded runtime, and inline otherwise.
    Async(AsyncBuiltinFn),
}

#[derive(Clone, Copy)]
pub struct ExtensionBuiltin {
    pub name: &'static str,
    /// Hiko type, in the same syntax as core builtin signatures. It may name
    /// the extension's opaque types.
    pub signature: BuiltinTypeSignature,
    pub func: ExtensionFn,
}

pub struct Extension {
    /// Name of the run-config table, `[extensions.<name>]`, and of the
    /// state in the heap.
    pub name: &'static str,
    /// Opaque types, such as `aws_config`.
    pub types: &'static [&'static str],
    pub builtins: &'static [ExtensionBuiltin],
    /// Hiko modules by import name, such as `("Github.Issue", source)`.
    pub modules: &'static [(&'static str, &'static str)],
    /// Library modules those modules import, such as `"Std.Result"`. They
    /// come from the project's lockfile like any named import. When one of
    /// the extension's modules imports one that is not pinned, or one not
    /// listed here, compiling fails with an error naming the module and the
    /// extension.
    pub requires: &'static [&'static str],
    /// Turn the run-config table into the extension's state. An error stops
    /// the VM from being built.
    pub configure: fn(&toml::Value) -> Result<ExtensionState, String>,
}

impl Extension {
    /// The named imports in the extension's modules, at any depth, that are
    /// neither its own modules nor listed in `requires`, as `"<module>
    /// imports <name>"`. Compiling such an import is an error; each
    /// extension's tests assert this is empty, so the error cannot reach a
    /// user.
    pub fn undeclared_imports(&self) -> Result<Vec<String>, String> {
        use hiko_syntax::ast::{Decl, DeclKind};
        fn imports(decls: &[Decl], out: &mut Vec<hiko_syntax::intern::Symbol>) {
            for decl in decls {
                match &decl.kind {
                    DeclKind::Import(name) | DeclKind::ImportWithNames(name, _) => out.push(*name),
                    DeclKind::Local(locals, body) => {
                        imports(locals, out);
                        imports(body, out);
                    }
                    DeclKind::Structure { decls, .. } => imports(decls, out),
                    _ => {}
                }
            }
        }
        let mut undeclared = Vec::new();
        for (module, source) in self.modules {
            let tokens = hiko_syntax::lexer::Lexer::new(source, 0)
                .tokenize()
                .map_err(|e| format!("{module}: {}", e.message))?;
            let program = hiko_syntax::parser::Parser::new(tokens)
                .parse_program()
                .map_err(|e| format!("{module}: {}", e.message))?;
            let mut names = Vec::new();
            imports(&program.decls, &mut names);
            for name in names {
                let import = program.interner.resolve(name);
                let own = self.modules.iter().any(|(m, _)| *m == import);
                if !own && !self.requires.contains(&import) {
                    undeclared.push(format!("{module} imports {import}"));
                }
            }
        }
        Ok(undeclared)
    }

    /// What `extensions` add to the compiler.
    pub fn compile_surface(extensions: &[&Extension]) -> ExtensionSurface {
        let mut surface = ExtensionSurface::default();
        for extension in extensions {
            surface.types.extend_from_slice(extension.types);
            surface.builtins.extend(
                extension
                    .builtins
                    .iter()
                    .map(|builtin| (builtin.name, builtin.signature)),
            );
            surface.modules.extend_from_slice(extension.modules);
            surface.requires.push(ExtensionRequires {
                extension: extension.name,
                modules: extension.modules,
                imports: extension.requires,
            });
        }
        surface
    }
}

impl fmt::Debug for Extension {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Extension")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

/// An [`AsyncJob`] inside an I/O request. It runs once; the request may be
/// cloned, but only one clone gets the job.
#[derive(Clone)]
pub struct ExtensionJob {
    pub builtin: Arc<str>,
    job: Arc<Mutex<Option<AsyncJob>>>,
}

impl ExtensionJob {
    pub fn new(builtin: Arc<str>, job: AsyncJob) -> Self {
        Self {
            builtin,
            job: Arc::new(Mutex::new(Some(job))),
        }
    }

    pub fn run(&self) -> Result<SendableValue, String> {
        let job = self
            .job
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
            .ok_or_else(|| format!("{}: job already ran", self.builtin))?;
        job()
    }
}

impl fmt::Debug for ExtensionJob {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ExtensionJob({})", self.builtin)
    }
}
