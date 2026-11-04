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

/// The end of an error whose fix is to declare and install Std.
const DECLARE_STD: &str = "if hiko.toml does not declare Std, add it with its version and registry (see docs/language/imports.md), then run 'hiko install'";

/// How `compile_in` reports a loader error with this exact message.
fn codegen(message: &str) -> String {
    format!("Codegen({message:?})")
}

fn outside_a_project(script: &str) -> String {
    format!(
        "module Aws.S3 of extension 'aws' needs Std.Result, which comes from a project's lockfile, but {script} has no hiko.toml in or above its directory; create one that declares Std with its version and registry (see docs/language/imports.md), then run 'hiko install'"
    )
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
fn a_lockfile_without_std_result_is_an_error_naming_the_module() {
    let dir = project("without-result", &["List"]);
    let err = compile_in(&dir, "import Aws.S3", s3_options()).unwrap_err();
    assert_eq!(
        err,
        codegen(&format!(
            "module Aws.S3 of extension 'aws' needs Std.Result, which {} does not pin; {DECLARE_STD}",
            dir.join("hiko.lock.toml").display()
        ))
    );
    // Aws.Config imports nothing, so it needs nothing.
    compile_in(&dir, "import Aws.Config", s3_options()).expect("Aws.Config alone");
}

#[test]
fn the_error_names_the_lockfile_the_project_uses() {
    let dir = project("renamed-lockfile", &["List"]);
    let manifest = std::fs::read_to_string(dir.join("hiko.toml")).unwrap();
    std::fs::write(
        dir.join("hiko.toml"),
        manifest.replace("hiko.lock.toml", "release.lock.toml"),
    )
    .unwrap();
    std::fs::rename(dir.join("hiko.lock.toml"), dir.join("release.lock.toml")).unwrap();
    let err = compile_in(&dir, "import Aws.S3", s3_options()).unwrap_err();
    assert_eq!(
        err,
        codegen(&format!(
            "module Aws.S3 of extension 'aws' needs Std.Result, which {} does not pin; {DECLARE_STD}",
            dir.join("release.lock.toml").display()
        ))
    );
}

#[test]
fn outside_a_project_the_error_names_the_extension() {
    let dir = temp_dir("no-project");
    let err = compile_in(&dir, "import Aws.S3", s3_options()).unwrap_err();
    assert_eq!(
        err,
        codegen(&outside_a_project(
            &dir.join("script.hml").display().to_string()
        ))
    );
}

#[test]
fn with_no_entry_file_the_error_says_this_program() {
    let err = Compiler::compile_with_options(parse("import Aws.S3"), s3_options())
        .map(|_| ())
        .map_err(|e| format!("{e:?}"))
        .unwrap_err();
    assert_eq!(err, codegen(&outside_a_project("this program")));
}

#[test]
fn outside_a_project_aws_config_compiles() {
    let dir = temp_dir("config-no-project");
    compile_in(&dir, "import Aws.Config", s3_options()).expect("Aws.Config imports nothing");
}

#[test]
fn a_project_with_no_lockfile_ends_in_the_fix() {
    let dir = project("no-lockfile", &["Result"]);
    std::fs::remove_file(dir.join("hiko.lock.toml")).unwrap();
    let err = compile_in(&dir, "import Aws.S3", s3_options()).unwrap_err();
    assert_eq!(
        err,
        codegen(&format!(
            "module Aws.S3 of extension 'aws' needs Std.Result: project manifest '{}' references missing module lockfile '{}'; {DECLARE_STD}",
            dir.join("hiko.toml").display(),
            dir.join("hiko.lock.toml").display()
        ))
    );
}

#[test]
fn a_lockfile_that_lacks_a_declared_std_ends_in_the_fix() {
    let dir = project("lacks-std", &["Result"]);
    std::fs::write(dir.join("hiko.lock.toml"), "schema_version = 1\n").unwrap();
    let err = compile_in(&dir, "import Aws.S3", s3_options()).unwrap_err();
    assert_eq!(
        err,
        codegen(&format!(
            "module Aws.S3 of extension 'aws' needs Std.Result: dependency 'Std' declared in project manifest '{}' is missing from module lockfile '{}'; {DECLARE_STD}",
            dir.join("hiko.toml").display(),
            dir.join("hiko.lock.toml").display()
        ))
    );
}

#[test]
fn a_lockfile_pinning_an_undeclared_std_ends_in_the_fix() {
    let dir = project("undeclared-std", &["Result"]);
    let manifest = std::fs::read_to_string(dir.join("hiko.toml")).unwrap();
    std::fs::write(dir.join("hiko.toml"), manifest.replace("Std =", "Other =")).unwrap();
    let err = compile_in(&dir, "import Aws.S3", s3_options()).unwrap_err();
    assert_eq!(
        err,
        codegen(&format!(
            "module Aws.S3 of extension 'aws' needs Std.Result: package 'Std' in '{}' is not declared in project manifest '{}'; {DECLARE_STD}",
            dir.join("hiko.lock.toml").display(),
            dir.join("hiko.toml").display()
        ))
    );
}

#[test]
fn a_loader_error_about_another_package_keeps_its_own_message() {
    let dir = project("other-package", &["Result"]);
    let lock = std::fs::read_to_string(dir.join("hiko.lock.toml")).unwrap();
    std::fs::write(
        dir.join("hiko.lock.toml"),
        format!("{lock}\n[packages.Other]\nversion = \"0.1.0\"\nbase_url = \"http://127.0.0.1:8000/Other-v0.1.0\"\n\n[packages.Other.modules]\n"),
    )
    .unwrap();
    let err = compile_in(&dir, "import Aws.S3", s3_options()).unwrap_err();
    assert_eq!(
        err,
        codegen(&format!(
            "module Aws.S3 of extension 'aws' needs Std.Result: package 'Other' in '{}' is not declared in project manifest '{}'",
            dir.join("hiko.lock.toml").display(),
            dir.join("hiko.toml").display()
        ))
    );
}

/// An extension module whose one requirement has an import of its own.
static LOGGING: Extension = Extension {
    name: "logging",
    types: &[],
    builtins: &[],
    modules: &[(
        "Logging.M",
        "import Std.Log\nstructure M = struct\n  val n = 1\nend\n",
    )],
    requires: &["Std.Log"],
    configure: |_| Ok(std::sync::Arc::new(())),
};

fn logging_options() -> ModuleLoaderOptions {
    ModuleLoaderOptions {
        extensions: Extension::compile_surface(&[&LOGGING]),
        ..s3_options()
    }
}

#[test]
fn a_librarys_own_imports_are_not_checked_against_requires() {
    assert_eq!(LOGGING.undeclared_imports(), Ok(vec![]));
    let dir = project("logging", &["Log", "Option"]);
    compile_in(&dir, "import Logging.M", logging_options()).expect("Std.Log imports Std.Option");
}

#[test]
fn a_librarys_own_unpinned_import_names_the_lockfile_and_the_fix() {
    let dir = project("logging-unpinned", &["Log"]);
    let err = compile_in(&dir, "import Logging.M", logging_options()).unwrap_err();
    assert_eq!(
        err,
        codegen(&format!(
            "module 'Option' in package 'Std' for named import 'Std.Option' not found in {}; {DECLARE_STD}",
            dir.join("hiko.lock.toml").display()
        ))
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
    assert_eq!(
        err,
        codegen(
            "module Aws.S3 of extension 'aws' needs Std.Result, which this artifact does not embed; build it with --lock and a lockfile that pins Std.Result"
        )
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
