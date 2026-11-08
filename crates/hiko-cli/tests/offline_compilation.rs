//! Compilation observes registries and the module cache without changing either.
#![cfg(feature = "cli-lock")]

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime};

use hiko_common::blake3_hex;
use hiko_compile::compiler::module_cache_path;

const MODULE: &str = "structure Answer = struct val value = 42 end\n";

struct Registry {
    url: String,
    requests: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Registry {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (count, stopping) = (requests.clone(), stop.clone());
        let package = format!(
            "name = \"Pkg\"\nversion = \"0.1.0\"\n[modules]\nAnswer = \"blake3:{}\"\n",
            blake3_hex(MODULE.as_bytes())
        );
        let worker = thread::spawn(move || {
            while !stopping.load(Ordering::SeqCst) {
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1));
                        continue;
                    }
                    Err(e) => panic!("registry accept: {e}"),
                };
                count.fetch_add(1, Ordering::SeqCst);
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(&stream);
                let mut request = String::new();
                reader.read_line(&mut request).unwrap();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                        break;
                    }
                }
                let body = match request.split_whitespace().nth(1) {
                    Some("/Pkg-v0.1.0/package.toml") => package.as_str(),
                    Some("/Pkg-v0.1.0/modules/Answer.hml") => MODULE,
                    other => panic!("unexpected registry path: {other:?}"),
                };
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
        });
        Self {
            url,
            requests,
            stop,
            worker: Some(worker),
        }
    }

    fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            let result = worker.join();
            if !thread::panicking() {
                result.unwrap();
            }
        }
    }
}

impl Drop for Registry {
    fn drop(&mut self) {
        self.stop();
    }
}

struct Project {
    root: PathBuf,
    registry: Registry,
    cached: PathBuf,
}

impl Project {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("hiko-offline-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let registry = Registry::new();
        fs::write(root.join("hiko.toml"), format!(
            "[defaults]\npolicy = \"deny\"\nlib_cache = \"cache\"\n[policies.deny]\npath = \"deny.toml\"\n[registries.test]\nurl = {:?}\n[dependencies]\nPkg = {{ version = \"0.1.0\", registry = \"test\" }}\n", registry.url
        )).unwrap();
        // No HTTP, filesystem or other optional host capabilities are granted.
        fs::write(root.join("deny.toml"), "").unwrap();
        fs::write(
            root.join("main.hml"),
            "import Pkg.Answer\nval answer = Answer.value + 1\n",
        )
        .unwrap();
        let hash = blake3_hex(MODULE.as_bytes());
        let base = format!("{}/Pkg-v0.1.0", registry.url);
        fs::write(root.join("hiko.lock.toml"), format!(
            "schema_version = 1\n[packages.Pkg]\nversion = \"0.1.0\"\nbase_url = {base:?}\n[packages.Pkg.modules]\nAnswer = \"blake3:{hash}\"\n"
        )).unwrap();
        let cached = module_cache_path(
            &root.join("cache"),
            &format!("{base}/modules/Answer.hml"),
            &hash,
        );
        Self {
            root,
            registry,
            cached,
        }
    }

    fn hiko(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_hiko-cli"))
            .current_dir(&self.root)
            .env("NO_COLOR", "1")
            .args(args)
            .output()
            .unwrap()
    }

    fn compile_commands(&self) -> [&[&str]; 3] {
        [
            &["run", "main.hml"],
            &["check", "main.hml"],
            &[
                "build-vm",
                "deny.toml",
                "--lock",
                "hiko.lock.toml",
                "--script",
                "main.hml",
                "--out",
                "artifact",
            ],
        ]
    }

    fn assert_compilation_refused_without_io(&self, detail: &str) {
        for args in self.compile_commands() {
            let before = snapshot(&self.root.join("cache"));
            let connections = self.registry.requests.load(Ordering::SeqCst);
            let output = self.hiko(args);
            assert_eq!(
                self.registry.requests.load(Ordering::SeqCst),
                connections,
                "{args:?} contacted registry"
            );
            assert_eq!(
                snapshot(&self.root.join("cache")),
                before,
                "{args:?} changed cache"
            );
            assert!(!output.status.success(), "{args:?} unexpectedly succeeded");
            let message = String::from_utf8_lossy(&output.stderr);
            for required in [
                "Pkg.Answer",
                self.cached.to_str().unwrap(),
                "hiko install",
                detail,
            ] {
                assert!(
                    message.contains(required),
                    "{args:?}: missing {required:?}: {message}"
                );
            }
        }
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}

// Include directories, contents and modification times, so an absent or empty
// cache and a write of identical bytes remain observable.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, (Option<Vec<u8>>, SystemTime)> {
    fn visit(path: &Path, result: &mut BTreeMap<PathBuf, (Option<Vec<u8>>, SystemTime)>) {
        if !path.exists() {
            return;
        }
        let metadata = fs::metadata(path).unwrap();
        let data = if metadata.is_dir() {
            None
        } else {
            Some(fs::read(path).unwrap())
        };
        result.insert(path.to_path_buf(), (data, metadata.modified().unwrap()));
        if metadata.is_dir() {
            for entry in fs::read_dir(path).unwrap() {
                visit(&entry.unwrap().path(), result);
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(root, &mut result);
    result
}

#[test]
fn missing_cache_is_offline_even_with_host_io_denied() {
    let project = Project::new("missing");
    project.assert_compilation_refused_without_io("missing");
    fs::create_dir(project.root.join("cache")).unwrap();
    project.assert_compilation_refused_without_io("missing");
    assert_eq!(project.registry.requests.load(Ordering::SeqCst), 0);
}

#[test]
fn corrupt_cache_is_reported_and_preserved_without_registry_io() {
    let project = Project::new("corrupt");
    fs::create_dir(project.root.join("cache")).unwrap();
    fs::write(
        &project.cached,
        "structure Answer = struct val value = 0 end\n",
    )
    .unwrap();
    project.assert_compilation_refused_without_io("BLAKE3");
    assert_eq!(project.registry.requests.load(Ordering::SeqCst), 0);
}

#[test]
fn install_repairs_cache_without_repinning_and_compilation_works_with_registry_stopped() {
    let mut project = Project::new("installed");
    let lockfile = project.root.join("hiko.lock.toml");
    let pins = fs::read(&lockfile).unwrap();
    enum CacheState {
        New,
        Missing,
        Corrupt,
    }
    for damage in [CacheState::New, CacheState::Missing, CacheState::Corrupt] {
        match damage {
            CacheState::New => {}
            CacheState::Missing => fs::remove_file(&project.cached).unwrap(),
            CacheState::Corrupt => fs::write(&project.cached, "tampered source").unwrap(),
        }
        let output = project.hiko(&["install"]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(fs::read(&lockfile).unwrap(), pins);
        assert_eq!(fs::read_to_string(&project.cached).unwrap(), MODULE);
    }
    assert!(project.registry.requests.load(Ordering::SeqCst) > 0);
    project.registry.stop();
    let before = snapshot(&project.root.join("cache"));
    for args in project.compile_commands() {
        let output = project.hiko(args);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            snapshot(&project.root.join("cache")),
            before,
            "{args:?} wrote cache"
        );
        assert_eq!(fs::read(&lockfile).unwrap(), pins);
    }
    let embedded = fs::read_to_string(project.root.join("artifact/src/embedded.rs")).unwrap();
    assert!(
        embedded.contains("Pkg.Answer"),
        "sealed artifact omitted the module"
    );
}
