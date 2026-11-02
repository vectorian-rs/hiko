//! Write seed corpora for the fuzz targets from the repository's own files:
//! `.hml` sources for `fuzz_typecheck`, their compiled programs (encoded)
//! for `fuzz_verify`, and `policies/*.toml` for `fuzz_run_config`.
//!
//! Run from `crates/hiko-vm/fuzz`: `cargo run --bin seed_corpus`.

use std::fs;
use std::path::{Path, PathBuf};

use hiko_compile::compiler::{Compiler, ModuleLoaderOptions};
use hiko_syntax::lexer::Lexer;
use hiko_syntax::parser::Parser;

fn files(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            files(&path, ext, out);
        } else if path.extension().is_some_and(|e| e == ext) {
            out.push(path);
        }
    }
}

fn write(target: &str, name: &str, bytes: &[u8]) {
    let dir = Path::new("corpus").join(target);
    fs::create_dir_all(&dir).expect("create corpus dir");
    fs::write(dir.join(name), bytes).expect("write corpus file");
}

fn main() {
    let root = Path::new("../../..");
    let mut sources = Vec::new();
    for dir in ["tests", "examples", "benchmarks", "libraries"] {
        files(&root.join(dir), "hml", &mut sources);
    }
    let (mut typecheck, mut programs) = (0, 0);
    for path in &sources {
        let Ok(source) = fs::read_to_string(path) else {
            continue;
        };
        let name = path
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .replace('/', "_");
        write("fuzz_typecheck", &name, source.as_bytes());
        typecheck += 1;

        let Ok(tokens) = Lexer::new(&source, 0).tokenize() else {
            continue;
        };
        let Ok(program) = Parser::new(tokens).parse_program() else {
            continue;
        };
        // Named imports resolve from the repository's libraries/.
        let options = ModuleLoaderOptions {
            cache_dir: None,
            local_package_root: Some(root.join("libraries")),
        };
        if let Ok((compiled, _)) = Compiler::compile_file_with_options(program, path, options) {
            write("fuzz_verify", &name, &hiko_vm_fuzz::encode(&compiled));
            programs += 1;
        }
    }
    let mut policies = Vec::new();
    files(&root.join("policies"), "toml", &mut policies);
    files(&root.join("tests"), "toml", &mut policies);
    for path in &policies {
        let name = path
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .replace('/', "_");
        write("fuzz_run_config", &name, &fs::read(path).unwrap());
    }
    println!(
        "{typecheck} sources, {programs} compiled programs, {} configs",
        policies.len()
    );
}
