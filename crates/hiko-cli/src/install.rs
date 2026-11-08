//! `hiko install`: write the module lockfile from the project manifest.
//!
//! Each `[dependencies]` entry names a package, a version and a registry.
//! Install fetches the package's `package.toml` from
//! `<registry>/<Package>-v<version>`, downloads every module it lists,
//! checks each against the manifest's BLAKE3 hash, stores it in the module
//! cache, and writes all of them to `hiko.lock.toml`. Nothing is written
//! unless every module verifies. `--check` reads only the package
//! manifests, writes nothing, and fails when the lockfile differs from what
//! install would write; `lock verify` checks the modules themselves. Compiling never changes
//! the lockfile; this command is the one place that does.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;

use hiko_common::{blake3_hex, http_get_text_limited};
use serde::Deserialize;

use crate::lock_verify::{
    LOCK_VERIFY_MAX_BYTES, LOCK_VERIFY_TIMEOUT, LockProjectManifest, ModuleLockPackage,
    ModuleLockfile, dependency_base_url, load_module_lockfile, load_project_manifest,
    normalize_blake3, resolve_manifest_path, validate_module_lockfile,
    validate_module_lockfile_against_manifest, validate_remote_base_url,
};
use crate::parse_flag_value;
use hiko_compile::compiler::{
    CompileError, bounded_lockfile_path, module_cache_path, project_module_cache_dir,
    store_cached_module,
};

/// A published package's `package.toml`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PackageManifest {
    name: String,
    version: String,
    modules: BTreeMap<String, String>,
}

/// A downloaded module, verified, to store in the cache once every module
/// has verified.
struct Download {
    url: String,
    hash: String,
    source: String,
}

pub(crate) fn install_command(args: &[String]) {
    let usage = "Usage: hiko install [--config <hiko.toml>] [--check]";
    let (config_path, check) = parse_install_args(args, usage);
    let manifest_path = resolve_manifest_path(config_path).unwrap_or_else(|message| {
        eprintln!("{message}");
        process::exit(1);
    });
    if let Err(message) = install(&manifest_path, check) {
        eprintln!("{message}");
        process::exit(1);
    }
}

fn parse_install_args(args: &[String], usage: &str) -> (Option<PathBuf>, bool) {
    let mut config_path = None;
    let mut check = false;
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--check" {
            check = true;
            i += 1;
            continue;
        }
        let Some((path, consumed)) = parse_flag_value(args, "--config", i, usage) else {
            eprintln!("{usage}");
            process::exit(1);
        };
        config_path = Some(PathBuf::from(path));
        i += consumed;
    }
    (config_path, check)
}

fn install(manifest_path: &Path, check: bool) -> Result<(), String> {
    let manifest = load_project_manifest(manifest_path)?;
    let lockfile_path =
        bounded_lockfile_path(manifest_path, manifest.defaults.lockfile.as_deref())?;
    // An existing lockfile that does not read is replaced, and said so:
    // install is the command that repairs it.
    let old = if lockfile_path.exists() {
        load_module_lockfile(&lockfile_path)
            .and_then(|old| validate_module_lockfile(&old, &lockfile_path).map(|()| old))
            .map(Some)
    } else {
        Ok(None)
    };

    let mut packages = BTreeMap::new();
    for name in manifest.dependencies.keys() {
        packages.insert(
            name.clone(),
            resolve_package(&manifest, manifest_path, name)?,
        );
    }
    let new = ModuleLockfile {
        schema_version: 1,
        packages,
    };
    // The rules `lock verify` reads a lockfile by.
    validate_module_lockfile(&new, &lockfile_path)?;
    validate_module_lockfile_against_manifest(&new, &lockfile_path, &manifest, manifest_path)?;

    let changes = match &old {
        Ok(old) => describe_changes(old.as_ref(), &new),
        Err(reason) => {
            let mut lines = vec![format!("! replacing an invalid lockfile: {reason}")];
            lines.extend(describe_changes(None, &new));
            lines
        }
    };
    for line in &changes {
        println!("{line}");
    }

    if !check {
        let cache_dir =
            project_module_cache_dir(manifest_path, manifest.defaults.lib_cache.as_deref())
                .map_err(compile_error)?;
        for d in download_modules(&new, &cache_dir)? {
            store_cached_module(&cache_dir, &d.url, &d.hash, &d.source).map_err(compile_error)?;
        }
    }
    if changes.is_empty() {
        println!("{} is up to date", lockfile_path.display());
        return Ok(());
    }
    if check {
        return Err(format!(
            "{} is out of date; run 'hiko install'",
            lockfile_path.display()
        ));
    }
    write_atomically(&lockfile_path, &render_lockfile(&new)?)?;
    println!("wrote {}", lockfile_path.display());
    Ok(())
}

