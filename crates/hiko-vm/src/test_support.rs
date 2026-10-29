//! Helpers shared by the unit tests in this crate.
//!
//! Named imports (`import Std.List`) resolve from the repository's
//! `libraries/` directory instead of the module server in `hiko.lock.toml`,
//! and any cache writes go to a per-process temporary directory instead of
//! `~/.hiko/lib-cache`. Modules are still verified against the lockfile
//! BLAKE3 hashes. Integration tests in `tests/` have a copy of this in
//! `tests/common/mod.rs`.

use std::path::{Path, PathBuf};

use hiko_compile::chunk::CompiledProgram;
use hiko_compile::compiler::{Compiler, ModuleLoaderOptions};
use hiko_syntax::lexer::Lexer;
use hiko_syntax::parser::Parser;

/// The repository's `libraries/` directory.
pub(crate) fn repo_libraries_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../libraries")
}

/// Loader options that never touch the network or the user's module cache.
pub(crate) fn hermetic_loader_options() -> ModuleLoaderOptions {
    ModuleLoaderOptions {
        cache_dir: Some(
            std::env::temp_dir().join(format!("hiko-vm-test-lib-cache-{}", std::process::id())),
        ),
        local_package_root: Some(repo_libraries_dir()),
    }
}

/// Compile a `.hml` file with [`hermetic_loader_options`].
pub(crate) fn compile_file_hermetic(path: &Path) -> CompiledProgram {
    let source = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let tokens = Lexer::new(&source, 0).tokenize().expect("lex error");
    let program = Parser::new(tokens).parse_program().expect("parse error");
    let (compiled, _) =
        Compiler::compile_file_with_options(program, path, hermetic_loader_options())
            .unwrap_or_else(|e| panic!("cannot compile {}: {e:?}", path.display()));
    compiled
}
