//! Embedded and extension transports enforce the same per-owner import authority.
#![cfg(feature = "loader-embedded")]

use hiko_compile::compiler::{
    Compiler, EmbeddedModule, EmbeddedSources, ExtensionSurface, ModuleLoaderOptions,
};
use hiko_syntax::{lexer::Lexer, parser::Parser};

fn leak(value: String) -> &'static str {
    Box::leak(value.into_boxed_str())
}

fn options(dependencies: &'static [&'static str], target: &'static str) -> ModuleLoaderOptions {
    let source = leak(format!(
        "import {target}.Answer\nstructure Entry = struct val value = Answer.value end"
    ));
    let answer = "structure Answer = struct val value = 42 end";
    let mut modules = vec![EmbeddedModule {
        import_name: "Acme.Entry",
        version: "0.1.0",
        base_url: "https://example.com/Acme-v0.1.0",
        dependencies,
        blake3: leak(hiko_common::blake3_hex(source.as_bytes())),
        source,
    }];
    let mut extensions = ExtensionSurface::default();
    if target == "Host" {
        extensions.modules.push(("Host.Answer", answer));
    } else {
        modules.push(EmbeddedModule {
            import_name: "Other.Answer",
            version: "2.0.0",
            base_url: "https://example.com/Other-v2.0.0",
            dependencies: &[],
            blake3: leak(hiko_common::blake3_hex(answer.as_bytes())),
            source: answer,
        });
    }
    ModuleLoaderOptions {
        embedded: Some(Box::leak(Box::new(EmbeddedSources {
            modules: Box::leak(modules.into_boxed_slice()),
        }))),
        extensions,
        ..Default::default()
    }
}

fn compile(options: ModuleLoaderOptions) -> Result<(), String> {
    let source = "import Acme.Entry\nval result = Entry.value + 1";
    let program = Parser::new(Lexer::new(source, 0).tokenize().unwrap())
        .parse_program()
        .unwrap();
    Compiler::compile_embedded(program, options)
        .map(|_| ())
        .map_err(|e| format!("{e:?}"))
}

#[test]
fn embedded_libraries_do_not_inherit_the_artifacts_other_packages() {
    let error = compile(options(&[], "Other")).unwrap_err();
    assert!(
        error.contains("package Acme cannot import Other.Answer: declare Other as its dependency"),
        "{error}"
    );
    compile(options(&["Other"], "Other")).unwrap();
}

#[test]
fn importing_a_linked_extension_also_needs_library_authority() {
    let error = compile(options(&[], "Host")).unwrap_err();
    assert!(
        error.contains("package Acme cannot import Host.Answer: declare Host as its dependency"),
        "{error}"
    );
    compile(options(&["Host"], "Host")).unwrap();
}

#[test]
fn embedded_packages_cannot_shadow_extensions() {
    let mut options = options(&["Other"], "Other");
    options.extensions.modules.push((
        "Other.Answer",
        "structure Answer = struct val value = 0 end",
    ));
    let error = compile(options).unwrap_err();
    assert!(error.contains("collides"), "{error}");
}

#[test]
fn conflicting_embedded_package_metadata_is_rejected() {
    let mut options = options(&["Other"], "Other");
    let old = options.embedded.unwrap();
    let mut modules: Vec<_> = old
        .modules
        .iter()
        .map(|m| EmbeddedModule {
            import_name: m.import_name,
            version: m.version,
            base_url: m.base_url,
            dependencies: m.dependencies,
            blake3: m.blake3,
            source: m.source,
        })
        .collect();
    modules.push(EmbeddedModule {
        import_name: "Acme.Extra",
        version: "9.0.0",
        base_url: "https://example.com/Acme-v9.0.0",
        dependencies: &[],
        blake3: "unused",
        source: "",
    });
    options.embedded = Some(Box::leak(Box::new(EmbeddedSources {
        modules: Box::leak(modules.into_boxed_slice()),
    })));
    let error = compile(options).unwrap_err();
    assert!(
        error.contains("conflicting owner/dependency metadata"),
        "{error}"
    );
}
