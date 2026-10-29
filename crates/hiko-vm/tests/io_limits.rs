//! Per-operation I/O timeouts and size limits (`[limits]` in a run policy).

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use hiko_vm::builder::VMBuilder;
use hiko_vm::config::RunConfig;
use hiko_vm::io_limits::IoLimits;
use hiko_vm::vm::VM;

fn compile(source: &str) -> hiko_compile::chunk::CompiledProgram {
    let tokens = hiko_syntax::lexer::Lexer::new(source, 0)
        .tokenize()
        .expect("lex");
    let program = hiko_syntax::parser::Parser::new(tokens)
        .parse_program()
        .expect("parse");
    hiko_compile::compiler::Compiler::compile(program)
        .expect("compile")
        .0
}

fn temp_dir(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "hiko-io-limits-{name}-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A server that accepts one connection and never answers.
fn silent_server(hold: Duration) -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        std::thread::sleep(hold);
        drop(stream);
    });
    (format!("http://{addr}/"), handle)
}

/// A server that answers 200 and streams an unbounded body until the client
/// hangs up.
fn endless_body_server() -> (String, std::thread::JoinHandle<u64>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0u8; 1024];
        let _ = stream.read(&mut request);
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\n")
            .unwrap();
        let chunk = [b'x'; 8192];
        let mut sent = 0u64;
        while stream.write_all(&chunk).is_ok() {
            sent += chunk.len() as u64;
            if sent > 1 << 30 {
                break;
            }
        }
        sent
    });
    (format!("http://{addr}/"), handle)
}

fn http_vm(source: &str) -> VMBuilder {
    VMBuilder::new(compile(source))
        .with_core()
        .allow_http_builtin("http_get", vec!["127.0.0.1".to_string()])
}

#[test]
fn http_get_fails_within_timeout_on_silent_server() {
    let (url, server) = silent_server(Duration::from_secs(4));
    let mut vm = http_vm(&format!("val _ = http_get {url:?}"))
        .http_timeout_ms(1000)
        .build();

    let started = Instant::now();
    let err = vm.run().unwrap_err();
    let elapsed = started.elapsed();

    assert!(err.message.starts_with("http_get:"), "{}", err.message);
    assert!(
        elapsed >= Duration::from_millis(900) && elapsed <= Duration::from_secs(2),
        "timeout took {elapsed:?}"
    );
    server.join().unwrap();
}

#[test]
fn http_response_is_cut_off_at_max_http_response_bytes() {
    const LIMIT: u64 = 64 * 1024;
    let (url, server) = endless_body_server();
    let mut vm = http_vm(&format!("val _ = http_get {url:?}"))
        .max_http_response_bytes(LIMIT)
        .build();

    let err = vm.run().unwrap_err();
    assert!(
        err.message.contains("max_http_response_bytes"),
        "{}",
        err.message
    );
    // Nothing is charged for a rejected body, and the VM heap never held it.
    assert_eq!(vm.io_bytes_used(), 0);
    assert!(
        (vm.heap_peak_bytes() as u64) < LIMIT,
        "heap peak {} bytes",
        vm.heap_peak_bytes()
    );
    let sent = server.join().unwrap();
    assert!(sent < 1 << 30, "client kept reading: {sent} bytes");
}

#[test]
fn read_file_over_max_file_read_bytes_fails() {
    let dir = temp_dir("read-file");
    let path = dir.join("big.txt");
    std::fs::write(&path, vec![b'a'; 4096]).unwrap();
    let mut vm = VMBuilder::new(compile(&format!(
        "val _ = read_file {:?}",
        path.to_string_lossy()
    )))
    .with_core()
    .register_builtin_name("read_file")
    .max_file_read_bytes(1024)
    .build();

    let err = vm.run().unwrap_err();
    assert_eq!(
        err.message,
        "read_file: result exceeds max_file_read_bytes (1024 bytes)"
    );
    assert_eq!(vm.io_bytes_used(), 0);
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(unix)]
#[test]
fn read_file_stops_at_limit_on_endless_file() {
    // /dev/zero never ends, so this only returns if the read stops at the limit.
    let mut vm = VMBuilder::new(compile("val _ = read_file_bytes \"/dev/zero\""))
        .with_core()
        .register_builtin_name("read_file_bytes")
        .max_file_read_bytes(1024)
        .build();
    let err = vm.run().unwrap_err();
    assert!(
        err.message.contains("max_file_read_bytes"),
        "{}",
        err.message
    );
}

