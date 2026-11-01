//! Host-facing helpers for output, stdin, and `exec`.

use smallvec::smallvec;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use super::*;
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

pub(super) struct ResolvedExec {
    display_command: String,
    allowed_path: ExecAllowedPath,
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

fn exec_allowed_path(path: PathBuf) -> Result<ExecAllowedPath, String> {
    let metadata = path
        .metadata()
        .map_err(|e| format!("cannot stat executable '{}': {e}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("executable '{}' is not a file", path.display()));
    }
    Ok(ExecAllowedPath {
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
    })
}

fn resolve_exec_command(command: &str) -> Result<ExecAllowedPath, String> {
    resolve_exec_command_path(command).and_then(exec_allowed_path)
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
        let mut resolved = Vec::new();
        let mut errors = Vec::new();
        for command in &allowed {
            match resolve_exec_command(command) {
                Ok(path) => resolved.push(path),
                Err(err) => errors.push(format!("{command}: {err}")),
            }
        }
        self.exec_allowed = allowed;
        self.exec_allowed_paths = resolved;
        self.exec_allowed_resolution_errors = errors;
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
                    #[cfg(feature = "builtin-aws-config")]
                    crate::value::HostHandleKind::AwsConfig => "<aws-config>".to_string(),
                    #[cfg(feature = "builtin-aws-s3")]
                    crate::value::HostHandleKind::AwsS3Client => "<aws-s3-client>".to_string(),
                    #[cfg(feature = "builtin-aws-sqs")]
                    crate::value::HostHandleKind::AwsSqsClient => "<aws-sqs-client>".to_string(),
                    crate::value::HostHandleKind::Unsupported => "<host-handle>".to_string(),
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

    pub fn clear_output_sink(&mut self) {
        self.output_sink = None;
    }

    /// Extract, resolve, and authorize an exec call before spawning.
    pub(super) fn prepare_exec(&self, arg: Value) -> Result<ResolvedExec, RuntimeError> {
        let (command, args) = extract_exec_command_and_args(&self.heap, arg)
            .map_err(|message| RuntimeError { message })?;
        let resolved_path = resolve_exec_command(&command).map_err(|err| RuntimeError {
            message: format!("exec: {err}"),
        })?;
        if let Some(allowed_path) = self
            .exec_allowed_paths
            .iter()
            .find(|allowed| **allowed == resolved_path)
            .cloned()
        {
            Ok(ResolvedExec {
                display_command: command,
                allowed_path,
                args,
            })
        } else {
            let mut message = format!(
                "exec: command '{}' resolved to '{}' which is not in the allowed list: {:?}",
                command,
                resolved_path.path.display(),
                self.exec_allowed
            );
            if !self.exec_allowed_resolution_errors.is_empty() {
                message.push_str(&format!(
                    " (unresolved configured commands: {:?})",
                    self.exec_allowed_resolution_errors
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

        let current_path = exec_allowed_path(exec.allowed_path.path.clone())?;
        if current_path != exec.allowed_path {
            return Err(format!(
                "exec: '{}' changed since it was allowed",
                exec.allowed_path.path.display()
            ));
        }

        let mut command = Command::new(&exec.allowed_path.path);
        command
            .args(&exec.args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt as _;
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
}
