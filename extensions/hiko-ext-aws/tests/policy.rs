//! The policy and the host-resource accounting, without AWS credentials or
//! the network: nothing here reaches a listing.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use hiko_compile::chunk::CompiledProgram;
use hiko_compile::compiler::{
    Compiler, EmbeddedModule, EmbeddedSources, ModuleLoaderOptions, module_cache_path,
};
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

/// `s3_options` with `extension` as the only extension.
fn nested_options(extension: &'static Extension) -> ModuleLoaderOptions {
    ModuleLoaderOptions {
        extensions: Extension::compile_surface(&[extension]),
        ..s3_options()
    }
}

#[test]
fn an_import_in_a_let_is_checked_when_the_script_loaded_it_first() {
    static NESTED: Extension = Extension {
        name: "nested",
        types: &[],
        builtins: &[],
        modules: &[("Nested.M", "val n = let import Std.Result in 1 end\n")],
        requires: &[],
        configure: |_| Ok(std::sync::Arc::new(())),
    };
    let err = compile_in(
        with_result(),
        "import Std.Result\nimport Nested.M",
        nested_options(&NESTED),
    )
    .unwrap_err();
    assert_eq!(
        err,
        codegen(
            "module Nested.M of extension 'nested' imports Std.Result, which the extension does not list in its requires; add it to requires in the extension's Extension"
        )
    );
}

#[test]
fn a_use_in_a_let_is_refused_when_the_script_used_the_file_first() {
    static NESTED: Extension = Extension {
        name: "nested",
        types: &[],
        builtins: &[],
        modules: &[("Nested.M", "val n = let use \"./helper.hml\" in 1 end\n")],
        requires: &[],
        configure: |_| Ok(std::sync::Arc::new(())),
    };
    let dir = temp_dir("nested-use");
    std::fs::write(dir.join("helper.hml"), "val helper = 1\n").unwrap();
    let err = compile_in(
        &dir,
        "use \"./helper.hml\"\nimport Nested.M",
        nested_options(&NESTED),
    )
    .unwrap_err();
    assert_eq!(
        err,
        codegen(
            "module Nested.M of extension 'nested' declares use './helper.hml'; an extension module cannot use a file"
        )
    );
}

static PATHS: Extension = Extension {
    name: "paths",
    types: &[],
    builtins: &[],
    modules: &[("Paths.M", "import __Builtin.Path\nval n = 1\n")],
    requires: &["__Builtin.Path"],
    configure: |_| Ok(std::sync::Arc::new(())),
};

#[test]
fn a_required_internal_builtin_needs_no_project() {
    let dir = temp_dir("builtin-no-project");
    compile_in(&dir, "import Paths.M", nested_options(&PATHS)).expect("no lockfile applies");
}

#[test]
fn a_required_internal_builtin_needs_no_embedded_copy() {
    static NOTHING: EmbeddedSources = EmbeddedSources {
        modules: &[],
        files: None,
    };
    let options = ModuleLoaderOptions {
        embedded: Some(&NOTHING),
        ..nested_options(&PATHS)
    };
    compile_in(with_result(), "import Paths.M", options).expect("nothing to embed");
}

/// The end of an error that `hiko install` repairs.
const REWRITE_LOCKFILE: &str =
    "run 'hiko install', which fetches the modules and rewrites the lockfile from hiko.toml";

/// `project`, with `from` in its manifest replaced by `to`.
fn project_with_manifest(name: &str, from: &str, to: &str) -> PathBuf {
    let dir = project(name, &["Result"]);
    let manifest = std::fs::read_to_string(dir.join("hiko.toml")).unwrap();
    assert!(manifest.contains(from));
    std::fs::write(dir.join("hiko.toml"), manifest.replace(from, to)).unwrap();
    dir
}

#[test]
fn a_version_mismatch_ends_in_the_fix() {
    let dir = project_with_manifest("version", "version = \"0.1.0\"", "version = \"0.2.0\"");
    let err = compile_in(&dir, "import Aws.S3", s3_options()).unwrap_err();
    assert_eq!(
        err,
        codegen(&format!(
            "module Aws.S3 of extension 'aws' needs Std.Result: dependency 'Std' version mismatch: project manifest '{}' requires 0.2.0, but module lockfile '{}' resolves 0.1.0; {REWRITE_LOCKFILE}",
            dir.join("hiko.toml").display(),
            dir.join("hiko.lock.toml").display()
        ))
    );
}

