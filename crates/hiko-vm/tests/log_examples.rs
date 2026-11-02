//! `examples/log_plain.hml` and `examples/log_effect.hml` log the same
//! program two ways (Std.Log): they must print the same stdout and the same
//! stderr lines, apart from the time.
#![cfg(feature = "builtin-default")]

mod common;

use std::sync::{Arc, Mutex};

use hiko_vm::builder::VMBuilder;
use hiko_vm::config::RunConfig;
use hiko_vm::vm::OutputSink;

#[derive(Default)]
struct Captured(Mutex<String>);

impl OutputSink for Captured {
    fn write(&self, text: &str) -> std::io::Result<()> {
        self.0.lock().unwrap().push_str(text);
        Ok(())
    }
}

/// Run `example` under the repository's default policy: (stdout, stderr).
fn run(example: &str) -> (String, String) {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
    let policy = std::fs::read_to_string(format!(
        "{root}/policies/software-developer-role.policy.toml"
    ))
    .unwrap();
    let config = RunConfig::from_toml(&policy).unwrap();
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
    let (time, rest) = line.split_at(21);
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
        let source = format!("val _ = {builtin} \"x\"");
        let tokens = hiko_syntax::lexer::Lexer::new(&source, 0)
            .tokenize()
            .unwrap();
        let program = hiko_syntax::parser::Parser::new(tokens)
            .parse_program()
            .unwrap();
        let compiled = hiko_compile::compiler::Compiler::compile(program)
            .unwrap()
            .0;
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