/// Package `name`'s lock entry, from the `package.toml` its registry
/// serves. Hashes are stored as `blake3:<lowercase hex>`.
fn resolve_package(
    manifest: &LockProjectManifest,
    manifest_path: &Path,
    name: &str,
) -> Result<ModuleLockPackage, String> {
    let dependency = &manifest.dependencies[name];
    // Both become part of a URL and of the lockfile's table names.
    check_name(name, "package name", manifest_path)?;
    check_version(&dependency.version, name, manifest_path)?;
    let registry = manifest
        .registries
        .get(&dependency.registry)
        .ok_or_else(|| {
            format!(
                "dependency '{name}' references unknown registry '{}' in project manifest '{}'",
                dependency.registry,
                manifest_path.display()
            )
        })?;
    let base_url = dependency_base_url(&registry.url, name, &dependency.version);
    validate_remote_base_url(name, &base_url, manifest_path)?;

    let package_url = format!("{base_url}/package.toml");
    let text = fetch(&package_url)?;
    let package: PackageManifest = toml::from_str(&text)
        .map_err(|e| format!("invalid package manifest '{package_url}': {e}"))?;
    if package.name != name || package.version != dependency.version {
        return Err(format!(
            "'{package_url}' describes {} {}, but hiko.toml asks for {name} {}",
            package.name, package.version, dependency.version
        ));
    }
    if package.modules.is_empty() {
        return Err(format!("'{package_url}' lists no modules"));
    }
    let mut modules = BTreeMap::new();
    for (module, hash) in package.modules {
        check_name(&module, "module name", Path::new(&package_url))?;
        let hex = parse_blake3(&hash)
            .ok_or_else(|| format!("'{package_url}': module {module} has invalid hash '{hash}'"))?;
        modules.insert(module, format!("blake3:{hex}"));
    }
    Ok(ModuleLockPackage {
        version: dependency.version.clone(),
        base_url,
        modules,
    })
}

/// Every module in `lockfile` that the cache does not already hold,
/// downloaded and checked against its hash. A cached copy is used only
/// when it still matches.
fn download_modules(lockfile: &ModuleLockfile, cache_dir: &Path) -> Result<Vec<Download>, String> {
    let mut downloads = Vec::new();
    for (name, package) in &lockfile.packages {
        for (module, hash) in &package.modules {
            let hex = normalize_blake3(hash);
            let url = format!("{}/modules/{module}.hml", package.base_url);
            let cached = fs::read(module_cache_path(cache_dir, &url, &hex));
            if cached.is_ok_and(|bytes| blake3_hex(&bytes) == hex) {
                continue;
            }
            let source = fetch(&url)?;
            let actual = blake3_hex(source.as_bytes());
            if actual != hex {
                return Err(format!(
                    "{name}.{module}: '{url}' has BLAKE3 {actual}, but its package.toml lists {hex}"
                ));
            }
            downloads.push(Download {
                url,
                hash: hex,
                source,
            });
        }
    }
    Ok(downloads)
}

fn compile_error(e: CompileError) -> String {
    match e {
        CompileError::Codegen(message) => message,
        other => format!("{other:?}"),
    }
}

fn fetch(url: &str) -> Result<String, String> {
    http_get_text_limited(url, LOCK_VERIFY_TIMEOUT, LOCK_VERIFY_MAX_BYTES)
        .map_err(|e| format!("cannot fetch '{url}': {e}"))
}

/// Package and module names become parts of URLs, file names and TOML
/// keys, so they must be identifiers: a letter, then letters, digits or
/// `_`. `source` is where the name was read.
fn check_name(name: &str, what: &str, source: &Path) -> Result<(), String> {
    let mut chars = name.chars();
    let valid = chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
    if valid {
        Ok(())
    } else {
        Err(format!(
            "'{}': invalid {what} '{name}'; use a letter, then letters, digits or '_'",
            source.display()
        ))
    }
}

