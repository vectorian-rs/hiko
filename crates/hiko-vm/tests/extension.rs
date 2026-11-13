//! The extension API, through a small test extension: a sync builtin that
//! reads configured state, a host resource, an async builtin, a module, and
//! child processes that inherit all of it.

mod common;

use common::Captured;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use hiko_builtin_meta::BuiltinTypeSignature;
use hiko_compile::chunk::CompiledProgram;
use hiko_compile::compiler::{CompileError, Compiler, ModuleLoaderOptions};
use hiko_vm::config::RunConfig;
use hiko_vm::extension::{AsyncJob, Extension, ExtensionBuiltin, ExtensionFn, ExtensionState};
use hiko_vm::heap::Heap;
use hiko_vm::sendable::SendableValue;
use hiko_vm::threaded::ThreadedRuntime;
use hiko_vm::value::{HeapObject, Value};

struct Greeting {
    prefix: String,
}

const COUNTER: &str = "test_counter";

fn configure(table: &toml::Value) -> Result<ExtensionState, String> {
    let prefix = table
        .get("prefix")
        .and_then(toml::Value::as_str)
        .ok_or("expected a string `prefix`")?;
    Ok(Arc::new(Greeting {
        prefix: prefix.to_string(),
    }))
}

fn string_arg(args: &[Value], heap: &Heap) -> Result<String, String> {
    match args[0] {
        Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => Ok(s.to_string()),
            _ => Err("expected a string".into()),
        },
        _ => Err("expected a string".into()),
    }
}

fn greet(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let name = string_arg(args, heap)?;
    let state = heap
        .extension_state::<Greeting>("test")
        .ok_or("test extension is not configured")?;
    let text = format!("{}{name}", state.prefix);
    heap.alloc(HeapObject::String(text.into()))
        .map(Value::Heap)
        .map_err(|e| e.to_string())
}

fn counter_new(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let start = match args[0] {
        Value::Int(n) => n,
        _ => return Err("expected an int".into()),
    };
    heap.alloc_extension_resource(COUNTER, Arc::new(AtomicI64::new(start)))
}

fn counter_next(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let counter = heap.extension_resource::<AtomicI64>(args[0], COUNTER, "counter_next")?;
    Ok(Value::Int(counter.fetch_add(1, Ordering::SeqCst)))
}

/// Uppercases its argument on the I/O pool.
fn shout(args: &[Value], heap: &mut Heap) -> Result<AsyncJob, String> {
    let text = string_arg(args, heap)?;
    Ok(Box::new(move || {
        Ok(SendableValue::String(text.to_uppercase().into()))
    }))
}

const fn sig(ty: &'static str) -> BuiltinTypeSignature {
    BuiltinTypeSignature { vars: &[], ty }
}

static TEST: Extension = Extension {
    name: "test",
    types: &[COUNTER],
    builtins: &[
        ExtensionBuiltin {
            name: "test_greet",
            signature: sig("string -> string"),
            func: ExtensionFn::Sync(greet),
        },
        ExtensionBuiltin {
            name: "test_counter_new",
            signature: sig("int -> test_counter"),
            func: ExtensionFn::Sync(counter_new),
        },
        ExtensionBuiltin {
            name: "test_counter_next",
            signature: sig("test_counter -> int"),
            func: ExtensionFn::Sync(counter_next),
        },
        ExtensionBuiltin {
            name: "test_shout",
            signature: sig("string -> string"),
            func: ExtensionFn::Async(shout),
        },
    ],
    modules: &[(
        "Test.Greet",
        "structure Greet = struct\n  fun hello name = test_greet name\nend\n",
    )],
    requires: &[],
    configure,
};

const LINKED: &[&Extension] = &[&TEST];

fn parse(source: &str) -> hiko_syntax::ast::Program {
    let tokens = hiko_syntax::lexer::Lexer::new(source, 0)
        .tokenize()
        .expect("lex");
    hiko_syntax::parser::Parser::new(tokens)
        .parse_program()
        .expect("parse")
}

fn compile(source: &str, config: &RunConfig) -> Result<CompiledProgram, CompileError> {
    let configured = config.configured_extensions(LINKED).expect("linked");
    let options = ModuleLoaderOptions {
        extensions: Extension::compile_surface(&configured),
        ..Default::default()
    };
    Compiler::compile_with_options(parse(source), options).map(|(program, _)| program)
}

fn config(extra: &str) -> RunConfig {
    RunConfig::from_toml(&format!(
        "[capabilities.stdio.println]\nenabled = true\n[capabilities.int.to_string]\nenabled = true\n{extra}"
    ))
    .expect("config")
}

const CONFIGURED: &str = "[extensions.test]\nprefix = \"hi \"\n";

#[test]
fn sync_builtin_module_and_host_resource() {
    let config = config(CONFIGURED);
    let program = compile(
        r#"import Test.Greet
val _ = println (Greet.hello "ada")
val c = test_counter_new 41
val _ = test_counter_next c
val _ = println (int_to_string (test_counter_next c))
val _ = println (test_shout "quiet")"#,
        &config,
    )
    .expect("compile");
    let mut vm = config
        .build_vm_with_extensions(program, LINKED)
        .expect("build");
    vm.enable_output_capture();
    vm.run().expect("run");
    assert_eq!(vm.get_output(), ["hi ada\n", "42\n", "QUIET\n"]);
}

