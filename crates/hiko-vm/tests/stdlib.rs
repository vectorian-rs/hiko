mod common;

fn run_hiko_file(path: &str) {
    let compiled = common::compile_file_hermetic(path);
    let mut vm = hiko_vm::vm::VM::new(compiled);
    vm.run()
        .unwrap_or_else(|error| panic!("runtime error in {path}: {error:?}"));
}

#[test]
fn test_stdlib_list() {
    run_hiko_file("../../tests/run/test_list.hml");
}

#[test]
fn test_stdlib_stack_safe() {
    run_hiko_file("../../tests/run/test_stdlib_stack_safe.hml");
}

#[test]
fn test_stdlib_option() {
    run_hiko_file("../../tests/run/test_option.hml");
}

#[test]
fn test_stdlib_either() {
    run_hiko_file("../../tests/run/test_either.hml");
}

#[test]
fn test_stdlib_result() {
    run_hiko_file("../../tests/run/test_result.hml");
}

#[test]
fn test_stdlib_time() {
    run_hiko_file("../../tests/run/test_time.hml");
}

#[cfg(feature = "builtin-date")]
#[test]
fn test_stdlib_date() {
    run_hiko_file("../../tests/run/test_date.hml");
}

#[cfg(all(feature = "builtin-json", feature = "builtin-regex"))]
#[test]
fn test_stdlib_wrappers() {
    run_hiko_file("../../tests/run/test_std_wrappers.hml");
}

#[test]
fn test_numeric_module_examples() {
    run_hiko_file("../../examples/numeric_modules.hml");
}

#[test]
fn test_sre_health_and_report() {
    run_hiko_file("../../tests/run/test_sre.hml");
}

#[test]
fn test_stdlib_log() {
    run_hiko_file("../../tests/run/test_log.hml");
}

fn compile_thin_source(source: &str) -> Result<hiko_compile::chunk::CompiledProgram, String> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/run/test_thin_modules.hml");
    let tokens = hiko_syntax::lexer::Lexer::new(source, 0)
        .tokenize()
        .map_err(|e| format!("{e:?}"))?;
    let program = hiko_syntax::parser::Parser::new(tokens)
        .parse_program()
        .map_err(|e| format!("{e:?}"))?;
    hiko_compile::compiler::Compiler::compile_file_with_options(
        program,
        &path,
        common::hermetic_loader_options(),
    )
    .map(|(program, _)| program)
    .map_err(|e| format!("{e:?}"))
}

#[cfg(any(feature = "builtin-convert", feature = "builtin-path"))]
fn run_thin_source(source: &str, policy: &str) -> Result<(), String> {
    let program = compile_thin_source(source)?;
    let config = hiko_vm::config::RunConfig::from_toml(policy)?;
    config.build_vm(program).run().map_err(|e| e.message)
}

#[cfg(all(
    feature = "builtin-convert",
    feature = "builtin-path",
    feature = "builtin-system",
    feature = "builtin-testing"
))]
#[test]
fn test_stdlib_thin_modules() {
    run_hiko_file("../../tests/run/test_thin_modules.hml");
}

#[cfg(feature = "builtin-convert")]
#[test]
fn thin_conversions_need_only_the_invoked_policy_leaf() {
    let cases = [
        ("Int", "to_string 7", "int_to_string"),
        ("Int", "of_string \"7\"", "string_to_int"),
        ("Float", "of_int 7", "int_to_float"),
        ("Float", "to_string 7.0", "float_to_string"),
        ("Char", "of_int 65", "int_to_char"),
        ("Char", "to_int #\"A\"", "char_to_int"),
        ("Word", "of_int 7", "int_to_word"),
        ("Word", "to_int 0w7", "word_to_int"),
        ("Word", "of_string \"7\"", "string_to_word"),
        ("Word", "to_string 0w7", "word_to_string"),
    ];
    for (module, call, leaf) in cases {
        run_thin_source(&format!("import Std.{module}\nval x = 1"), "")
            .unwrap_or_else(|e| panic!("{module} import: {e}"));
        let source = format!("import Std.{module}\nval x = {module}.{call}");
        let policy = format!(
            "[capabilities.{}.{}]\nenabled = true\n",
            module.to_lowercase(),
            call.split_whitespace().next().unwrap()
        );
        run_thin_source(&source, &policy).unwrap_or_else(|e| panic!("{leaf}: {e}"));
        let denied = run_thin_source(&source, "").unwrap_err();
        assert!(
            denied.contains(&format!("undefined global: {leaf}")),
            "{denied}"
        );
    }
}

