//! Host-facing helpers for output, stdin, and `exec`.

use smallvec::smallvec;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use super::*;
use crate::builder::ExecArgvRule;
use crate::io_limits::ReadLimit;

pub trait OutputSink: Send + Sync {
    fn write(&self, text: &str) -> std::io::Result<()>;
}

#[derive(Default)]
pub struct StdoutOutputSink {
    lock: Mutex<()>,
}

impl OutputSink for StdoutOutputSink {
    fn write(&self, text: &str) -> std::io::Result<()> {
        use std::io::Write as _;

        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut stdout = std::io::stdout().lock();
        stdout.write_all(text.as_bytes())?;
        stdout.flush()
    }
}

/// The host's stderr, for `eprint` and `eprintln`.
#[derive(Default)]
pub struct StderrOutputSink;

impl OutputSink for StderrOutputSink {
    fn write(&self, text: &str) -> std::io::Result<()> {
        use std::io::Write as _;

        // Stderr is unbuffered; one write per call keeps a line whole.
        std::io::stderr().lock().write_all(text.as_bytes())
    }
}

pub(super) struct ResolvedExec {
    display_command: String,
    allowed: AllowedExecutable,
    args: Vec<String>,
}

fn extract_exec_command_and_args(heap: &Heap, arg: Value) -> Result<(String, Vec<String>), String> {
    let (v0, v1) = match arg {
        Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
            HeapObject::Tuple(t) if t.len() >= 2 => (t[0], t[1]),
            _ => return Err("exec: expected (String, String list)".into()),
        },
        _ => return Err("exec: expected (String, String list)".into()),
    };

    let command = match v0 {
        Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => s.to_string(),
            _ => return Err("exec: expected String for command".into()),
        },
        _ => return Err("exec: expected String for command".into()),
    };

    let mut args = Vec::new();
    let mut cur = v1;
    loop {
        match cur {
            Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
                HeapObject::Data { tag, .. } if *tag == TAG_NIL => break,
                HeapObject::Data { tag, fields } if *tag == TAG_CONS && fields.len() == 2 => {
                    match fields[0] {
                        Value::Heap(sr) => match heap.get(sr).map_err(|e| e.to_string())? {
                            HeapObject::String(s) => args.push(s.to_string()),
                            _ => return Err("exec: args must be strings".into()),
                        },
                        _ => return Err("exec: args must be strings".into()),
                    }
                    cur = fields[1];
                }
                _ => return Err("exec: expected String list for args".into()),
            },
            _ => return Err("exec: expected String list for args".into()),
        }
    }

    Ok((command, args))
}

fn command_has_explicit_path(command: &str) -> bool {
    let path = Path::new(command);
    path.is_absolute() || path.components().count() > 1
}

fn read_exec_pipe<R: std::io::Read>(
    reader: R,
    limit: ReadLimit,
    stream: &str,
) -> Result<String, String> {
    let context = format!("exec: {stream}");
    let (buf, _) = crate::io_backend::read_to_bytes_limited(reader, limit, &context)?;
    String::from_utf8(buf).map_err(|e| format!("{context}: {e}"))
}

/// Kill an `exec` child and every process in its process group.
///
/// Must be called before the child is reaped: while the group leader is
/// unreaped its pid, and so the group id, cannot be reused by another process.
fn kill_exec_child(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        use rustix::process::{Pid, Signal, kill_process_group};
        // The child leads its own group (`process_group(0)`), so its pid is
        // the group id.
        kill_process_group(Pid::from_child(child), Signal::KILL).ok();
    }
    child.kill().ok();
    child.wait().ok();
}

fn canonicalize_exec_candidate(path: &Path) -> Option<PathBuf> {
    let metadata = path.metadata().ok()?;
    if !metadata.is_file() {
        return None;
    }
    std::fs::canonicalize(path).ok()
}

