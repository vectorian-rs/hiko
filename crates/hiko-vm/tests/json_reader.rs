#![cfg(feature = "builtin-json-reader")]
mod common;
use hiko_compile::{
    chunk::CompiledProgram,
    compiler::{CompileError, Compiler},
};
use hiko_syntax::{lexer::Lexer, parser::Parser};
use hiko_vm::{config::RunConfig, value::Value, vm::VM};
use std::path::Path;

fn compile(source: &str) -> Result<CompiledProgram, CompileError> {
    let p = Parser::new(Lexer::new(source, 0).tokenize().unwrap())
        .parse_program()
        .unwrap();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("inline.hml");
    Compiler::compile_file_with_options(p, &path, common::hermetic_loader_options()).map(|(p, _)| p)
}
fn configured(source: &str) -> VM {
    let config = RunConfig::from_toml("[capabilities]\njson_reader = true\n[limits]\nmax_fuel = 1000000000\nmax_host_work = 1000000000\nmax_host_resources = 32\n").unwrap();
    config.build_vm(compile(source).unwrap())
}

#[test]
fn opaque_type_cannot_be_forged_or_shadowed_and_raw_alias_is_private() {
    for declaration in [
        "val bad : JsonReader.reader = 7",
        "val bad = JsonReader.kind 7",
        "structure BuiltinJsonReader = struct datatype reader = Fake end",
        "val bad = json_reader_of_string \"null\"",
    ] {
        assert!(
            compile(&format!("import Std.JsonReader\n{declaration}")).is_err(),
            "{declaration}"
        );
    }
}

#[test]
fn cancellation_of_an_open_owner_refunds_only_at_teardown() {
    use hiko_vm::vm::RunResult;
    let mut vm = configured(
        "import Std.JsonReader\nval r = JsonReader.of_string \"[1,2]\"\nfun loop () = let val _ = JsonReader.kind r in loop () end\nval _ = loop ()",
    );
    vm.set_max_memory_bytes(4_000_000);
    let budget = vm.heap().memory_budget().unwrap().clone();
    let mut opened = false;
    for _ in 0..1000 {
        assert!(matches!(vm.run_slice(1), RunResult::Yielded));
        if vm.heap().host_resource_count() == 1 {
            opened = true;
            break;
        }
    }
    assert!(opened);
    assert!(budget.used() > 0);
    vm.request_cancellation();
    assert!(matches!(vm.run_slice(1), RunResult::Cancelled));
    assert_eq!(vm.heap().host_resource_count(), 1);
    drop(vm);
    assert_eq!(budget.used(), 0);
}

#[test]
#[cfg(feature = "builtin-process")]
fn forward_traversal_completes_on_both_runtimes() {
    use hiko_vm::{process::ProcessStatus, runtime::Runtime, threaded::ThreadedRuntime};
    let p = compile("import Std.JsonReader\nval r = JsonReader.of_string \"[10,20]\"\nval _ = JsonReader.begin_array r\nfun sum acc = if JsonReader.next_item r then sum (acc + JsonReader.read_int r) else acc\nval total = sum 0\nval _ = if total = 30 then JsonReader.finish r else JsonReader.read_null r").unwrap();
    let mut single = Runtime::new();
    let pid = single.spawn_root(p.clone());
    assert!(single.run_to_completion().unwrap().is_empty());
    assert!(matches!(single.get_status(pid), Some(ProcessStatus::Done)));
    for workers in [1, 4] {
        let threaded = ThreadedRuntime::new(workers);
        let pid = threaded.spawn_root(p.clone());
        assert!(threaded.run_to_completion().unwrap().is_empty());
        assert!(threaded.failure(pid).is_none());
    }
}

