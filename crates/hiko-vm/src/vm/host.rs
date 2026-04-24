//! Host-facing helpers for output, stdin, and `exec`.

use smallvec::smallvec;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::*;

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
    resolved_path: PathBuf,
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
            HeapObject::String(s) => s.clone(),
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
                            HeapObject::String(s) => args.push(s.clone()),
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

fn canonicalize_exec_candidate(path: &Path) -> Option<PathBuf> {
    let metadata = path.metadata().ok()?;
    if !metadata.is_file() {
        return None;
    }
    std::fs::canonicalize(path).ok()
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
            match resolve_exec_command_path(command) {
                Ok(path) => resolved.push(path),
                Err(err) => errors.push(format!("{command}: {err}")),
            }
        }
        self.exec_allowed = allowed;
        self.exec_allowed_paths = resolved;
        self.exec_allowed_resolution_errors = errors;
    }

    /// Set the timeout for exec calls (in seconds).
    pub fn set_exec_timeout(&mut self, timeout: u64) {
        self.exec_timeout = timeout;
    }

    /// Format a value for display in `print` / `println`.
    pub(super) fn display_value(&self, v: &Value) -> String {
        match v {
            Value::Builtin(id) => {
                format!("<builtin:{}>", self.builtins[*id as usize].name.as_ref())
            }
            Value::Pid(pid) => format!("<pid {pid}>"),
            Value::Heap(r) => match self.heap.get(*r) {
                Ok(HeapObject::String(s)) => s.clone(),
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
                Ok(HeapObject::Rng { .. }) => "<rng>".to_string(),
                Ok(HeapObject::Closure { .. }) => "<fn>".to_string(),
                Ok(HeapObject::Continuation { .. }) => "<continuation>".to_string(),
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
        let resolved_path = resolve_exec_command_path(&command).map_err(|err| RuntimeError {
            message: format!("exec: {err}"),
        })?;
        if self
            .exec_allowed_paths
            .iter()
            .any(|path| path == &resolved_path)
        {
            Ok(ResolvedExec {
                display_command: command,
                resolved_path,
                args,
            })
        } else {
            let mut message = format!(
                "exec: command '{}' resolved to '{}' which is not in the allowed list: {:?}",
                command,
                resolved_path.display(),
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

    /// Execute a previously authorized command with timeout.
    pub(super) fn run_exec(&mut self, exec: ResolvedExec) -> Result<Value, String> {
        use std::io::Read as _;
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};

        let mut child = Command::new(&exec.resolved_path)
            .args(&exec.args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("exec: {e}"))?;

        let mut child_stdout = child.stdout.take().unwrap();
        let mut child_stderr = child.stderr.take().unwrap();

        let stdout_handle = std::thread::spawn(move || {
            let mut buf = String::new();
            child_stdout.read_to_string(&mut buf).ok();
            buf
        });
        let stderr_handle = std::thread::spawn(move || {
            let mut buf = String::new();
            child_stderr.read_to_string(&mut buf).ok();
            buf
        });

        let deadline = Instant::now() + Duration::from_secs(self.exec_timeout);
        let status = loop {
            match child.try_wait().map_err(|e| format!("exec: {e}"))? {
                Some(status) => break status,
                None if Instant::now() >= deadline => {
                    child.kill().ok();
                    child.wait().ok();
                    return Err(format!(
                        "exec: '{}' timed out after {}s",
                        exec.display_command, self.exec_timeout
                    ));
                }
                None => std::thread::sleep(Duration::from_millis(50)),
            }
        };

        let stdout_str = stdout_handle.join().unwrap_or_default();
        let stderr_str = stderr_handle.join().unwrap_or_default();
        self.heap
            .charge_io_bytes((stdout_str.len() + stderr_str.len()) as u64)
            .map_err(|e| format!("exec: {e}"))?;

        let exit_code = Value::Int(status.code().unwrap_or(-1) as i64);
        let stdout = Value::Heap(
            self.heap
                .alloc(HeapObject::String(stdout_str))
                .map_err(|e| e.to_string())?,
        );
        let stderr = Value::Heap(
            self.heap
                .alloc(HeapObject::String(stderr_str))
                .map_err(|e| e.to_string())?,
        );

        self.heap
            .alloc(HeapObject::Tuple(smallvec![exit_code, stdout, stderr]))
            .map(Value::Heap)
            .map_err(|e| e.to_string())
    }
}
