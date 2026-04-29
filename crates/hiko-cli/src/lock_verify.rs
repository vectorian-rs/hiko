use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;
use std::time::Duration;

use hiko_common::{blake3_hex, http_get_text_limited};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct LockManifestDefaults {
    lockfile: Option<String>,
    #[serde(default)]
    #[serde(rename = "policy")]
    _policy: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct LockRegistry {
    url: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct LockDependency {
    version: String,
    registry: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct LockProjectManifest {
    #[serde(default)]
    #[serde(rename = "project")]
    _project: toml::Table,
    #[serde(default)]
    defaults: LockManifestDefaults,
    #[serde(default)]
    registries: BTreeMap<String, LockRegistry>,
    #[serde(default)]
    dependencies: BTreeMap<String, LockDependency>,
    #[serde(default)]
    #[serde(rename = "policies")]
    _policies: toml::Table,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ModuleLockfile {
    schema_version: u32,
    #[serde(default)]
    packages: BTreeMap<String, ModuleLockPackage>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ModuleLockPackage {
    version: String,
    base_url: String,
    #[serde(default)]
    modules: BTreeMap<String, String>,
}

const LOCK_VERIFY_TIMEOUT: Duration = Duration::from_secs(30);
const LOCK_VERIFY_MAX_BYTES: u64 = 1024 * 1024;

pub(crate) fn lock_command(args: &[String]) {
    let usage = "Usage: hiko lock verify [--config <hiko.toml>]";
    let Some(subcommand) = args.first() else {
        eprintln!("{usage}");
        process::exit(1);
    };
    if subcommand != "verify" {
        eprintln!("{usage}");
        process::exit(1);
    }

    let config_path = parse_verify_args(&args[1..], usage);
    let manifest_path = resolve_manifest_path(config_path).unwrap_or_else(|message| {
        eprintln!("{message}");
        process::exit(1);
    });

    if let Err(message) = verify_lockfile(&manifest_path) {
        eprintln!("{message}");
        process::exit(1);
    }
}

fn parse_verify_args(args: &[String], usage: &str) -> Option<PathBuf> {
    let mut config_path = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--config" => {
                let Some(path) = args.get(i + 1) else {
                    eprintln!("{usage}");
                    process::exit(1);
                };
                config_path = Some(PathBuf::from(path));
                i += 2;
            }
            arg if arg.starts_with("--config=") => {
                config_path = Some(PathBuf::from(arg.trim_start_matches("--config=")));
                i += 1;
            }
            _ => {
                eprintln!("{usage}");
                process::exit(1);
            }
        }
    }
    config_path
}

fn resolve_manifest_path(config_path: Option<PathBuf>) -> Result<PathBuf, String> {
    if let Some(path) = config_path {
        return Ok(path);
    }
    let cwd = env::current_dir().map_err(|e| format!("Cannot determine current directory: {e}"))?;
    find_project_manifest_from(&cwd)
        .ok_or_else(|| "No hiko.toml found; pass --config <hiko.toml>".to_string())
}

fn verify_lockfile(manifest_path: &Path) -> Result<(), String> {
    let manifest = load_project_manifest(manifest_path)?;
    let manifest_dir = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    let lockfile_name = manifest
        .defaults
        .lockfile
        .as_deref()
        .unwrap_or("hiko.lock.toml");
    let lockfile_path = manifest_dir.join(lockfile_name);
    let lockfile = load_module_lockfile(&lockfile_path)?;

    validate_module_lockfile(&lockfile, &lockfile_path)?;
    validate_module_lockfile_against_manifest(&lockfile, &lockfile_path, &manifest, manifest_path)?;
    verify_locked_modules(&lockfile)
}

fn verify_locked_modules(lockfile: &ModuleLockfile) -> Result<(), String> {
    let mut failures = 0usize;
    for (package_name, package) in &lockfile.packages {
        for (module_name, expected_hash) in &package.modules {
            let module_url = format!(
                "{}/modules/{module_name}.hml",
                package.base_url.trim_end_matches('/')
            );
            match verify_locked_module(package_name, module_name, &module_url, expected_hash) {
                Ok(()) => println!("OK {package_name}.{module_name}"),
                Err(message) => {
                    failures += 1;
                    eprintln!("ERROR {package_name}.{module_name}: {message}");
                }
            }
        }
    }

    if failures == 0 {
        Ok(())
    } else {
        Err(format!("lock verification failed for {failures} module(s)"))
    }
}

fn load_project_manifest(path: &Path) -> Result<LockProjectManifest, String> {
    let text = fs::read_to_string(path)
        .map_err(|e| format!("Cannot read project manifest '{}': {e}", path.display()))?;
    toml::from_str(&text).map_err(|e| format!("Invalid project manifest '{}': {e}", path.display()))
}

fn load_module_lockfile(path: &Path) -> Result<ModuleLockfile, String> {
    let text = fs::read_to_string(path)
        .map_err(|e| format!("Cannot read module lockfile '{}': {e}", path.display()))?;
    toml::from_str(&text).map_err(|e| format!("Invalid module lockfile '{}': {e}", path.display()))
}

fn validate_module_lockfile(lockfile: &ModuleLockfile, path: &Path) -> Result<(), String> {
    if lockfile.schema_version != 1 {
        return Err(format!(
            "unsupported module lockfile schema_version {} in '{}'; expected 1",
            lockfile.schema_version,
            path.display()
        ));
    }
    for (package_name, package) in &lockfile.packages {
        validate_remote_base_url(package_name, &package.base_url, path)?;
        if package.version.trim().is_empty() {
            return Err(format!(
                "package '{package_name}' in '{}' is missing version",
                path.display()
            ));
        }
        for (module_name, expected_hash) in &package.modules {
            if module_name.trim().is_empty() || expected_hash.trim().is_empty() {
                return Err(format!(
                    "package '{package_name}' in '{}' has an empty module name or hash",
                    path.display()
                ));
            }
        }
    }
    Ok(())
}

fn validate_module_lockfile_against_manifest(
    lockfile: &ModuleLockfile,
    lockfile_path: &Path,
    manifest: &LockProjectManifest,
    manifest_path: &Path,
) -> Result<(), String> {
    if manifest.dependencies.is_empty() {
        return Ok(());
    }
    for package_name in lockfile.packages.keys() {
        if !manifest.dependencies.contains_key(package_name) {
            return Err(format!(
                "package '{package_name}' in '{}' is not declared in project manifest '{}'",
                lockfile_path.display(),
                manifest_path.display()
            ));
        }
    }
    for (package_name, dependency) in &manifest.dependencies {
        let package = lockfile.packages.get(package_name).ok_or_else(|| {
            format!(
                "dependency '{package_name}' declared in project manifest '{}' is missing from module lockfile '{}'",
                manifest_path.display(),
                lockfile_path.display()
            )
        })?;
        validate_dependency_source(
            package_name,
            dependency,
            package,
            manifest,
            manifest_path,
            lockfile_path,
        )?;
    }
    Ok(())
}

fn validate_dependency_source(
    package_name: &str,
    dependency: &LockDependency,
    package: &ModuleLockPackage,
    manifest: &LockProjectManifest,
    manifest_path: &Path,
    lockfile_path: &Path,
) -> Result<(), String> {
    if package.version != dependency.version {
        return Err(format!(
            "dependency '{package_name}' version mismatch: project manifest '{}' requires {}, but module lockfile '{}' resolves {}",
            manifest_path.display(),
            dependency.version,
            lockfile_path.display(),
            package.version
        ));
    }

    let registry = manifest
        .registries
        .get(&dependency.registry)
        .ok_or_else(|| {
            format!(
                "dependency '{package_name}' references unknown registry '{}' in project manifest '{}'",
                dependency.registry,
                manifest_path.display()
            )
        })?;
    let expected_base_url = format!(
        "{}/{}-v{}",
        registry.url.trim_end_matches('/'),
        package_name,
        dependency.version
    );
    if package.base_url.trim_end_matches('/') != expected_base_url {
        return Err(format!(
            "dependency '{package_name}' source mismatch: project manifest '{}' expects '{}', but module lockfile '{}' resolves '{}'",
            manifest_path.display(),
            expected_base_url,
            lockfile_path.display(),
            package.base_url
        ));
    }
    Ok(())
}

fn validate_remote_base_url(package_name: &str, base_url: &str, path: &Path) -> Result<(), String> {
    let base_url = base_url.trim();
    if base_url.starts_with("https://") {
        return Ok(());
    }
    if let Some(rest) = base_url.strip_prefix("http://") {
        let authority = rest.split('/').next().unwrap_or_default();
        let host = authority
            .rsplit_once('@')
            .map(|(_, host)| host)
            .unwrap_or(authority);
        let host = host
            .strip_prefix('[')
            .and_then(|host| host.split_once(']').map(|(host, _)| host))
            .unwrap_or_else(|| host.split(':').next().unwrap_or(host));
        if matches!(host, "localhost" | "127.0.0.1" | "::1") {
            return Ok(());
        }
    }
    Err(format!(
        "package '{package_name}' in '{}' uses insecure base_url '{base_url}'; use https:// or local http://127.0.0.1/localhost for development",
        path.display()
    ))
}

fn verify_locked_module(
    package_name: &str,
    module_name: &str,
    module_url: &str,
    expected_hash: &str,
) -> Result<(), String> {
    let source = http_get_text_limited(module_url, LOCK_VERIFY_TIMEOUT, LOCK_VERIFY_MAX_BYTES)
        .map_err(|e| format!("cannot fetch '{module_url}': {e}"))?;
    let actual = blake3_hex(source.as_bytes());
    let expected = normalize_blake3(expected_hash);
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "BLAKE3 mismatch for {package_name}.{module_name}; expected {expected}, got {actual}; url {module_url}"
        ))
    }
}

fn normalize_blake3(hash: &str) -> String {
    hash.trim()
        .strip_prefix("blake3:")
        .unwrap_or(hash.trim())
        .to_ascii_lowercase()
}

fn find_project_manifest_from(start_dir: &Path) -> Option<PathBuf> {
    let mut current = Some(start_dir);
    while let Some(dir) = current {
        let candidate = dir.join("hiko.toml");
        if candidate.is_file() {
            return Some(candidate);
        }
        current = dir.parent();
    }
    None
}