/// The identity of the file at `path` now.
fn file_identity(path: PathBuf) -> Result<FileIdentity, String> {
    let metadata = path
        .metadata()
        .map_err(|e| format!("cannot stat executable '{}': {e}", path.display()))?;
    identity_from(path, &metadata)
}

/// Open the executable at `path` and take its identity from the open file,
/// which stays open. Only an executable the VM may run but not read (mode
/// `--x`) is allowed by identity alone, as before #160. Any other open
/// error leaves the command out of the policy, with the error recorded.
fn allowed_executable(path: PathBuf) -> Result<AllowedExecutable, String> {
    match std::fs::File::open(&path) {
        Ok(file) => {
            let metadata = file
                .metadata()
                .map_err(|e| format!("cannot stat executable '{}': {e}", path.display()))?;
            Ok(AllowedExecutable {
                id: identity_from(path, &metadata)?,
                pin: Some(Arc::new(file)),
            })
        }
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => Ok(AllowedExecutable {
            id: file_identity(path)?,
            pin: None,
        }),
        Err(e) => Err(format!("cannot open executable '{}': {e}", path.display())),
    }
}

fn identity_from(path: PathBuf, metadata: &std::fs::Metadata) -> Result<FileIdentity, String> {
    if !metadata.is_file() {
        return Err(format!("executable '{}' is not a file", path.display()));
    }
    Ok(FileIdentity {
        path,
        #[cfg(unix)]
        dev: {
            use std::os::unix::fs::MetadataExt as _;
            metadata.dev()
        },
        #[cfg(unix)]
        ino: {
            use std::os::unix::fs::MetadataExt as _;
            metadata.ino()
        },
        #[cfg(unix)]
        ctime: {
            use std::os::unix::fs::MetadataExt as _;
            (metadata.ctime(), metadata.ctime_nsec())
        },
        len: metadata.len(),
    })
}

/// The path to run an allowed executable from. On Linux that is the pinned
/// file itself, through `/proc/self/fd`, so a file swapped in at the path
/// after the identity check cannot run. A script (`#!`) is the exception:
/// its interpreter opens the path after exec, when the descriptor is closed.
/// Elsewhere, and without `/proc`, it is the path, checked just before.
fn exec_path(allowed: &AllowedExecutable) -> PathBuf {
    #[cfg(target_os = "linux")]
    if let Some(pin) = &allowed.pin {
        use std::os::fd::AsRawFd as _;
        use std::os::unix::fs::FileExt as _;
        let mut magic = [0u8; 2];
        let is_script = pin
            .read_at(&mut magic, 0)
            .is_ok_and(|n| n == 2 && &magic == b"#!");
        let fd_path = PathBuf::from(format!("/proc/self/fd/{}", pin.as_raw_fd()));
        if !is_script && fd_path.exists() {
            return fd_path;
        }
    }
    allowed.id.path.clone()
}

fn resolve_exec_command(command: &str) -> Result<FileIdentity, String> {
    resolve_exec_command_path(command).and_then(file_identity)
}

fn resolve_exec_command_path(command: &str) -> Result<PathBuf, String> {
    let path = Path::new(command);
    if command_has_explicit_path(command) {
        return canonicalize_exec_candidate(path)
            .ok_or_else(|| format!("cannot resolve executable '{}'", path.display()));
    }

    let path_env = std::env::var_os("PATH")
        .ok_or_else(|| format!("cannot resolve executable '{command}': PATH is not set"))?;

    for dir in std::env::split_paths(&path_env) {
        if let Some(resolved) = canonicalize_exec_candidate(&dir.join(command)) {
            return Ok(resolved);
        }

        #[cfg(windows)]
        {
            let pathext =
                std::env::var_os("PATHEXT").unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".into());
            for ext in pathext
                .to_string_lossy()
                .split(';')
                .filter(|s| !s.is_empty())
            {
                let trimmed = ext.trim_start_matches('.');
                if let Some(resolved) =
                    canonicalize_exec_candidate(&dir.join(format!("{command}.{trimmed}")))
                {
                    return Ok(resolved);
                }
            }
        }
    }

    Err(format!(
        "cannot resolve executable '{command}' from the current PATH"
    ))
}