#[cfg(feature = "builtin-convert")]
#[test]
fn thin_conversion_errors_match_raw_errors() {
    let policy = [
        "char.of_int",
        "word.of_int",
        "word.to_int",
        "int.of_string",
        "word.of_string",
    ]
    .map(|key| format!("[capabilities.{key}]\nenabled = true\n"))
    .join("");
    for (module, wrapper, raw) in [
        ("Char", "Char.of_int (~1)", "int_to_char (~1)"),
        ("Char", "Char.of_int 55296", "int_to_char 55296"),
        ("Char", "Char.of_int 4294967361", "int_to_char 4294967361"),
        ("Word", "Word.of_int (~1)", "int_to_word (~1)"),
        (
            "Word",
            "Word.to_int 0w18446744073709551615",
            "word_to_int 0w18446744073709551615",
        ),
        (
            "Word",
            "Word.of_string \"invalid\"",
            "string_to_word \"invalid\"",
        ),
        (
            "Word",
            "Word.of_string \"18446744073709551616\"",
            "string_to_word \"18446744073709551616\"",
        ),
        ("Word", "Word.of_string \"   \"", "string_to_word \"   \""),
        ("Int", "Int.of_string \"   \"", "string_to_int \"   \""),
        (
            "Int",
            "Int.of_string \"invalid\"",
            "string_to_int \"invalid\"",
        ),
        (
            "Int",
            "Int.of_string \"9223372036854775808\"",
            "string_to_int \"9223372036854775808\"",
        ),
    ] {
        let wrapped = run_thin_source(&format!("import Std.{module}\nval x = {wrapper}"), &policy)
            .unwrap_err();
        let raw = run_thin_source(&format!("val x = {raw}"), &policy).unwrap_err();
        assert_eq!(wrapped, raw, "{wrapper}");
    }
}

#[cfg(feature = "builtin-convert")]
#[test]
fn existing_width_modules_keep_empty_policy_behavior() {
    for (module, expression) in [
        ("Int32", "Int32.to_int (Int32.of_int 3) = 3"),
        ("Word32", "Word32.to_int (Word32.of_int 3) = 3"),
        ("Float32", "Float32.to_float (Float32.of_float 3.0) = 3.0"),
    ] {
        let source = format!(
            "import Std.{module}\nval x = if {expression} then () else panic \"wrong value\""
        );
        run_thin_source(&source, "").unwrap_or_else(|e| panic!("{module}: {e}"));
    }
}

#[cfg(feature = "builtin-path")]
#[test]
fn thin_path_import_defers_permission_until_call() {
    run_thin_source("import Std.Path\nval x = 1", "").unwrap();
    let source = "import Std.Path\nval x = Path.join (\"a\", \"b\")";
    let denied = run_thin_source(source, "").unwrap_err();
    assert!(denied.contains("undefined global: path_join"), "{denied}");
    run_thin_source(source, "[capabilities.path.join]\nenabled = true").unwrap();
}

#[test]
fn thin_modules_report_missing_cargo_features() {
    for (module, feature, enabled) in [
        ("Int", "builtin-convert", cfg!(feature = "builtin-convert")),
        (
            "Float",
            "builtin-convert",
            cfg!(feature = "builtin-convert"),
        ),
        ("Char", "builtin-convert", cfg!(feature = "builtin-convert")),
        ("Word", "builtin-convert", cfg!(feature = "builtin-convert")),
        ("Path", "builtin-path", cfg!(feature = "builtin-path")),
    ] {
        if !enabled {
            let error =
                compile_thin_source(&format!("import Std.{module}\nval x = 1")).unwrap_err();
            assert!(
                error.contains("not available in this build") && error.contains(feature),
                "{error}"
            );
        }
    }
    if !cfg!(feature = "builtin-system") || !cfg!(feature = "builtin-testing") {
        let missing = if !cfg!(feature = "builtin-system") {
            "builtin-system"
        } else {
            "builtin-testing"
        };
        let error = compile_thin_source("import Std.System\nval x = 1").unwrap_err();
        assert!(
            error.contains("not available in this build") && error.contains(missing),
            "{error}"
        );
    }
}
