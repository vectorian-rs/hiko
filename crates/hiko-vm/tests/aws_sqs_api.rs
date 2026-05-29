#![cfg(feature = "builtin-aws-sqs")]

use std::path::Path;
use std::sync::Arc;

use hiko_vm::builder::{AwsConfigPolicy, AwsSqsPolicy, VMBuilder};
use hiko_vm::io_backend::MockIoBackend;
use hiko_vm::threaded::ThreadedRuntime;

fn compile_file(path: &str) -> hiko_compile::chunk::CompiledProgram {
    let source = std::fs::read_to_string(path).unwrap_or_else(|_| panic!("cannot read {path}"));
    let tokens = hiko_syntax::lexer::Lexer::new(&source, 0)
        .tokenize()
        .expect("lex error");
    let program = hiko_syntax::parser::Parser::new(tokens)
        .parse_program()
        .expect("parse error");
    hiko_compile::compiler::Compiler::compile_file(program, Path::new(path))
        .expect("compile error")
        .0
}

#[test]
fn aws_sqs_public_api_runs_with_mock_io_backend() {
    let compiled = compile_file("../../tests/run/test_aws_sqs_api_runtime.hml");
    let vm = VMBuilder::new(compiled)
        .with_core()
        .with_aws_config(AwsConfigPolicy {
            allowed_sso_profiles: Vec::new(),
            allow_instance_profile: true,
        })
        .with_aws_sqs(AwsSqsPolicy {
            allow_list_queues: true,
        })
        .build();
    let runtime = ThreadedRuntime::new(1).with_io_backend(Arc::new(MockIoBackend::new()));
    let pid = runtime.spawn_root_vm(vm);
    runtime.run_to_completion().expect("runtime failed");
    assert_eq!(runtime.failure(pid), None);
}