impl VM {
    /// Set the allowed commands for the exec builtin.
    pub fn set_exec_allowed(&mut self, allowed: Vec<String>) {
        self.set_exec_policy(allowed, Vec::new());
    }

    /// Set the commands `exec` may run with any arguments, and the argument
    /// rules. Each command is resolved to a file and opened once, now; a call
    /// must resolve to the same file.
    pub fn set_exec_policy(&mut self, allowed: Vec<String>, rules: Vec<ExecArgvRule>) {
        let any_arguments = allowed.into_iter().map(|command| ExecArgvRule {
            argv: vec![command],
            rest_any: true,
        });
        let mut opened: HashMap<String, Result<AllowedExecutable, String>> = HashMap::new();
        let mut policy = ResolvedExecPolicy::default();
        for rule in any_arguments.chain(rules) {
            let Some(command) = rule.argv.first() else {
                policy.errors.push("an argv rule is empty".to_string());
                continue;
            };
            let executable = opened.entry(command.clone()).or_insert_with(|| {
                let resolved = resolve_exec_command_path(command).and_then(allowed_executable);
                if let Err(err) = &resolved {
                    policy.errors.push(format!("{command}: {err}"));
                }
                resolved
            });
            if let Ok(executable) = executable {
                policy.rules.push((rule.clone(), executable.clone()));
            }
        }
        self.exec_policy = Arc::new(policy);
    }

    /// Format a value for display in `print` / `println`.
    pub(super) fn display_value(&self, v: &Value) -> String {
        match v {
            Value::Builtin(id) => {
                format!(
                    "<builtin:{}>",
                    self.builtins.entries[*id as usize].name.as_ref()
                )
            }
            Value::Word(w) => format!("0w{w}"),
            Value::Pid(pid) => format!("<pid {pid}>"),
            Value::Heap(r) => match self.heap.get(*r) {
                Ok(HeapObject::String(s)) => s.to_string(),
                Ok(HeapObject::Tuple(elems)) => {
                    let parts: Vec<String> = elems.iter().map(|e| self.display_value(e)).collect();
                    format!("({})", parts.join(", "))
                }
                Ok(HeapObject::Data { tag, fields }) => {
                    if *tag == TAG_NIL && fields.is_empty() {
                        "[]".to_string()
                    } else if *tag == TAG_CONS && fields.len() == 2 {
                        let mut parts = vec![self.display_value(&fields[0])];
                        let mut tail = fields[1];
                        loop {
                            match tail {
                                Value::Heap(r2) => match self.heap.get(r2) {
                                    Ok(HeapObject::Data { tag: t, fields: f })
                                        if *t == TAG_NIL && f.is_empty() =>
                                    {
                                        break;
                                    }
                                    Ok(HeapObject::Data { tag: t, fields: f })
                                        if *t == TAG_CONS && f.len() == 2 =>
                                    {
                                        parts.push(self.display_value(&f[0]));
                                        tail = f[1];
                                    }
                                    _ => {
                                        parts.push(self.display_value(&tail));
                                        break;
                                    }
                                },
                                _ => {
                                    parts.push(self.display_value(&tail));
                                    break;
                                }
                            }
                        }
                        format!("[{}]", parts.join(", "))
                    } else {
                        format!("Data({tag})")
                    }
                }
                Ok(HeapObject::Bytes(b)) => format!("<bytes:{}>", b.len()),
                Ok(HeapObject::HostHandle { kind, .. }) => match kind {
                    crate::value::HostHandleKind::Unsupported => "<host-handle>".to_string(),
                    crate::value::HostHandleKind::Extension(kind) => format!("<{kind}>"),
                },
                Ok(HeapObject::Rng { .. }) => "<rng>".to_string(),
                Ok(HeapObject::Closure { .. }) => "<fn>".to_string(),
                Err(_) => "<dangling ref>".to_string(),
            },
            other => other.to_string(),
        }
    }

