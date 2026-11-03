//! The policy and the host-resource accounting, without AWS credentials or
//! the network: nothing here reaches a listing.

use hiko_compile::chunk::CompiledProgram;
use hiko_compile::compiler::{Compiler, ModuleLoaderOptions};
use hiko_ext_aws::AWS;
use hiko_vm::config::RunConfig;
use hiko_vm::extension::Extension;

const LINKED: &[&Extension] = &[&AWS];

fn repo() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn parse(source: &str) -> hiko_syntax::ast::Program {
    let tokens = hiko_syntax::lexer::Lexer::new(source, 0)
        .tokenize()
        .expect("lex");
    hiko_syntax::parser::Parser::new(tokens)
        .parse_program()
        .expect("parse")
}

/// The extension surface `config` turns on, and the repository's packages
/// without the module server or the user's cache.
fn loader_options(config: &RunConfig) -> ModuleLoaderOptions {
    let configured = config.configured_extensions(LINKED).expect("linked");
    ModuleLoaderOptions {
        extensions: Extension::compile_surface(&configured),
        local_package_root: Some(repo().join("libraries")),
        // One directory for every run, so runs do not leave new ones.
        cache_dir: Some(std::env::temp_dir().join("hiko-ext-aws-test-cache")),
        ..Default::default()
    }
}

/// Compiles `source` as a file at the repository root, so its imports
/// resolve through the repository's project, as a script's would: the AWS
/// modules import Std.Result.
fn compile(source: &str, config: &RunConfig) -> CompiledProgram {
    Compiler::compile_file_with_options(
        parse(source),
        &repo().join("script.hml"),
        loader_options(config),
    )
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

#[test]
fn the_sre_cloudwatch_example_type_checks() {
    let path = repo().join("examples/sre/cloudwatch_alarms.hml");
    let source = std::fs::read_to_string(&path).expect("example");
    let config = RunConfig::from_toml("[extensions.aws]\ncloudwatch = true\n").expect("config");
    Compiler::compile_file_with_options(parse(&source), &path, loader_options(&config))
        .unwrap_or_else(|e| panic!("{e:?}"));
}