/// A version is part of the package URL: letters, digits, `.`, `-` and
/// `+`, not starting with `.`.
fn check_version(version: &str, package: &str, source: &Path) -> Result<(), String> {
    let valid = !version.is_empty()
        && !version.starts_with('.')
        && version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'));
    if valid {
        Ok(())
    } else {
        Err(format!(
            "'{}': invalid version '{version}' for {package}",
            source.display()
        ))
    }
}

/// The lowercase hex digest of a `blake3:<64 hex digits>` hash.
fn parse_blake3(hash: &str) -> Option<String> {
    let hex = hash.strip_prefix("blake3:")?;
    (hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| hex.to_ascii_lowercase())
}

/// One line per difference between the lockfile on disk and the new one:
/// `+` added, `-` removed, `~` changed.
fn describe_changes(old: Option<&ModuleLockfile>, new: &ModuleLockfile) -> Vec<String> {
    let no_packages = BTreeMap::new();
    let no_modules = BTreeMap::new();
    let old_packages = old.map_or(&no_packages, |o| &o.packages);
    let mut lines = Vec::new();
    for (name, package) in &new.packages {
        let previous = old_packages.get(name);
        match previous {
            None => lines.push(format!(
                "+ {name} {} ({})",
                package.version, package.base_url
            )),
            Some(p)
                if p.version != package.version
                    || p.base_url.trim_end_matches('/')
                        != package.base_url.trim_end_matches('/') =>
            {
                lines.push(format!(
                    "~ {name} {} ({}) -> {} ({})",
                    p.version, p.base_url, package.version, package.base_url
                ))
            }
            Some(_) => {}
        }
        let old_modules = previous.map_or(&no_modules, |p| &p.modules);
        for (module, hash) in &package.modules {
            match old_modules.get(module) {
                None => lines.push(format!("+ {name}.{module} {hash}")),
                Some(old_hash) if normalize_blake3(old_hash) != normalize_blake3(hash) => {
                    lines.push(format!("~ {name}.{module} {old_hash} -> {hash}"))
                }
                Some(_) => {}
            }
        }
        for module in old_modules.keys() {
            if !package.modules.contains_key(module) {
                lines.push(format!("- {name}.{module}"));
            }
        }
    }
    for name in old_packages.keys() {
        if !new.packages.contains_key(name) {
            lines.push(format!("- {name}"));
        }
    }
    lines
}

fn render_lockfile(lockfile: &ModuleLockfile) -> Result<String, String> {
    toml::to_string(lockfile).map_err(|e| format!("cannot write the lockfile: {e}"))
}