    /// Inject stdin content for embedders running the VM in-process.
    pub fn set_stdin_override(&mut self, input: String) {
        self.heap.set_stdin_override(input);
    }

    pub fn set_output_sink(&mut self, sink: Arc<dyn OutputSink>) {
        self.output_sink = Some(sink);
    }

    /// Where `eprint` and `eprintln` write, as `set_output_sink` is for
    /// `print` and `println`. Without one their output is dropped. Child
    /// processes inherit it.
    pub fn set_error_sink(&mut self, sink: Arc<dyn OutputSink>) {
        self.error_sink = Some(sink);
    }

    pub fn clear_output_sink(&mut self) {
        self.output_sink = None;
    }

    /// Extract, resolve, and authorize an exec call before spawning.
    pub(super) fn prepare_exec(&self, arg: Value) -> Result<ResolvedExec, RuntimeError> {
        let (command, args) = extract_exec_command_and_args(&self.heap, arg)
            .map_err(|message| RuntimeError { message })?;
        if command.contains('\0') || args.iter().any(|arg| arg.contains('\0')) {
            return Err(RuntimeError {
                message: "exec: the command and arguments must not contain NUL".into(),
            });
        }
        let resolved = resolve_exec_command(&command).map_err(|err| RuntimeError {
            message: format!("exec: {err}"),
        })?;
        let allowed = self
            .exec_policy
            .rules
            .iter()
            .find(|(rule, executable)| executable.id == resolved && rule.args_match(&args));
        if let Some((_, allowed)) = allowed {
            Ok(ResolvedExec {
                display_command: command,
                allowed: allowed.clone(),
                args,
            })
        } else if let Some((_, changed)) =
            self.exec_policy.rules.iter().find(|(rule, executable)| {
                rule.args_match(&args)
                    && executable.id != resolved
                    && executable.id.same_file(&resolved)
            })
        {
            Err(RuntimeError {
                message: format!(
                    "exec: '{}' changed after the exec policy was applied (it was written, replaced, or its permissions changed); it is refused until the VM is started again",
                    changed.id.path.display()
                ),
            })
        } else {
            let rules: Vec<String> = self
                .exec_policy
                .rules
                .iter()
                .map(|(rule, _)| {
                    let rest = if rule.rest_any { " ..." } else { "" };
                    format!("{}{rest}", rule.argv.join(" "))
                })
                .collect();
            // The arguments are not shown: they can hold secrets, such as a
            // token, and refusals end up in logs.
            let mut message = format!(
                "exec: command '{}' resolved to '{}' with {} argument(s) matches no exec rule; the rules are: {:?}",
                command,
                resolved.path.display(),
                args.len(),
                rules
            );
            if !self.exec_policy.errors.is_empty() {
                message.push_str(&format!(
                    " (unresolved configured commands: {:?})",
                    self.exec_policy.errors
                ));
            }
            Err(RuntimeError { message })
        }
    }

    /// Execute a previously authorized command with a wall-clock timeout and
    /// per-stream output limit. On timeout or overflow the child's whole
    /// process group is killed.
    pub(super) fn run_exec(&mut self, exec: ResolvedExec) -> Result<Value, String> {
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};

        let allowed = &exec.allowed.id;
        if file_identity(allowed.path.clone())? != *allowed {
            return Err(format!(
                "exec: '{}' changed since it was allowed",
                allowed.path.display()
            ));
        }