#[test]
fn single_grant_is_required_and_does_not_enable_legacy_json_or_peers() {
    let source =
        "import Std.JsonReader\nval r = JsonReader.of_string \"null\"\nval _ = JsonReader.finish r";
    let program = compile(source).unwrap();
    let denied = RunConfig::from_toml("").unwrap();
    let error = denied.build_vm(program.clone()).run().unwrap_err();
    assert!(error.message.contains("json_reader_of_string"), "{error:?}");
    let granted = RunConfig::from_toml("[capabilities]\njson_reader = true\n").unwrap();
    let names = granted.enabled_builtin_names();
    assert!(!names.contains("json_parse"));
    assert!(!names.contains("read_file"));
    for meta in hiko_builtin_meta::JSON_READER_BUILTINS {
        assert!(names.contains(meta.name));
    }
    granted.build_vm(program).run().unwrap();
    // The previous JSON profile does not grant the reader.
    assert!(
        !RunConfig::from_toml("[capabilities.json.parse]\nenabled = true\n")
            .unwrap()
            .enabled_builtin_names()
            .contains("json_reader_of_string")
    );
    assert!(RunConfig::from_toml("[capabilities.json_reader]\nenabled = true\n").is_err());
}

#[test]
fn field_option_null_empty_literals_duplicates_and_checked_scalars() {
    let mut vm = configured(
        r#"
import Std.JsonReader
import Std.Option
val r = JsonReader.of_string "{\"a\":1,\"a\":2,\"a.b\":null,\"\":\"\",\"b\":true,\"n\":1.00,\"f\":-0.0}"
val _ = JsonReader.begin_object r
val found_first = JsonReader.find_next_field "a" r
val first = JsonReader.read_int r
val found_second = JsonReader.find_next_field "a" r
val second = JsonReader.read_word r
val literal = case JsonReader.next_field r of Option.Some s => s | Option.None => "missing"
val null_kind = JsonReader.kind r
val _ = JsonReader.read_null r
val empty_key = case JsonReader.next_field r of Option.Some s => s | Option.None => "missing"
val empty_value = JsonReader.read_string r
val _ = JsonReader.next_field r
val boolean = JsonReader.read_bool r
val _ = JsonReader.next_field r
val raw = JsonReader.read_number_text r
val _ = JsonReader.next_field r
val floating = JsonReader.read_float r
val missing = JsonReader.find_next_field "a" r
val _ = JsonReader.finish_object r
val _ = JsonReader.finish r
val _ = JsonReader.close r
"#,
    );
    vm.run().unwrap();
    assert!(matches!(vm.get_global("first"), Some(Value::Int(1))));
    assert!(matches!(vm.get_global("second"), Some(Value::Word(2))));
    assert!(matches!(vm.get_global("boolean"), Some(Value::Bool(true))));
    assert!(matches!(vm.get_global("missing"), Some(Value::Bool(false))));
    let Some(Value::Float(f)) = vm.get_global("floating") else {
        panic!()
    };
    assert_eq!(f.to_bits(), (-0.0f64).to_bits());
    for (name, expected) in [
        ("literal", "a.b"),
        ("null_kind", "null"),
        ("empty_key", ""),
        ("empty_value", ""),
        ("raw", "1.00"),
    ] {
        let Some(Value::Heap(r)) = vm.get_global(name) else {
            panic!("{name}")
        };
        let hiko_vm::value::HeapObject::String(s) = vm.heap().get(*r).unwrap() else {
            panic!()
        };
        assert_eq!(&**s, expected);
    }
    assert_eq!(vm.heap().host_resource_count(), 0);
}

#[test]
fn protocol_and_numeric_failures_close_all_aliases() {
    for body in [
        "val _ = JsonReader.begin_array r\nval _ = JsonReader.next_item r\nval _ = JsonReader.next_item r",
        "val _ = JsonReader.begin_array r\nval _ = JsonReader.finish_object r",
        "val _ = JsonReader.skip r\nval _ = JsonReader.kind r",
        "val _ = JsonReader.finish r\nval _ = JsonReader.finish alias",
    ] {
        let mut vm = configured(&format!(
            "import Std.JsonReader\nval r = JsonReader.of_string \"[1,2]\"\nval alias = r\n{body}"
        ));
        assert!(vm.run().is_err(), "{body}");
        assert_eq!(vm.heap().host_resource_count(), 0);
    }
    for (number, method, kind) in [
        ("1.0", "read_int", "IntegerTokenRequired"),
        ("18446744073709551616", "read_word", "NumberRange"),
        ("1e-400", "read_float", "Underflow"),
    ] {
        let mut vm = configured(&format!(
            "import Std.JsonReader\nval r = JsonReader.of_string \"{number}\"\nval x = JsonReader.{method} r"
        ));
        let error = vm.run().unwrap_err();
        assert!(error.message.contains(kind), "{error:?}");
        assert_eq!(vm.heap().host_resource_count(), 0);
    }
}