/// Replace `path` with `text` through a temporary file next to it, so a
/// failed write leaves the old lockfile intact.
fn write_atomically(path: &Path, text: &str) -> Result<(), String> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".tmp-{}", process::id()));
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, text).map_err(|e| format!("cannot write '{}': {e}", tmp.display()))?;
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("cannot write '{}': {e}", path.display())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write as _;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Serve files under `root` over HTTP on 127.0.0.1 until the test ends.
    fn serve(root: PathBuf) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut request = String::new();
                if BufReader::new(&stream).read_line(&mut request).is_err() {
                    continue;
                }
                let path = request.split_whitespace().nth(1).unwrap_or("/");
                let response = match fs::read(root.join(path.trim_start_matches('/'))) {
                    Ok(body) => {
                        let mut r = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        )
                        .into_bytes();
                        r.extend(body);
                        r
                    }
                    Err(_) => {
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                            .to_vec()
                    }
                };
                let _ = stream.write_all(&response);
            }
        });
        url
    }

    fn temp_dir(name: &str) -> PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "hiko-install-{name}-{}-{}",
            process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A registry with package `Pkg` 0.1.0 holding the given modules, and a
    /// project depending on it with its cache inside the project.
    fn setup(name: &str, modules: &[(&str, &str)]) -> (PathBuf, PathBuf) {
        let registry = temp_dir(&format!("{name}-registry"));
        let package = registry.join("Pkg-v0.1.0");
        fs::create_dir_all(package.join("modules")).unwrap();
        let mut manifest = "name = \"Pkg\"\nversion = \"0.1.0\"\n\n[modules]\n".to_string();
        for (module, source) in modules {
            fs::write(
                package.join("modules").join(format!("{module}.hml")),
                source,
            )
            .unwrap();
            let _ = writeln!(
                manifest,
                "{module} = \"blake3:{}\"",
                blake3_hex(source.as_bytes())
            );
        }
        fs::write(package.join("package.toml"), manifest).unwrap();
        let url = serve(registry.clone());

        let project = temp_dir(&format!("{name}-project"));
        fs::write(
            project.join("hiko.toml"),
            format!(
                "[defaults]\nlockfile = \"hiko.lock.toml\"\nlib_cache = \"cache\"\n\n\
                 [registries.test]\nurl = \"{url}\"\n\n\
                 [dependencies]\nPkg = {{ version = \"0.1.0\", registry = \"test\" }}\n"
            ),
        )
        .unwrap();
        (project, package)
    }

    #[test]
    fn installs_every_module_and_caches_it() {
        let (project, _) = setup("fresh", &[("A", "val a = 1\n"), ("B", "val b = 2\n")]);
        let manifest = project.join("hiko.toml");
        install(&manifest, false).unwrap();

        let lock = load_module_lockfile(&project.join("hiko.lock.toml")).unwrap();
        let package = &lock.packages["Pkg"];
        assert_eq!(package.version, "0.1.0");
        assert_eq!(
            package.modules["A"],
            format!("blake3:{}", blake3_hex(b"val a = 1\n"))
        );
        assert_eq!(package.modules.len(), 2);
        // Where the compiler's loader looks for it.
        let url = format!("{}/modules/A.hml", package.base_url);
        let cached = hiko_compile::compiler::module_cache_path(
            &project.join("cache"),
            &url,
            &normalize_blake3(&package.modules["A"]),
        );
        assert_eq!(fs::read_to_string(cached).unwrap(), "val a = 1\n");

        // A second install changes nothing, and --check agrees.
        install(&manifest, false).unwrap();
        install(&manifest, true).unwrap();
    }

    #[test]
    fn install_repairs_missing_and_corrupt_cache_without_changing_pins() {
        let source = "val a = 42\n";
        let (project, _) = setup("repair", &[("A", source)]);
        let manifest = project.join("hiko.toml");
        install(&manifest, false).unwrap();
        let lockfile = project.join("hiko.lock.toml");
        let pins = fs::read(&lockfile).unwrap();
        let cached = fs::read_dir(project.join("cache"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();

        fs::remove_file(&cached).unwrap();
        install(&manifest, false).unwrap();
        assert_eq!(fs::read_to_string(&cached).unwrap(), source);
        assert_eq!(fs::read(&lockfile).unwrap(), pins);

        fs::write(&cached, "tampered").unwrap();
        install(&manifest, true).unwrap();
        assert_eq!(fs::read_to_string(&cached).unwrap(), "tampered");
        assert_eq!(fs::read(&lockfile).unwrap(), pins);
        install(&manifest, false).unwrap();
        assert_eq!(fs::read_to_string(&cached).unwrap(), source);
        assert_eq!(fs::read(&lockfile).unwrap(), pins);
    }

    #[test]
    fn a_module_that_does_not_match_its_hash_writes_nothing() {
        let (project, package) = setup("tampered", &[("A", "val a = 1\n")]);
        fs::write(package.join("modules/A.hml"), "val a = 2\n").unwrap();
        let err = install(&project.join("hiko.toml"), false).unwrap_err();
        assert!(err.contains("Pkg.A") && err.contains("BLAKE3"), "{err}");
        assert!(!project.join("hiko.lock.toml").exists());
        assert!(!project.join("cache").exists());
    }

    #[test]
    fn check_fails_on_an_outdated_lockfile_and_writes_nothing() {
        let (project, package) = setup("outdated", &[("A", "val a = 1\n")]);
        let manifest = project.join("hiko.toml");
        install(&manifest, false).unwrap();
        let before = fs::read_to_string(project.join("hiko.lock.toml")).unwrap();

        // A new release of the module, and its manifest updated to match.
        let source = "val a = 3\n";
        fs::write(package.join("modules/A.hml"), source).unwrap();
        fs::write(
            package.join("package.toml"),
            format!(
                "name = \"Pkg\"\nversion = \"0.1.0\"\n\n[modules]\nA = \"blake3:{}\"\n",
                blake3_hex(source.as_bytes())
            ),
        )
        .unwrap();
        let err = install(&manifest, true).unwrap_err();
        assert!(err.contains("out of date"), "{err}");
        assert_eq!(
            fs::read_to_string(project.join("hiko.lock.toml")).unwrap(),
            before
        );

        install(&manifest, false).unwrap();
        install(&manifest, true).unwrap();
    }

    #[test]
    fn cached_modules_are_not_downloaded_again_and_check_reads_only_manifests() {
        let (project, package) = setup("cached", &[("A", "val a = 1\n")]);
        let manifest = project.join("hiko.toml");
        install(&manifest, false).unwrap();
        // The registry no longer serves the module: the cached copy, which
        // still matches its hash, is enough, and --check never fetches it.
        fs::remove_file(package.join("modules/A.hml")).unwrap();
        install(&manifest, false).unwrap();
        install(&manifest, true).unwrap();
        // A cached copy that no longer matches is fetched again, and now
        // that fails.
        let cache = project.join("cache");
        let cached = fs::read_dir(&cache)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        fs::write(cached, "tampered").unwrap();
        let err = install(&manifest, false).unwrap_err();
        assert!(err.contains("cannot fetch"), "{err}");
    }

    #[test]
    fn a_package_manifest_for_another_package_is_refused() {
        let (project, package) = setup("wrong-name", &[("A", "val a = 1\n")]);
        let text = fs::read_to_string(package.join("package.toml")).unwrap();
        fs::write(
            package.join("package.toml"),
            text.replace("name = \"Pkg\"", "name = \"Other\""),
        )
        .unwrap();
        let err = install(&project.join("hiko.toml"), false).unwrap_err();
        assert!(err.contains("describes Other"), "{err}");
    }

    #[test]
    fn names_must_be_identifiers_and_versions_plain() {
        let p = Path::new("p");
        assert!(check_name("Result", "module name", p).is_ok());
        assert!(check_name("Int32", "module name", p).is_ok());
        for bad in ["", "../x", "a/b", "1x", "x.y", "x y", "a]"] {
            assert!(check_name(bad, "module name", p).is_err(), "{bad:?}");
        }
        for good in ["0.1.0", "1.0.0-rc.1", "2.0.0+build"] {
            assert!(check_version(good, "Pkg", p).is_ok(), "{good:?}");
        }
        for bad in ["", "../x", ".1", "0.1.0/../Other-v9", "1 0", "1?x"] {
            assert!(check_version(bad, "Pkg", p).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn an_invalid_lockfile_is_replaced_and_check_reports_it() {
        let (project, _) = setup("invalid-lock", &[("A", "val a = 1\n")]);
        let manifest = project.join("hiko.toml");
        fs::write(project.join("hiko.lock.toml"), "<<<<<<< HEAD\n").unwrap();
        let err = install(&manifest, true).unwrap_err();
        assert!(err.contains("out of date"), "{err}");
        install(&manifest, false).unwrap();
        install(&manifest, true).unwrap();
    }

    #[test]
    fn removing_every_dependency_empties_the_lockfile() {
        let (project, _) = setup("emptied", &[("A", "val a = 1\n")]);
        let manifest = project.join("hiko.toml");
        install(&manifest, false).unwrap();
        let text = fs::read_to_string(&manifest).unwrap();
        let without = text.split("[dependencies]").next().unwrap().to_string();
        fs::write(&manifest, without).unwrap();
        install(&manifest, false).unwrap();
        let lock = load_module_lockfile(&project.join("hiko.lock.toml")).unwrap();
        assert!(lock.packages.is_empty());
        install(&manifest, true).unwrap();
    }

    #[test]
    fn hashes_must_be_blake3_hex() {
        let hex = "a".repeat(64);
        assert_eq!(parse_blake3(&format!("blake3:{hex}")), Some(hex.clone()));
        assert_eq!(parse_blake3(&hex), None);
        assert_eq!(parse_blake3("blake3:abc"), None);
        assert_eq!(parse_blake3(&format!("blake3:{}", "g".repeat(64))), None);
    }

    #[test]
    fn rendered_lockfile_reads_back_the_same() {
        let mut modules = BTreeMap::new();
        modules.insert("A".to_string(), format!("blake3:{}", "a".repeat(64)));
        let lockfile = ModuleLockfile {
            schema_version: 1,
            packages: BTreeMap::from([(
                "Pkg".to_string(),
                ModuleLockPackage {
                    version: "0.1.0".to_string(),
                    base_url: "https://example.com/Pkg-v0.1.0".to_string(),
                    modules,
                },
            )]),
        };
        let text = render_lockfile(&lockfile).unwrap();
        let read: ModuleLockfile = toml::from_str(&text).unwrap();
        assert_eq!(read, lockfile);
    }
}
