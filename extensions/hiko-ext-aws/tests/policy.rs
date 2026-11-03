//! The policy and the host-resource accounting, without AWS credentials or
//! the network: nothing here reaches a listing.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use hiko_compile::chunk::CompiledProgram;
use hiko_compile::compiler::{Compiler, EmbeddedSources, ModuleLoaderOptions};
use hiko_ext_aws::AWS;
use hiko_vm::config::RunConfig;
use hiko_vm::extension::Extension;

const LINKED: &[&Extension] = &[&AWS];

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn parse(source: &str) -> hiko_syntax::ast::Program {
    let tokens = hiko_syntax::lexer::Lexer::new(source, 0)
        .tokenize()
        .expect("lex");
    hiko_syntax::parser::Parser::new(tokens)
        .parse_program()
        .expect("parse")
}

/// A new project directory whose lockfile pins `modules` of the
/// repository's Std package, with the hashes its `package.toml` lists. The
/// tests do not depend on the repository's own lockfile.
fn project(name: &str, modules: &[&str]) -> PathBuf {
    let package: toml::Table = toml::from_str(
        &std::fs::read_to_string(repo().join("libraries/Std-v0.1.0/package.toml")).unwrap(),
    )
    .unwrap();
    let dir = temp_dir(name);
    std::fs::write(
        dir.join("hiko.toml"),
        "[defaults]\nlockfile = \"hiko.lock.toml\"\n\n[registries.local]\nurl = \"http://127.0.0.1:8000\"\n\n[dependencies]\nStd = { version = \"0.1.0\", registry = \"local\" }\n",
    )
    .unwrap();
    let mut lock = "schema_version = 1\n\n[packages.Std]\nversion = \"0.1.0\"\nbase_url = \"http://127.0.0.1:8000/Std-v0.1.0\"\n\n[packages.Std.modules]\n".to_string();
    for module in modules {
        let hash = package["modules"][*module].as_str().unwrap();
        lock.push_str(&format!("{module} = \"{hash}\"\n"));
    }
    std::fs::write(dir.join("hiko.lock.toml"), lock).unwrap();
    dir
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("hiko-ext-aws-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The project most tests compile in: it pins Std.Result, which the AWS
/// modules import.
fn with_result() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| project("with-result", &["Result"]))
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

/// Compiles `source` as a script in `dir`, so its imports resolve through
/// the project `dir` belongs to, if any.
fn compile_in(
    dir: &Path,
    source: &str,
    options: ModuleLoaderOptions,
) -> Result<CompiledProgram, String> {
    Compiler::compile_file_with_options(parse(source), &dir.join("script.hml"), options)
        .map(|(program, _)| program)
        .map_err(|e| format!("{e:?}"))
}

fn run(source: &str, policy: &str) -> Result<(), String> {
    let config = RunConfig::from_toml(policy).expect("config");
    let program = compile_in(with_result(), source, loader_options(&config)).expect("compile");
    let mut vm = config
        .build_vm_with_extensions(program, LINKED)
        .expect("build");
    vm.run().map_err(|e| e.message)
}

const S3_POLICY: &str = "[extensions.aws]\nsso_profiles = [\"dev\"]\ns3 = true\n";

fn s3_options() -> ModuleLoaderOptions {
    loader_options(&RunConfig::from_toml(S3_POLICY).unwrap())
}

#[test]
fn aws_listings_are_std_results() {
    // The point of returning Std.Result: its functions apply directly.
    compile_in(
        with_result(),
        "import Aws.Config\nimport Aws.S3\nimport Std.Result\n\
         fun count c = Result.either (fn buckets => 1) (fn message => 0) (S3.list_buckets c)\n\
         val r : (S3.bucket list, string) Result.result = S3.list_buckets (S3.client (Config.sso_profile \"dev\"))",
        s3_options(),
    )
    .expect("an S3 listing is a Std.Result");
}

#[test]
fn a_lockfile_without_std_result_is_an_error_naming_the_extension() {
    // `requires` is one rule for all of the extension's modules, so this
    // holds for Aws.Config too, which itself imports nothing.
    let dir = project("without-result", &["List"]);
    for import in ["import Aws.S3", "import Aws.Config"] {
        let err = compile_in(&dir, import, s3_options()).unwrap_err();
        assert!(
            err.contains("extension 'aws' needs Std.Result, which hiko.lock.toml does not pin")
                && err.contains("hiko install"),
            "{import}: {err}"
        );
    }
}

#[test]
fn outside_a_project_the_error_names_the_extension() {
    let dir = temp_dir("no-project");
    let err = compile_in(&dir, "import Aws.S3", s3_options()).unwrap_err();
    assert!(
        err.contains("extension 'aws' needs Std.Result") && err.contains("hiko.toml"),
        "{err}"
    );
}

#[test]
fn a_sealed_artifact_without_std_result_is_an_error_naming_the_extension() {
    // What `build-vm --ext aws` without `--lock` embeds: no modules.
    static NOTHING: EmbeddedSources = EmbeddedSources {
        modules: &[],
        files: None,
    };
    let options = ModuleLoaderOptions {
        embedded: Some(&NOTHING),
        ..s3_options()
    };
    let err = compile_in(with_result(), "import Aws.S3", options).unwrap_err();
    assert!(
        err.contains("extension 'aws' needs Std.Result, which this artifact does not embed"),
        "{err}"
    );
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
    // The example is part of the repository's project (it imports Sre), so
    // it resolves through the repository's lockfile.
    Compiler::compile_file_with_options(parse(&source), &path, loader_options(&config))
        .unwrap_or_else(|e| panic!("{e:?}"));
}
