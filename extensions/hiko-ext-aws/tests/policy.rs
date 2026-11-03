//! The policy and the host-resource accounting, without AWS credentials or
//! the network: nothing here reaches a listing.

use hiko_compile::chunk::CompiledProgram;
use hiko_compile::compiler::{Compiler, ModuleLoaderOptions};
use hiko_ext_aws::AWS;
use hiko_vm::config::RunConfig;
use hiko_vm::extension::Extension;

const LINKED: &[&Extension] = &[&AWS];

fn compile(source: &str, config: &RunConfig) -> CompiledProgram {
    let tokens = hiko_syntax::lexer::Lexer::new(source, 0)
        .tokenize()
        .expect("lex");
    let program = hiko_syntax::parser::Parser::new(tokens)
        .parse_program()
        .expect("parse");
    let configured = config.configured_extensions(LINKED).expect("linked");
    let options = ModuleLoaderOptions {
        extensions: Extension::compile_surface(&configured),
        ..Default::default()
    };
    Compiler::compile_with_options(program, options)
        .expect("compile")
        .0
}

fn run(source: &str, policy: &str) -> Result<(), String> {
    let config = RunConfig::from_toml(policy).expect("config");
    let program = compile(source, &config);
    let mut vm = config
        .build_vm_with_extensions(program, LINKED)
        .expect("build");
    vm.run().map_err(|e| e.message)
}

#[test]
fn an_sso_profile_outside_the_policy_is_refused() {
    let err = run(
        "import Aws.Config\nval c = Config.sso_profile \"prod\"",
        "[extensions.aws]\nsso_profiles = [\"dev\"]\n",
    )
    .unwrap_err();
    assert!(
        err.contains("profile 'prod' is not in sso_profiles"),
        "{err}"
    );
}

#[test]
fn there_is_no_instance_profile() {
    // Credentials come only from SSO profiles; the old key is refused.
    let table: toml::Value = toml::from_str("instance_profile = true").unwrap();
    let err = (AWS.configure)(&table).map(|_| ()).unwrap_err();
    assert!(err.contains("instance_profile"), "{err}");
}

#[test]
fn a_disabled_service_cannot_make_clients() {
    let err = run(
        "import Aws.Config\nimport Aws.SQS\nval q = SQS.client (Config.sso_profile \"dev\")",
        "[extensions.aws]\nsso_profiles = [\"dev\"]\ns3 = true\n",
    )
    .unwrap_err();
    assert!(
        err.contains("aws_sqs_client: the service is not enabled"),
        "{err}"
    );
}

#[test]
fn allowed_configs_and_clients_are_created_without_io() {
    run(
        "import Aws.Config\nimport Aws.S3\nval s3 = S3.client (Config.sso_profile \"dev\")",
        "[extensions.aws]\nsso_profiles = [\"dev\"]\ns3 = true\n",
    )
    .expect("no I/O happens before a listing");
}

#[test]
fn configs_count_against_their_host_resource_limit() {
    let err = run(
        "import Aws.Config\nval a = Config.sso_profile \"dev\"\nval b = Config.sso_profile \"dev\"",
        "[limits.host_resources]\naws_config = 1\n[extensions.aws]\nsso_profiles = [\"dev\"]\n",
    )
    .unwrap_err();
    assert!(err.contains("aws_config"), "{err}");
}

#[test]
fn cloudwatch_is_off_by_default() {
    let err = run(
        "import Aws.Config\nimport Aws.CloudWatch\nval c = CloudWatch.client (Config.sso_profile \"dev\")",
        "[extensions.aws]\nsso_profiles = [\"dev\"]\n",
    )
    .unwrap_err();
    assert!(
        err.contains("aws_cloudwatch_client: the service is not enabled"),
        "{err}"
    );
}

#[test]
fn an_enabled_cloudwatch_client_is_created_without_io() {
    run(
        "import Aws.Config\nimport Aws.CloudWatch\nval c = CloudWatch.client (Config.sso_profile \"dev\")",
        "[extensions.aws]\nsso_profiles = [\"dev\"]\ncloudwatch = true\n",
    )
    .expect("no I/O happens before a listing");
}