#[test]
fn a_source_mismatch_ends_in_the_fix() {
    let dir = project_with_manifest("source", "127.0.0.1:8000", "127.0.0.1:9000");
    let err = compile_in(&dir, "import Aws.S3", s3_options()).unwrap_err();
    assert_eq!(
        err,
        codegen(&format!(
            "module Aws.S3 of extension 'aws' needs Std.Result: dependency 'Std' source mismatch: project manifest '{}' expects 'http://127.0.0.1:9000/Std-v0.1.0', but module lockfile '{}' resolves 'http://127.0.0.1:8000/Std-v0.1.0'; {REWRITE_LOCKFILE}",
            dir.join("hiko.toml").display(),
            dir.join("hiko.lock.toml").display()
        ))
    );
}

#[test]
fn an_unknown_registry_ends_in_the_fix() {
    let dir = project_with_manifest("registry", "registry = \"local\"", "registry = \"gone\"");
    let err = compile_in(&dir, "import Aws.S3", s3_options()).unwrap_err();
    assert_eq!(
        err,
        codegen(&format!(
            "module Aws.S3 of extension 'aws' needs Std.Result: dependency 'Std' references unknown registry 'gone' in project manifest '{}'; add [registries.gone] to hiko.toml (see docs/language/imports.md), then run 'hiko install'",
            dir.join("hiko.toml").display()
        ))
    );
}

/// The hash `libraries/Std-v0.1.0/package.toml` lists for `Std.Result`.
fn std_result_hash() -> String {
    let package: toml::Table = toml::from_str(
        &std::fs::read_to_string(repo().join("libraries/Std-v0.1.0/package.toml")).unwrap(),
    )
    .unwrap();
    let hash = package["modules"]["Result"].as_str().unwrap();
    hash.strip_prefix("blake3:").unwrap().to_string()
}

#[test]
fn a_wrong_pinned_hash_says_to_check_before_repinning() {
    let dir = project("wrong-hash", &["Result"]);
    let wrong = "0".repeat(64);
    let lock = std::fs::read_to_string(dir.join("hiko.lock.toml")).unwrap();
    std::fs::write(
        dir.join("hiko.lock.toml"),
        lock.replace(&std_result_hash(), &wrong),
    )
    .unwrap();
    let err = compile_in(&dir, "import Aws.S3", s3_options()).unwrap_err();
    assert_eq!(
        err,
        codegen(&format!(
            "module Aws.S3 of extension 'aws' needs Std.Result: local named import 'Std.Result' ({}) failed BLAKE3 verification: expected {wrong}, got {}; the source differs from the lockfile's pin, so find out why at the registry before 'hiko install' replaces the pin",
            repo()
                .join("libraries/Std-v0.1.0/modules/Result.hml")
                .display(),
            std_result_hash()
        ))
    );
}

/// `project`, with Std at the registry on `authority`.
fn project_at(name: &str, authority: &str) -> PathBuf {
    let dir = project_with_manifest(name, "127.0.0.1:8000", authority);
    let lock = std::fs::read_to_string(dir.join("hiko.lock.toml")).unwrap();
    std::fs::write(
        dir.join("hiko.lock.toml"),
        lock.replace("127.0.0.1:8000", authority),
    )
    .unwrap();
    dir
}

/// A loopback registry that answers one request with `Std.Result`, and its
/// authority.
fn serve_std_result() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let authority = listener.local_addr().unwrap().to_string();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0; 4096];
        let _ = stream.read(&mut request).unwrap();
        let body = std::fs::read(repo().join("libraries/Std-v0.1.0/modules/Result.hml")).unwrap();
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(head.as_bytes()).unwrap();
        stream.write_all(&body).unwrap();
    });
    authority
}

