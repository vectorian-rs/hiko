#![cfg(feature = "builtin-aws-sqs")]

mod common;

use std::sync::Arc;

use common::compile_file_hermetic as compile_file;
use hiko_vm::builder::{AwsConfigPolicy, AwsSqsPolicy, VMBuilder};
use hiko_vm::io_backend::MockIoBackend;
use hiko_vm::threaded::ThreadedRuntime;

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