        let mut command = Command::new(exec_path(&exec.allowed));
        command
            .args(&exec.args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt as _;
            // The program sees the path it was allowed by, not /proc/self/fd/N.
            command.arg0(&allowed.path);
            // Lead a new process group so a timeout can kill grandchildren too.
            command.process_group(0);
        }
        let mut child = command.spawn().map_err(|e| format!("exec: {e}"))?;

        let timeout = self.heap.io_limits().exec_timeout;
        let output_limit = self.heap.exec_output_limit();
        let (tx, rx) = mpsc::channel();
        for (stream, is_stdout) in [("stdout", true), ("stderr", false)] {
            let tx = tx.clone();
            let pipe: Box<dyn std::io::Read + Send> = if is_stdout {
                Box::new(child.stdout.take().expect("stdout is piped"))
            } else {
                Box::new(child.stderr.take().expect("stderr is piped"))
            };
            std::thread::spawn(move || {
                let _ = tx.send((is_stdout, read_exec_pipe(pipe, output_limit, stream)));
            });
        }
        drop(tx);

        let deadline = Instant::now() + timeout;
        let timed_out = |child: &mut std::process::Child| {
            kill_exec_child(child);
            format!(
                "exec: '{}' timed out after {}ms",
                exec.display_command,
                timeout.as_millis()
            )
        };