#[test]
fn a_cache_that_cannot_be_written_says_to_repair_it() {
    let dir = project_at("blocked-cache", &serve_std_result());
    let manifest = std::fs::read_to_string(dir.join("hiko.toml")).unwrap();
    std::fs::write(
        dir.join("hiko.toml"),
        manifest.replace("[defaults]\n", "[defaults]\nlib_cache = \"blocked\"\n"),
    )
    .unwrap();
    std::fs::write(dir.join("blocked"), "an ordinary file\n").unwrap();
    let options = ModuleLoaderOptions {
        local_package_root: None,
        cache_dir: None,
        ..s3_options()
    };
    let err = compile_in(&dir, "import Aws.S3", options).unwrap_err();
    assert_eq!(
        err,
        codegen(&format!(
            "module Aws.S3 of extension 'aws' needs Std.Result: cannot create module cache '{}': File exists (os error 17); repair that path: the module cache must be a directory this user can write",
            dir.join("blocked").display()
        ))
    );
}

#[test]
fn a_cached_module_that_cannot_be_read_says_to_repair_it() {
    let dir = project("unreadable-cache", &["Result"]);
    let cache = dir.join("cache");
    std::fs::create_dir_all(&cache).unwrap();
    let cached = module_cache_path(
        &cache,
        "http://127.0.0.1:8000/Std-v0.1.0/modules/Result.hml",
        &std_result_hash(),
    );
    std::fs::write(&cached, [0xff]).unwrap();
    let options = ModuleLoaderOptions {
        local_package_root: None,
        cache_dir: Some(cache),
        ..s3_options()
    };
    let err = compile_in(&dir, "import Aws.S3", options).unwrap_err();
    assert_eq!(
        err,
        codegen(&format!(
            "module Aws.S3 of extension 'aws' needs Std.Result: cannot read cached module '{}': stream did not contain valid UTF-8; repair or remove that file",
            cached.display()
        ))
    );
}

#[test]
fn a_local_module_that_cannot_be_read_says_to_repair_it() {
    let dir = project("unreadable-local", &["Result"]);
    let modules = dir.join("libraries/Std-v0.1.0/modules");
    std::fs::create_dir_all(&modules).unwrap();
    std::fs::write(modules.join("Result.hml"), [0xff]).unwrap();
    let options = ModuleLoaderOptions {
        local_package_root: Some(dir.join("libraries")),
        ..s3_options()
    };
    let err = compile_in(&dir, "import Aws.S3", options).unwrap_err();
    assert_eq!(
        err,
        codegen(&format!(
            "module Aws.S3 of extension 'aws' needs Std.Result: cannot read local module '{}': stream did not contain valid UTF-8; repair or remove that file",
            modules.join("Result.hml").display()
        ))
    );
}

#[test]
fn a_module_that_does_not_fetch_names_the_extension_and_the_fix() {
    // Port 1 on loopback: refused, with no network and no real registry.
    let dir = project_at("unreachable", "127.0.0.1:1");
    let options = ModuleLoaderOptions {
        local_package_root: None,
        ..s3_options()
    };
    let err = compile_in(&dir, "import Aws.S3", options).unwrap_err();
    assert!(
        err.starts_with(
            "Codegen(\"module Aws.S3 of extension 'aws' needs Std.Result: cannot fetch named import 'Std.Result' from 'http://127.0.0.1:1/Std-v0.1.0/modules/Result.hml': "
        ),
        "{err}"
    );
    assert!(
        err.ends_with("; check that the registry in hiko.toml is reachable\")"),
        "{err}"
    );
}

#[test]
fn an_embedded_module_failing_its_hash_names_the_extension_and_the_fix() {
    static WRONG: EmbeddedSources = EmbeddedSources {
        modules: &[EmbeddedModule {
            import_name: "Std.Result",
            blake3: "blake3:0000000000000000000000000000000000000000000000000000000000000000",
            source: include_str!("../../../libraries/Std-v0.1.0/modules/Result.hml"),
        }],
        files: None,
    };
    let options = ModuleLoaderOptions {
        embedded: Some(&WRONG),
        ..s3_options()
    };
    let err = compile_in(with_result(), "import Aws.S3", options).unwrap_err();
    assert_eq!(
        err,
        codegen(&format!(
            "module Aws.S3 of extension 'aws' needs Std.Result: embedded module 'Std.Result' failed BLAKE3 verification: expected {}, got {}; the artifact's source differs from its pin, so rebuild it with build-vm --lock",
            "0".repeat(64),
            std_result_hash()
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