#[test]
fn max_io_bytes_still_applies_when_smaller_than_per_op_limit() {
    let dir = temp_dir("io-budget");
    let path = dir.join("f.txt");
    std::fs::write(&path, vec![b'a'; 4096]).unwrap();
    let mut vm = VMBuilder::new(compile(&format!(
        "val _ = read_file {:?}",
        path.to_string_lossy()
    )))
    .with_core()
    .register_builtin_name("read_file")
    .max_io_bytes(100)
    .build();

    let err = vm.run().unwrap_err();
    assert!(
        err.message.contains("max_io_bytes (100 bytes)"),
        "{}",
        err.message
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(unix)]
fn write_script(dir: &std::path::Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    std::fs::write(&path, body).unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

#[cfg(unix)]
fn process_alive(pid: &str) -> bool {
    std::process::Command::new("kill")
        .args(["-0", pid])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(unix)]
fn exec_vm(script: &std::path::Path) -> VMBuilder {
    VMBuilder::new(compile(&format!(
        "val _ = exec ({:?}, [])",
        script.to_string_lossy()
    )))
    .with_core()
    .with_exec(hiko_vm::builder::ExecPolicy {
        allowed: vec![script.to_string_lossy().to_string()],
    })
}

#[cfg(unix)]
#[test]
fn exec_timeout_kills_the_whole_process_group() {
    let dir = temp_dir("exec-timeout");
    let pid_file = dir.join("grandchild.pid");
    // The script starts a grandchild, records its pid, and waits on it.
    let script = write_script(
        &dir,
        "slow.sh",
        &format!(
            "#!/bin/sh\nsleep 60 &\necho $! > {:?}\nwait\n",
            pid_file.to_string_lossy()
        ),
    );
    let mut vm = exec_vm(&script).exec_timeout_ms(500).build();

    let started = Instant::now();
    let err = vm.run().unwrap_err();
    let elapsed = started.elapsed();

    assert!(
        err.message.contains("timed out after 500ms"),
        "{}",
        err.message
    );
    assert!(
        elapsed < Duration::from_millis(1500),
        "timeout took {elapsed:?}"
    );

    let pid = std::fs::read_to_string(&pid_file).unwrap();
    let pid = pid.trim();
    // The killed grandchild is reparented and reaped by init; allow a moment.
    let gone_by = Instant::now() + Duration::from_secs(2);
    while process_alive(pid) && Instant::now() < gone_by {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!process_alive(pid), "grandchild {pid} survived the timeout");
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(unix)]
#[test]
fn exec_output_over_limit_is_cut_off_without_waiting_for_timeout() {
    let dir = temp_dir("exec-output");
    // Writes forever; only a stopped reader ends this before the timeout.
    let script = write_script(&dir, "loud.sh", "#!/bin/sh\nexec yes\n");
    let mut vm = exec_vm(&script)
        .exec_timeout_ms(10_000)
        .max_exec_output_bytes(64 * 1024)
        .build();

    let started = Instant::now();
    let err = vm.run().unwrap_err();
    let elapsed = started.elapsed();

    assert_eq!(
        err.message,
        "exec: stdout: result exceeds max_exec_output_bytes (65536 bytes)"
    );
    assert!(elapsed < Duration::from_secs(5), "took {elapsed:?}");
    assert_eq!(vm.io_bytes_used(), 0);
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(unix)]
#[test]
fn exec_within_limits_still_returns_output() {
    let dir = temp_dir("exec-ok");
    let script = write_script(&dir, "ok.sh", "#!/bin/sh\necho out\necho err >&2\nexit 3\n");
    let program = compile(&format!(
        "val (code, out, err) = exec ({:?}, [])",
        script.to_string_lossy()
    ));
    let mut vm = VMBuilder::new(program)
        .with_core()
        .with_exec(hiko_vm::builder::ExecPolicy {
            allowed: vec![script.to_string_lossy().to_string()],
        })
        .exec_timeout_ms(5000)
        .build();
    vm.run().unwrap();
    assert_eq!(global_int(&vm, "code"), 3);
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(unix)]
fn global_int(vm: &VM, name: &str) -> i64 {
    match vm.get_global(name) {
        Some(hiko_vm::value::Value::Int(n)) => *n,
        other => panic!("{name}: expected Int, got {other:?}"),
    }
}

#[test]
fn policy_without_io_limit_keys_uses_defaults() {
    let config = RunConfig::from_toml("[limits]\nmax_fuel = 1000\n").unwrap();
    let vm = config.build_vm(compile("val x = 1"));
    assert_eq!(vm.io_limits(), IoLimits::default());
}

#[test]
fn policy_io_limit_keys_reach_the_vm() {
    let config = RunConfig::from_toml(
        "[limits]\n\
         http_timeout_ms = 1500\n\
         http_connect_timeout_ms = 700\n\
         exec_timeout_ms = 2500\n\
         max_http_response_bytes = 10\n\
         max_file_read_bytes = 20\n\
         max_exec_output_bytes = 30\n\
         max_concurrent_io = 40\n",
    )
    .unwrap();
    let vm = config.build_vm(compile("val x = 1"));
    assert_eq!(
        vm.io_limits(),
        IoLimits {
            http_timeout: Duration::from_millis(1500),
            http_connect_timeout: Duration::from_millis(700),
            exec_timeout: Duration::from_millis(2500),
            max_http_response_bytes: 10,
            max_file_read_bytes: 20,
            max_exec_output_bytes: 30,
            max_concurrent_io: 40,
        }
    );

    let source = config.to_rust_source();
    for line in [
        ".http_timeout_ms(1500)",
        ".http_connect_timeout_ms(700)",
        ".exec_timeout_ms(2500)",
        ".max_http_response_bytes(10)",
        ".max_file_read_bytes(20)",
        ".max_exec_output_bytes(30)",
        ".max_concurrent_io(40)",
    ] {
        assert!(source.contains(line), "generated source misses {line}");
    }
}

#[test]
fn exec_leaf_no_longer_accepts_timeout_seconds() {
    let err = RunConfig::from_toml(
        "[capabilities.exec.exec]\nenabled = true\nallowed_commands = [\"ls\"]\ntimeout = 30\n",
    )
    .unwrap_err();
    assert!(err.contains("timeout"), "{err}");
}