        // Read both streams to EOF first. The child is not reaped during this
        // phase, so `kill_exec_child` can still safely signal its group.
        let mut stdout_str = None;
        let mut stderr_str = None;
        while stdout_str.is_none() || stderr_str.is_none() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(remaining) {
                Ok((is_stdout, Ok(text))) => {
                    if is_stdout {
                        stdout_str = Some(text);
                    } else {
                        stderr_str = Some(text);
                    }
                }
                Ok((_, Err(err))) => {
                    kill_exec_child(&mut child);
                    return Err(err);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => return Err(timed_out(&mut child)),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    kill_exec_child(&mut child);
                    return Err("exec: output reader thread panicked".into());
                }
            }
        }
        let (stdout_str, stderr_str) = (
            stdout_str.unwrap_or_default(),
            stderr_str.unwrap_or_default(),
        );

        let status = loop {
            match child.try_wait().map_err(|e| format!("exec: {e}"))? {
                Some(status) => break status,
                None if Instant::now() >= deadline => return Err(timed_out(&mut child)),
                None => std::thread::sleep(Duration::from_millis(10)),
            }
        };

        self.heap
            .charge_io_bytes((stdout_str.len() + stderr_str.len()) as u64)
            .map_err(|e| format!("exec: {e}"))?;

        let exit_code = Value::Int(status.code().unwrap_or(-1) as i64);
        let stdout = Value::Heap(
            self.heap
                .alloc(HeapObject::String(stdout_str.into()))
                .map_err(|e| e.to_string())?,
        );
        let stderr = Value::Heap(
            self.heap
                .alloc(HeapObject::String(stderr_str.into()))
                .map_err(|e| e.to_string())?,
        );

        self.heap
            .alloc(HeapObject::Tuple(smallvec![exit_code, stdout, stderr]))
            .map(Value::Heap)
            .map_err(|e| e.to_string())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("hiko-host-{prefix}-{nanos}"));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn write_executable(path: &Path, body: &str) {
        std::fs::write(path, body).expect("write executable");
        let mut perms = std::fs::metadata(path).expect("metadata").permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).expect("chmod executable");
    }

    const LIMIT_3: ReadLimit = ReadLimit {
        max_bytes: 3,
        limit_name: "max_exec_output_bytes",
    };

    #[test]
    fn read_exec_pipe_rejects_output_over_limit() {
        let err = read_exec_pipe(std::io::Cursor::new("abcd"), LIMIT_3, "stdout").unwrap_err();
        assert_eq!(
            err,
            "exec: stdout: result exceeds max_exec_output_bytes (3 bytes)"
        );
    }

    #[test]
    fn read_exec_pipe_allows_output_at_limit() {
        let text = read_exec_pipe(std::io::Cursor::new("abc"), LIMIT_3, "stdout").unwrap();
        assert_eq!(text, "abc");
    }

    #[test]
    fn exec_identity_changes_when_allowed_path_is_replaced() {
        let root = temp_dir("exec-identity");
        let script = root.join("tool.sh");
        write_executable(&script, "#!/bin/sh\necho first\n");
        let first =
            resolve_exec_command(script.to_str().expect("utf8 path")).expect("first identity");

        std::fs::remove_file(&script).expect("remove executable");
        write_executable(&script, "#!/bin/sh\necho second\n");
        let second =
            resolve_exec_command(script.to_str().expect("utf8 path")).expect("second identity");

        assert_eq!(first.path, second.path);
        assert_ne!(first, second);

        let _ = std::fs::remove_dir_all(root);
    }

    /// #160: while the allowed file is held open its inode cannot be reused,
    /// so a same-size replacement made within one timestamp tick still has
    /// a different identity. Nothing here depends on timing.
    #[test]
    fn pinned_executable_detects_a_same_size_replacement() {
        let root = temp_dir("exec-pinned");
        let script = root.join("tool.sh");
        write_executable(&script, "#!/bin/sh\necho first\n");
        let allowed = allowed_executable(script.clone()).expect("pinned identity");
        assert!(allowed.pin.is_some());

        for round in 0..50 {
            std::fs::remove_file(&script).expect("remove executable");
            write_executable(&script, "#!/bin/sh\necho secnd\n");
            let now = file_identity(script.clone()).expect("current identity");
            assert_ne!(allowed.id.ino, now.ino, "round {round}: inode reused");
            assert_ne!(allowed.id, now, "round {round}");
        }

        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(feature = "builtin-exec")]
    #[test]
    fn exec_says_when_an_allowed_file_changed_in_place() {
        use crate::builder::{ExecPolicy, VMBuilder};

        let root = temp_dir("exec-changed");
        let script = root.join("tool.sh");
        write_executable(&script, "#!/bin/sh\necho first\n");
        let program = crate::test_support::compile_source_hermetic(&format!(
            "val _ = exec ({:?}, [])",
            script.to_string_lossy()
        ));
        let mut vm = VMBuilder::new(program)
            .with_core()
            .with_exec(ExecPolicy {
                allowed: vec![script.to_string_lossy().into_owned()],
                rules: Vec::new(),
            })
            .build();

        // A write in place keeps the inode and changes the size.
        use std::io::Write as _;
        std::fs::OpenOptions::new()
            .append(true)
            .open(&script)
            .unwrap()
            .write_all(b"echo more\n")
            .unwrap();
        let err = vm.run().expect_err("a changed file must not run");
        assert!(
            err.message
                .contains("changed after the exec policy was applied"),
            "{}",
            err.message
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(feature = "builtin-exec")]
    #[test]
    fn exec_refuses_an_allowed_file_replaced_after_the_policy() {
        use crate::builder::{ExecPolicy, VMBuilder};

        let root = temp_dir("exec-replaced");
        let script = root.join("tool.sh");
        write_executable(&script, "#!/bin/sh\necho first\n");
        let program = crate::test_support::compile_source_hermetic(&format!(
            "val _ = exec ({:?}, [])",
            script.to_string_lossy()
        ));
        let mut vm = VMBuilder::new(program)
            .with_core()
            .with_exec(ExecPolicy {
                allowed: vec![script.to_string_lossy().into_owned()],
                rules: Vec::new(),
            })
            .build();

        std::fs::remove_file(&script).expect("remove executable");
        write_executable(&script, "#!/bin/sh\necho secnd\n");
        let err = vm.run().expect_err("the replaced file must not run");
        assert!(
            err.message.contains("matches no exec rule"),
            "{}",
            err.message
        );

        let _ = std::fs::remove_dir_all(root);
    }
}