#[test]
fn unconfigured_extension_is_unknown_to_the_compiler() {
    let config = config("");
    let err = compile(r#"val _ = test_greet "ada""#, &config).unwrap_err();
    assert!(
        format!("{err:?}").contains("unbound variable: test_greet"),
        "{err:?}"
    );
    let err = compile("import Test.Greet\nval x = 1", &config).unwrap_err();
    assert!(format!("{err:?}").contains("Test"), "{err:?}");
}

#[test]
fn table_for_an_extension_not_in_the_build_is_an_error() {
    let config = config("[extensions.missing]\n");
    let err = config.configured_extensions(LINKED).unwrap_err();
    assert!(
        err.contains("extension 'missing' is not in this build"),
        "{err}"
    );
}

#[test]
fn configure_error_stops_the_build() {
    let config = config("[extensions.test]\nprefix = 3\n");
    let program = compile("val x = 1", &config).expect("compile");
    let err = config
        .build_vm_with_extensions(program, LINKED)
        .err()
        .expect("configure must fail");
    assert!(
        err.contains("extensions.test: expected a string `prefix`"),
        "{err}"
    );
}

#[test]
fn extension_host_resources_count_against_their_kind_limit() {
    let config = config(&format!(
        "{CONFIGURED}[limits.host_resources]\ntest_counter = 1\n"
    ));
    let program = compile(
        "val a = test_counter_new 1\nval b = test_counter_new 2",
        &config,
    )
    .expect("compile");
    let mut vm = config
        .build_vm_with_extensions(program, LINKED)
        .expect("build");
    let err = vm.run().unwrap_err();
    assert!(err.message.contains("test_counter"), "{}", err.message);
}

#[test]
fn async_builtin_and_state_in_a_child_under_the_runtime() {
    let config = config(&format!(
        "{CONFIGURED}[capabilities.fiber.spawn]\nenabled = true\n\
         [capabilities.fiber.join_or_fail]\nenabled = true\n[capabilities.fiber.join]\nenabled = true\n"
    ));
    let program = compile(
        r#"val child = spawn (fn () => test_shout (test_greet "bob"))
val _ = println (await_process child)"#,
        &config,
    )
    .expect("compile");
    let mut vm = config
        .build_vm_with_extensions(program, LINKED)
        .expect("build");
    let sink = Arc::new(Captured::default());
    vm.set_output_sink(sink.clone());
    let runtime = ThreadedRuntime::new(2);
    let pid = runtime.spawn_root_vm(vm);
    runtime.run_to_completion().expect("run");
    assert!(runtime.failure(pid).is_none(), "{:?}", runtime.failure(pid));
    assert_eq!(*sink.0.lock().unwrap(), "HI BOB\n");
}

#[test]
fn an_import_missing_from_requires_is_an_error_naming_the_module() {
    // A module that imports a library module its extension does not list.
    static UNDECLARED: Extension = Extension {
        name: "undeclared",
        types: &[],
        builtins: &[],
        modules: &[(
            "Undeclared.M",
            "import Std.List\nstructure M = struct\n  val n = 1\nend\n",
        )],
        requires: &[],
        configure,
    };
    assert_eq!(
        UNDECLARED.undeclared_imports(),
        Ok(vec!["Undeclared.M imports Std.List".to_string()])
    );
    let config = RunConfig::from_toml("[extensions.undeclared]\n").expect("config");
    let configured = config
        .configured_extensions(&[&UNDECLARED])
        .expect("linked");
    let options = ModuleLoaderOptions {
        extensions: Extension::compile_surface(&configured),
        ..Default::default()
    };
    let err = Compiler::compile_with_options(parse("import Undeclared.M"), options)
        .map(|_| ())
        .expect_err("undeclared import");
    assert_eq!(
        format!("{err:?}"),
        format!(
            "Codegen({:?})",
            "module Undeclared.M of extension 'undeclared' imports Std.List, which the extension does not list in its requires; add it to requires in the extension's Extension"
        )
    );
}

#[test]
fn undeclared_imports_finds_an_import_inside_a_let() {
    static IN_LET: Extension = Extension {
        name: "in_let",
        types: &[],
        builtins: &[],
        modules: &[("InLet.M", "val n = let import Std.List in 1 end\n")],
        requires: &[],
        configure,
    };
    assert_eq!(
        IN_LET.undeclared_imports(),
        Ok(vec!["InLet.M imports Std.List".to_string()])
    );
}

#[test]
fn an_extension_module_cannot_use_a_file() {
    static USES_FILE: Extension = Extension {
        name: "uses_file",
        types: &[],
        builtins: &[],
        modules: &[(
            "UsesFile.M",
            "use \"./helper.hml\"\nstructure M = struct\n  val n = 1\nend\n",
        )],
        requires: &[],
        configure,
    };
    assert_eq!(
        USES_FILE.undeclared_imports(),
        Ok(vec![
            "UsesFile.M uses ./helper.hml; an extension module cannot use a file".to_string()
        ])
    );
    let options = ModuleLoaderOptions {
        extensions: Extension::compile_surface(&[&USES_FILE]),
        ..Default::default()
    };
    let err = Compiler::compile_with_options(parse("import UsesFile.M"), options)
        .map(|_| ())
        .expect_err("a use in an extension module");
    assert_eq!(
        format!("{err:?}"),
        format!(
            "Codegen({:?})",
            "parse error in 'UsesFile.M': use was removed; use import Proj.<Module> for local modules under src/"
        )
    );
}
