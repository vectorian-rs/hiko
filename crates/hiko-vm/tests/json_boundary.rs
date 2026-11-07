#![cfg(feature = "builtin-json")]

mod common;

use std::path::Path;

use hiko_compile::chunk::CompiledProgram;
use hiko_compile::compiler::{CompileError, Compiler};
use hiko_syntax::{lexer::Lexer, parser::Parser};
use hiko_vm::{config::RunConfig, threaded::ThreadedRuntime, vm::VM};

fn compile(source: &str) -> Result<CompiledProgram, CompileError> {
    let program = Parser::new(Lexer::new(source, 0).tokenize().unwrap())
        .parse_program()
        .unwrap();
    let entry = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/run/test_std_wrappers.hml");
    Compiler::compile_file_with_options(program, &entry, common::hermetic_loader_options()).map(
        |(program, warnings)| {
            assert!(warnings.is_empty(), "{warnings:?}");
            program
        },
    )
}

#[test]
fn public_json_surface_rejects_unrelated_types() {
    for declaration in [
        "val x : int = Json.parse \"7\"",
        "val x = Json.to_string (fn (x : int) => x)",
        "val x = Json.JArray [42]",
    ] {
        let error = compile(&format!("import Std.Json\n{declaration}")).unwrap_err();
        assert!(format!("{error:?}").contains("type mismatch"), "{error:?}");
    }
}

#[test]
fn json_constructor_aliases_round_trip_on_both_runtimes() {
    let program = compile(
        r#"
        import Std.Json
        fun describe value = case value of
            Json.JNull => "null"
          | Json.JBool b => if b then "true" else "false"
          | Json.JInt n => int_to_string n
          | Json.JFloat _ => "float"
          | Json.JStr s => s
          | Json.JArray _ => "array"
          | Json.JObject _ => "object"
        val values = [Json.JNull, Json.JBool true, Json.JInt 42, Json.JFloat 1.5,
                      Json.JStr "text", Json.JArray [Json.JInt 7],
                      Json.JObject [("nested", Json.JArray [Json.JBool false])]]
        fun check xs = case xs of [] => () | value :: rest =>
            let
                val encoded = Json.to_string value
                val decoded : Json.json = Json.parse encoded
                val _ = assert_eq (Json.to_string decoded, encoded, "JSON round trip")
                val _ = println (describe decoded)
            in check rest end
        val _ = check values
    "#,
    )
    .unwrap();
    let expected = [
        "null\n", "true\n", "42\n", "float\n", "text\n", "array\n", "object\n",
    ];
    let mut vm = VM::new(program.clone());
    vm.enable_output_capture();
    vm.run().unwrap();
    assert_eq!(vm.get_output(), expected);
    for workers in [1, 4] {
        let runtime = ThreadedRuntime::new(workers);
        runtime.spawn_root(program.clone());
        assert_eq!(runtime.run_to_completion().unwrap(), expected);
    }
}

#[test]
fn trusted_json_converters_ignore_and_restore_user_bindings() {
    let program = compile(
        r#"
        fun json_parse (_ : string) = 99
        fun json_to_string (_ : int) = "user serializer"
        import Std.Json
        val _ = assert_eq (Json.to_string (Json.parse "[7]"), "[7]", "trusted converters")
        val _ = assert_eq (json_parse "ignored", 99, "user parser survives import")
        val _ = assert_eq (json_to_string 1, "user serializer", "user serializer survives import")
        val _ = println "bindings preserved"
    "#,
    )
    .unwrap();
    let mut vm = VM::new(program.clone());
    vm.enable_output_capture();
    vm.run().unwrap();
    assert_eq!(vm.get_output(), ["bindings preserved\n"]);
    for workers in [1, 4] {
        let runtime = ThreadedRuntime::new(workers);
        runtime.spawn_root(program.clone());
        assert_eq!(
            runtime.run_to_completion().unwrap(),
            ["bindings preserved\n"]
        );
    }
}

#[test]
fn malformed_json_keeps_the_host_diagnostic() {
    let program = compile("import Std.Json\nval _ = Json.parse \"{bad\"").unwrap();
    let error = VM::new(program).run().unwrap_err();
    assert!(format!("{error:?}").contains("json_parse:"), "{error:?}");
}

#[test]
fn json_wrappers_cannot_enable_denied_converters() {
    let program = compile("import Std.Json\nval s = Json.to_string (Json.parse \"null\")").unwrap();
    let names = [
        "json_parse",
        "json_to_string",
        "json_get",
        "json_keys",
        "json_length",
    ];
    for denied in ["json_parse", "json_to_string"] {
        let config = names
            .iter()
            .filter(|&&name| name != denied)
            .map(|name| format!("[capabilities.json.{name}]\nenabled = true\n"))
            .collect::<String>();
        let config = RunConfig::from_toml(&config).unwrap();
        assert!(!config.enabled_builtin_names().contains(denied));
        let error = config.build_vm(program.clone()).run().unwrap_err();
        let message = format!("{error:?}");
        assert!(
            message.contains(denied) && message.contains("undefined global"),
            "{message}"
        );
    }
}
