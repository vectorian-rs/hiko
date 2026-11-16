//! `examples/log_plain.hml` and `examples/log_effect.hml` log the same
//! program two ways (Std.Log): they must print the same stdout and the same
//! stderr lines, apart from the time.
#![cfg(feature = "builtin-default")]

mod common;

use std::sync::Arc;

use common::Captured;
use hiko_vm::builder::VMBuilder;
use hiko_vm::config::RunConfig;

const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");

/// Run `example` under the repository's default policy: (stdout, stderr).
fn run(example: &str) -> (String, String) {
    let policy = std::fs::read_to_string(format!(
        "{ROOT}/policies/software-developer-role.policy.toml"
    ))
    .unwrap();
    run_with(example, &policy)
}

/// Run `example` under the run config `policy`: (stdout, stderr).
fn run_with(example: &str, policy: &str) -> (String, String) {
    let root = ROOT;
    let config = RunConfig::from_toml(policy).unwrap();
    let program = common::compile_file_hermetic(&format!("{root}/examples/{example}"));
    let mut vm = config.apply_to_builder(VMBuilder::new(program)).build();
    let (out, err) = (Arc::new(Captured::default()), Arc::new(Captured::default()));
    vm.set_output_sink(out.clone());
    vm.set_error_sink(err.clone());
    vm.run()
        .unwrap_or_else(|e| panic!("{example}: {}", e.message));
    let out = out.0.lock().unwrap().clone();
    let err = err.0.lock().unwrap().clone();
    (out, err)
}

/// A log line without its `YYYY-MM-DDTHH:MM:SSZ ` prefix, after checking
/// the prefix's shape.
fn without_time(line: &str) -> &str {
    let (Some(time), Some(rest)) = (line.get(..21), line.get(21..)) else {
        panic!("not a log line: {line:?}")
    };
    let shape: String = time
        .chars()
        .map(|c| if c.is_ascii_digit() { '0' } else { c })
        .collect();
    assert_eq!(shape, "0000-00-00T00:00:00Z ", "{line}");
    rest
}

#[test]
fn plain_and_effect_examples_log_the_same_lines() {
    let (plain_out, plain_err) = run("log_plain.hml");
    let (effect_out, effect_err) = run("log_effect.hml");

    assert_eq!(plain_out, "total: 8\n");
    assert_eq!(effect_out, plain_out);

    let plain: Vec<&str> = plain_err.lines().map(without_time).collect();
    let effect: Vec<&str> = effect_err.lines().map(without_time).collect();
    assert!(
        plain.contains(&"WARN  shop: skipping pears: quantity 0"),
        "{plain:?}"
    );
    assert!(plain.contains(&"INFO  shop: done total=8"), "{plain:?}");
    assert_eq!(effect, plain);
}

#[test]
fn eprint_needs_its_capability() {
    for builtin in ["eprint", "eprintln"] {
        let compiled = common::compile_source(&format!(
            "import __Builtin.Stdio\nval _ = BuiltinStdio.{builtin} \"x\""
        ));
        let build = |toml: &str| {
            let config = RunConfig::from_toml(toml).unwrap();
            let mut vm = config
                .apply_to_builder(VMBuilder::new(compiled.clone()))
                .build();
            let err = Arc::new(Captured::default());
            vm.set_error_sink(err.clone());
            (vm.run().map_err(|e| e.message), err)
        };

        let (result, _) = build("[capabilities.stdio.println]\nenabled = true\n");
        let message = result.expect_err("refused without its capability");
        assert!(message.contains(builtin), "{message}");

        let (result, err) = build(&format!("[capabilities.stdio.{builtin}]\nenabled = true\n"));
        result.unwrap();
        let expected = if builtin == "eprintln" { "x\n" } else { "x" };
        assert_eq!(*err.0.lock().unwrap(), expected);
    }
}

/// The capabilities Std.Log says it needs are enough (plus `println` for
/// the example's own output).
#[test]
fn std_log_needs_only_the_capabilities_it_lists() {
    let policy: String = [
        "stdio.print",
        "stdio.println",
        "stdio.eprint",
        "stdio.eprintln",
        "stdio.read_line",
        "stdio.read_stdin",
        "time.now",
        "int.to_string",
        "string.length",
        "string.join",
        "env.get",
        "string.lower",
        "string.trim",
        "string.substring",
        "string.contains",
        "string.starts_with",
        "string.ends_with",
        "string.split",
        "string.replace",
        "string.upper",
    ]
    .iter()
    .map(|leaf| format!("[capabilities.{leaf}]\nenabled = true\n"))
    .collect();
    for example in ["log_plain.hml", "log_effect.hml"] {
        let (out, err) = run_with(example, &policy);
        assert_eq!(out, "total: 8\n", "{example}");
        assert!(
            err.contains("WARN  shop: skipping pears"),
            "{example}: {err}"
        );
    }
}