#[test]
fn selected_reads_and_close_do_not_certify_bad_suffix_but_finish_does() {
    let mut vm = configured(
        "import Std.JsonReader\nval r = JsonReader.of_string \"1 false\"\nval earlier = JsonReader.read_int r\nval _ = JsonReader.finish r",
    );
    assert!(vm.run().unwrap_err().message.contains("byte 2"));
    assert!(matches!(vm.get_global("earlier"), Some(Value::Int(1))));
    let mut vm = configured(
        "import Std.JsonReader\nval r = JsonReader.of_string \"1 false\"\nval earlier = JsonReader.read_int r\nval _ = JsonReader.close r",
    );
    vm.run().unwrap();
}

#[test]
fn whole_fixture_growth_uses_original_billion_guards_and_one_input_tariff() {
    for n in [64, 128, 256] {
        let input = format!("[{}]", vec!["0"; n].join(","));
        let source = format!(
            "import Std.JsonReader\nval r = JsonReader.of_string \"{input}\"\nval _ = JsonReader.begin_array r\nfun sum acc = if JsonReader.next_item r then sum (acc + JsonReader.read_int r) else acc\nval total = sum 0\nval _ = JsonReader.finish r"
        );
        let mut vm = configured(&source);
        vm.run().unwrap();
        assert!(matches!(vm.get_global("total"), Some(Value::Int(0))));
        let expected = 45 + (2180 * input.len() as u64).div_ceil(1024) + 45 * (2 * n + 3) as u64;
        assert_eq!(vm.heap().host_work_used(), expected);
        let fuel = vm.fuel_budget().unwrap().used();
        assert!(fuel > 0 && fuel < 1_000_000_000);
        println!(
            "reader whole-fixture n={n} input={} fuel={fuel} host={expected} peak={}",
            input.len(),
            vm.heap_peak_bytes()
        );
    }
}

#[test]
#[cfg(feature = "builtin-process")]
fn direct_nested_captures_and_reader_results_are_rejected_by_both_runtimes() {
    use hiko_vm::{runtime::Runtime, threaded::ThreadedRuntime};
    let sources = [
        "val r = JsonReader.of_string \"null\"\nval p = Fiber.spawn (fn () => JsonReader.kind r)\nval _ = Fiber.join_or_fail p",
        "val r = JsonReader.of_string \"null\"\nval pair = (r, 3)\nval p = Fiber.spawn (fn () => let val (x, _) = pair in JsonReader.kind x end)\nval _ = Fiber.join_or_fail p",
        "val p = Fiber.spawn (fn () => [JsonReader.of_string \"null\"])\nval _ = Fiber.join_or_fail p",
    ];
    for body in sources {
        let p = compile(&format!("import Std.JsonReader\nimport Std.Fiber\n{body}")).unwrap();
        let mut single = Runtime::new();
        let pid = single.spawn_root(p.clone());
        let _ = single.run_to_completion();
        let status = format!("{:?}", single.get_status(pid));
        assert!(
            status.contains("cannot send host resource handles"),
            "{status}"
        );
        for workers in [1, 4] {
            let threaded = ThreadedRuntime::new(workers);
            let pid = threaded.spawn_root(p.clone());
            let _ = threaded.run_to_completion();
            let failure = threaded.failure(pid);
            assert!(failure.is_some());
            assert!(
                format!("{failure:?}").contains("cannot send host resource handles"),
                "{failure:?}"
            );
        }
    }
}
