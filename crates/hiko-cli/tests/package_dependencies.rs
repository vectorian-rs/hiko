//! The public loader must not give libraries their consumer's dependency authority.
#![cfg(feature = "cli-lock")]

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use hiko_common::blake3_hex;
use hiko_compile::compiler::module_cache_path;

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Project(PathBuf);
impl Project {
    fn new(dependencies: Option<&str>) -> Self {
        let root = std::env::temp_dir().join(format!(
            "hiko-package-dependencies-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("cache")).unwrap();
        fs::write(root.join("hiko.toml"), "[defaults]\npolicy = 'deny'\nlib_cache = 'cache'\n[policies.deny]\npath = 'deny.toml'\n[registries.test]\nurl = 'http://127.0.0.1:9'\n[dependencies]\nAcme = { version = '0.1.0', registry = 'test' }\nOther = { version = '0.1.0', registry = 'test' }\n").unwrap();
        fs::write(root.join("deny.toml"), "").unwrap();
        fs::write(
            root.join("main.hml"),
            "import Acme.Entry\nval result = Entry.value + 1\n",
        )
        .unwrap();
        let mut lock = String::from("schema_version = 1\n");
        for (package, module, source, deps) in [
            (
                "Acme",
                "Entry",
                "import Other.Hidden\nstructure Entry = struct val value = Hidden.value end\n",
                dependencies,
            ),
            (
                "Other",
                "Hidden",
                "structure Hidden = struct val value = 42 end\n",
                Some("[]"),
            ),
        ] {
            let hash = blake3_hex(source.as_bytes());
            let base = format!("http://127.0.0.1:9/{package}-v0.1.0");
            lock.push_str(&format!(
                "[packages.{package}]\nversion = '0.1.0'\nbase_url = '{base}'\n"
            ));
            if let Some(deps) = deps {
                lock.push_str(&format!("dependencies = {deps}\n"));
            }
            lock.push_str(&format!(
                "[packages.{package}.modules]\n{module} = 'blake3:{hash}'\n"
            ));
            let path = module_cache_path(
                &root.join("cache"),
                &format!("{base}/modules/{module}.hml"),
                &hash,
            );
            fs::write(path, source).unwrap();
        }
        fs::write(root.join("hiko.lock.toml"), lock).unwrap();
        Self(root)
    }
    fn command(&self, command: &str) -> Output {
        Command::new(env!("CARGO_BIN_EXE_hiko-cli"))
            .current_dir(&self.0)
            .args([command, "main.hml"])
            .output()
            .unwrap()
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn a_library_cannot_inherit_its_consumers_dependencies() {
    let project = Project::new(Some("[]"));
    for command in ["check", "run"] {
        let output = project.command(command);
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "{command} unexpectedly succeeded");
        assert!(
            error.contains(
                "package Acme cannot import Other.Hidden: declare Other as its dependency"
            ),
            "{command}: {error}"
        );
    }
}

#[test]
fn a_library_can_import_its_declared_dependency_offline() {
    let project = Project::new(Some("['Other']"));
    for command in ["check", "run"] {
        let output = project.command(command);
        assert!(
            output.status.success(),
            "{command}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn missing_dependency_metadata_fails_closed() {
    let project = Project::new(None);
    let output = project.command("check");
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "old metadata must not confer root authority"
    );
    assert!(error.contains("dependencies"), "{error}");
}
