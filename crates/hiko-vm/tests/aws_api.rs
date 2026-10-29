#![cfg(feature = "builtin-aws-s3")]

mod common;

use std::sync::Arc;

use common::compile_file_hermetic as compile_file;
use hiko_vm::builder::{AwsConfigPolicy, AwsS3Policy, VMBuilder};
use hiko_vm::io_backend::MockIoBackend;
use hiko_vm::threaded::ThreadedRuntime;

#[test]
fn aws_public_api_shape_compiles() {
    let _ = compile_file("../../tests/run/test_aws_api.hml");
}

#[test]
fn aws_public_api_runs_with_mock_io_backend() {
    let compiled = compile_file("../../tests/run/test_aws_api_runtime.hml");
    let vm = VMBuilder::new(compiled)
        .with_core()
        .with_aws_config(AwsConfigPolicy {
            allowed_sso_profiles: Vec::new(),
            allow_instance_profile: true,
        })
        .with_aws_s3(AwsS3Policy {
            allow_list_buckets: true,
        })
        .build();
    let runtime = ThreadedRuntime::new(1).with_io_backend(Arc::new(MockIoBackend::new()));
    let pid = runtime.spawn_root_vm(vm);
    runtime.run_to_completion().expect("runtime failed");
    assert_eq!(runtime.failure(pid), None);
}

#[test]
fn instance_profile_requires_policy() {
    let compiled = compile_file("../../tests/run/test_aws_api_runtime.hml");
    let vm = VMBuilder::new(compiled)
        .with_core()
        .with_aws_config(AwsConfigPolicy {
            allowed_sso_profiles: Vec::new(),
            allow_instance_profile: false,
        })
        .with_aws_s3(AwsS3Policy {
            allow_list_buckets: true,
        })
        .build();
    let runtime = ThreadedRuntime::new(1).with_io_backend(Arc::new(MockIoBackend::new()));
    let pid = runtime.spawn_root_vm(vm);
    runtime.run_to_completion().expect("runtime failed");
    let failure = runtime.failure(pid).expect("root should fail");
    assert!(
        failure
            .to_string()
            .contains("AWS instance profile auth is not allowed"),
        "{failure:?}"
    );
}
