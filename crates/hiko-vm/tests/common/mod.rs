//! Shared helpers for hiko-vm integration tests.
//!
//! Named imports resolve from the repository's `libraries/` directory instead
//! of the module server in `hiko.lock.toml`, and any cache writes go to a
//! per-process temporary directory instead of `~/.hiko/lib-cache`. Modules
//! are still verified against the lockfile BLAKE3 hashes. Mirrors
//! `src/test_support.rs`, which unit tests use.

use std::path::Path;

use hiko_compile::chunk::CompiledProgram;
use hiko_compile::compiler::{Compiler, ModuleLoaderOptions};

pub fn hermetic_loader_options() -> ModuleLoaderOptions {
    ModuleLoaderOptions {
        cache_dir: Some(
            std::env::temp_dir().join(format!("hiko-vm-test-lib-cache-{}", std::process::id())),
        ),
        local_package_root: Some(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../libraries")),
    }
}

/// Compile a `.hml` file (path relative to the crate directory) without the
/// network or the user's module cache. Warnings are printed to stderr.
pub fn compile_file_hermetic(path: &str) -> CompiledProgram {
    let source = std::fs::read_to_string(path).unwrap_or_else(|_| panic!("cannot read {path}"));
    let tokens = hiko_syntax::lexer::Lexer::new(&source, 0)
        .tokenize()
        .expect("lex error");
    let program = hiko_syntax::parser::Parser::new(tokens)
        .parse_program()
        .expect("parse error");
    let (compiled, warnings) =
        Compiler::compile_file_with_options(program, Path::new(path), hermetic_loader_options())
            .expect("compile error");
    for w in &warnings {
        eprintln!("Warning: {}", w.message);
    }
    compiled
}
