//! Host-facing helpers for output, stdin, and `exec`.

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

/// An authorized, pinned command shared with an I/O backend. Clones share
/// one execution and cancellation state; only the VM can prepare a new job.
#[derive(Clone)]
pub struct ExecJob {
    inner: Arc<ExecJobInner>,
}

struct ExecJobInner {
    exec: ResolvedExec,
    timeout: std::time::Duration,
    output_limit: ReadLimit,
    state: Mutex<ExecState>,
    changed: std::sync::Condvar,
}

#[derive(Default)]
struct ExecState {
    started: bool,
    running: bool,
    cancelled: bool,
    #[cfg(all(test, unix, feature = "builtin-exec"))]
    child_pid: Option<u32>,
}

impl std::fmt::Debug for ExecJob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExecJob")
            .field("command", &self.inner.exec.display_command)
            .finish_non_exhaustive()
    }
}

impl ExecJob {
    pub fn timeout(&self) -> std::time::Duration {
        self.inner.timeout
    }

    /// Prevent a queued command from starting, or wait until a running command
    /// has killed/reaped its child and joined both readers. Do not hold backend
    /// queue locks while calling this method.
    pub fn cancel(&self) {
        let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        state.cancelled = true;
        while state.running {
            state = self
                .inner
                .changed
                .wait(state)
                .unwrap_or_else(|e| e.into_inner());
        }
    }

    fn cancelled(&self) -> bool {
        self.inner
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .cancelled
    }

    pub(crate) fn timeout_error(&self) -> String {
        format!(
            "exec: '{}' timed out after {}ms",
            self.inner.exec.display_command,
            self.inner.timeout.as_millis()
        )
    }

    /// Run once on a blocking host/I/O thread. The returned bytes count only
    /// stdout/stderr, matching the synchronous exec accounting.
    pub fn run(&self) -> Result<(crate::sendable::SendableValue, u64), String> {
        let now = std::time::Instant::now();
        let deadline = now
            .checked_add(self.timeout())
            .unwrap_or(now + std::time::Duration::from_secs(100 * 365 * 24 * 3600));
        self.run_until(deadline)
    }

    pub(crate) fn run_until(
        &self,
        deadline: std::time::Instant,
    ) -> Result<(crate::sendable::SendableValue, u64), String> {
        {
            let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.started {
                return Err("exec: job already ran".into());
            }
            state.started = true;
            if state.cancelled {
                return Err("exec: cancelled".into());
            }
            state.running = true;
        }
        struct Running<'a>(&'a ExecJobInner);
        impl Drop for Running<'_> {
            fn drop(&mut self) {
                let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
                state.running = false;
                self.0.changed.notify_all();
            }
        }
        let _running = Running(&self.inner);
        self.execute(deadline)
    }

    fn execute(
        &self,
        deadline: std::time::Instant,
    ) -> Result<(crate::sendable::SendableValue, u64), String> {
        use crate::sendable::SendableValue;
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};
        let exec = &self.inner.exec;
        let allowed = &exec.allowed.id;
        if file_identity(allowed.path.clone())? != *allowed {
            return Err(format!(
                "exec: '{}' changed since it was allowed",
                allowed.path.display()
            ));
        }
        if self.cancelled() {
            return Err("exec: cancelled".into());
        }
        if Instant::now() >= deadline {
            return Err(self.timeout_error());
        }
        let mut command = Command::new(exec_path(&exec.allowed));
        command
            .args(&exec.args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt as _;
            command.arg0(&exec.display_command);
            command.process_group(0);
        }

        // The guard lives inside the scope closure: on error/panic it kills
        // the group before the scope waits for its output reader threads.
        struct ChildGuard {
            child: std::process::Child,
            reaped: bool,
        }
        impl Drop for ChildGuard {
            fn drop(&mut self) {
                if !self.reaped {
                    kill_exec_child(&mut self.child);
                }
            }
        }
        std::thread::scope(|scope| {
            let mut guard = ChildGuard {
                child: command.spawn().map_err(|e| format!("exec: {e}"))?,
                reaped: false,
            };
            #[cfg(all(test, unix, feature = "builtin-exec"))]
            {
                self.inner
                    .state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .child_pid = Some(guard.child.id());
            }
            let (tx, rx) = mpsc::channel();
            for (stream, is_stdout) in [("stdout", true), ("stderr", false)] {
                let tx = tx.clone();
                let pipe: Box<dyn std::io::Read + Send> = if is_stdout {
                    Box::new(guard.child.stdout.take().expect("stdout is piped"))
                } else {
                    Box::new(guard.child.stderr.take().expect("stderr is piped"))
                };
                scope.spawn(move || {
                    let _ = tx.send((
                        is_stdout,
                        read_exec_pipe(pipe, self.inner.output_limit, stream),
                    ));
                });
            }
            drop(tx);
            let mut stdout = None;
            let mut stderr = None;
            while stdout.is_none() || stderr.is_none() {
                if self.cancelled() {
                    return Err("exec: cancelled".into());
                }
                if Instant::now() >= deadline {
                    return Err(self.timeout_error());
                }
                let left = deadline.saturating_duration_since(Instant::now());
                match rx.recv_timeout(left.min(Duration::from_millis(10))) {
                    Ok((is_stdout, Ok(text))) => {
                        if is_stdout {
                            stdout = Some(text);
                        } else {
                            stderr = Some(text);
                        }
                    }
                    Ok((_, Err(err))) => return Err(err),
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        return Err("exec: output reader thread panicked".into());
                    }
                }
            }
            let status = loop {
                if self.cancelled() {
                    return Err("exec: cancelled".into());
                }
                if Instant::now() >= deadline {
                    return Err(self.timeout_error());
                }
                match guard.child.try_wait().map_err(|e| format!("exec: {e}"))? {
                    Some(status) => {
                        guard.reaped = true;
                        break status;
                    }
                    None => std::thread::sleep(Duration::from_millis(10)),
                }
            };
            let (stdout, stderr) = (stdout.unwrap_or_default(), stderr.unwrap_or_default());
            let bytes = (stdout.len() + stderr.len()) as u64;
            Ok((
                SendableValue::Tuple(vec![
                    SendableValue::Int(status.code().unwrap_or(-1) as i64),
                    SendableValue::String(stdout.into()),
                    SendableValue::String(stderr.into()),
                ]),
                bytes,
            ))
        })
    }
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
/// which stays open. Only an execute-only file (no read permission bits at
/// all) is allowed by identity alone, as before #160. Any other open error,
/// including a permission denied by an ACL or a security module, leaves the
/// command out of the policy, with the error recorded.
fn allowed_executable(path: PathBuf) -> Result<AllowedExecutable, String> {
    match std::fs::File::open(&path) {
        Ok(file) => {
            let metadata = file
                .metadata()
                .map_err(|e| format!("cannot stat executable '{}': {e}", path.display()))?;
            let via = exec_via(&file);
            Ok(AllowedExecutable {
                id: identity_from(path, &metadata)?,
                pin: Some(Arc::new(file)),
                via,
            })
        }
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied && is_execute_only(&path) => {
            Ok(AllowedExecutable {
                id: file_identity(path)?,
                pin: None,
                via: ExecVia::Path("it is execute-only, so it is not held open"),
            })
        }
        Err(e) => Err(format!("cannot open executable '{}': {e}", path.display())),
    }
}

/// Whether `path` has no read permission bits at all (mode `--x`).
fn is_execute_only(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        path.metadata()
            .is_ok_and(|metadata| metadata.permissions().mode() & 0o444 == 0)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        false
    }
}

/// How to run the pinned `file`. On Linux an ELF binary runs as
/// `/proc/self/fd/N`, the file the VM holds open. Anything else runs by
/// path: a `#!` script or a binfmt_misc binary is loaded by an interpreter
/// that opens the path itself, after the descriptor is closed on exec.
fn exec_via(file: &std::fs::File) -> ExecVia {
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsRawFd as _;
        use std::os::unix::fs::FileExt as _;
        let mut magic = [0u8; 4];
        if !file
            .read_at(&mut magic, 0)
            .is_ok_and(|n| n == 4 && &magic == b"\x7fELF")
        {
            return ExecVia::Path("it is not an ELF binary (a #! script or a binfmt_misc format)");
        }
        let fd_path = PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd()));
        if !fd_path.exists() {
            return ExecVia::Path("/proc/self/fd is not available");
        }
        ExecVia::Descriptor(fd_path)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = file;
        ExecVia::Path("only Linux can run a program through the descriptor it holds")
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

/// The path `exec` starts an allowed executable from.
fn exec_path(allowed: &AllowedExecutable) -> &Path {
    match &allowed.via {
        ExecVia::Descriptor(fd_path) => fd_path,
        ExecVia::Path(_) => &allowed.id.path,
    }
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
        for (command, executable) in &opened {
            let Ok(executable) = executable else { continue };
            // Not held open, anywhere; or run by path where Linux could
            // have run the descriptor. Other systems always run by path, as
            // documented, so that is not noted per command.
            let ExecVia::Path(reason) = executable.via else {
                continue;
            };
            if executable.pin.is_none() || cfg!(target_os = "linux") {
                policy.notes.push(format!(
                    "exec: {command} ('{}') runs by path, so a file swapped in at that path between the check and the spawn would run: {reason}",
                    executable.id.path.display()
                ));
            }
        }
        policy.notes.sort();
        self.exec_policy = Arc::new(policy);
    }

    /// Allowed executables with weaker protection than this platform gives,
    /// one sentence each. Embedders should show them when a run starts.
    pub fn exec_policy_notes(&self) -> &[String] {
        &self.exec_policy.notes
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
                rule.args_match(&args) && executable.id.path == resolved.path
            })
        {
            Err(RuntimeError {
                message: format!(
                    "exec: '{}' changed or was replaced after the exec policy was applied (it was written, replaced, or its permissions changed); it is refused until the VM is started again",
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

    pub(super) fn prepare_exec_job(&self, arg: Value) -> Result<ExecJob, RuntimeError> {
        Ok(ExecJob {
            inner: Arc::new(ExecJobInner {
                exec: self.prepare_exec(arg)?,
                timeout: self.heap.io_limits().exec_timeout,
                output_limit: self.heap.exec_output_limit(),
                state: Mutex::new(ExecState::default()),
                changed: std::sync::Condvar::new(),
            }),
        })
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
            "import __Builtin.Exec\nval _ = BuiltinExec.exec ({:?}, [])",
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
                .contains("changed or was replaced after the exec policy was applied"),
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
            "import __Builtin.Exec\nval _ = BuiltinExec.exec ({:?}, [])",
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
            err.message.contains("changed or was replaced"),
            "{}",
            err.message
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_runs_the_pinned_binary_not_a_file_swapped_in_at_its_path() {
        let root = temp_dir("exec-descriptor");
        // A copy of the shell, a plain ELF binary: on Ubuntu 26.04 coreutils
        // such as /bin/echo are one uutils multi-call binary.
        let tool = root.join("sh");
        std::fs::copy(std::fs::canonicalize("/bin/sh").unwrap(), &tool).unwrap();
        let allowed = allowed_executable(tool.clone()).expect("pinned");
        assert!(
            matches!(allowed.via, ExecVia::Descriptor(_)),
            "{:?}",
            allowed.via
        );

        std::fs::remove_file(&tool).unwrap();
        write_executable(&tool, "#!/bin/sh\necho swapped\n");
        use std::os::unix::process::CommandExt as _;
        // argv[0] as `run_exec` sets it: the command, not /proc/self/fd/N.
        let out = std::process::Command::new(exec_path(&allowed))
            .arg0(&tool)
            .args(["-c", "echo pinned"])
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            "pinned\n",
            "status {:?}, stderr {:?}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_runs_a_script_by_path_and_says_so() {
        let root = temp_dir("exec-script-note");
        let script = root.join("tool.sh");
        write_executable(&script, "#!/bin/sh\necho hi\n");
        let allowed = allowed_executable(script.clone()).expect("pinned");
        assert!(matches!(allowed.via, ExecVia::Path(_)), "{:?}", allowed.via);

        let program = crate::test_support::compile_source_hermetic("val _ = ()");
        let mut vm = VM::new(program);
        vm.set_exec_allowed(vec![script.to_string_lossy().into_owned()]);
        let notes = vm.exec_policy_notes();
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].contains("runs by path"), "{notes:?}");
        assert!(notes[0].contains("not an ELF binary"), "{notes:?}");

        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn only_an_execute_only_file_is_allowed_without_being_held_open() {
        use std::os::unix::fs::PermissionsExt as _;

        let root = temp_dir("exec-only");
        let script = root.join("tool.sh");
        write_executable(&script, "#!/bin/sh\necho hi\n");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o111)).unwrap();
        let allowed = allowed_executable(script.clone()).expect("allowed");
        if allowed.pin.is_none() {
            // Not root: the file cannot be opened, so it is checked by
            // identity only, and that is reported.
            assert!(
                matches!(allowed.via, ExecVia::Path(reason) if reason.contains("execute-only"))
            );
        }
        assert!(is_execute_only(&script));

        // Readable by someone, so a permission error is not "execute-only".
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o411)).unwrap();
        assert!(!is_execute_only(&script));

        let _ = std::fs::remove_dir_all(root);
    }
    #[cfg(feature = "builtin-exec")]
    mod cancellation {
        use super::*;
        use crate::io_backend::{IoBackend, IoRequest, IoResult, IoToken, ThreadPoolIoBackend};
        use std::time::{Duration, Instant};

        fn job(timeout_ms: u64) -> ExecJob {
            let program = crate::test_support::compile_source_hermetic(
                "import __Builtin.Exec\nval result = BuiltinExec.exec (\"/bin/sleep\", [\"60\"])",
            );
            let mut vm = crate::builder::VMBuilder::new(program)
                .with_all()
                .with_exec(crate::builder::ExecPolicy {
                    allowed: vec!["/bin/sleep".into()],
                    rules: vec![],
                })
                .exec_timeout_ms(timeout_ms)
                .build();
            vm.set_async_io(true);
            match vm.run_slice(10_000) {
                crate::vm::RunResult::Io(IoRequest::Exec(job)) => job,
                result => panic!("expected owned exec request, got {result:?}"),
            }
        }

        fn started(job: &ExecJob) -> rustix::process::Pid {
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                if let Some(pid) = job.inner.state.lock().unwrap().child_pid {
                    return rustix::process::Pid::from_raw(pid as i32).unwrap();
                }
                assert!(Instant::now() < deadline, "exec did not spawn");
                std::thread::sleep(Duration::from_millis(1));
            }
        }

        fn reaped(pid: rustix::process::Pid) {
            assert_eq!(
                rustix::process::test_kill_process(pid),
                Err(rustix::io::Errno::SRCH),
                "exec child still exists"
            );
        }

        #[test]
        fn exec_cancel_before_start_prevents_spawning() {
            let exec = job(5_000);
            exec.cancel();
            assert_eq!(exec.run().unwrap_err(), "exec: cancelled");
            assert!(exec.inner.state.lock().unwrap().child_pid.is_none());
        }

        #[test]
        fn exec_cancel_running_reaps_before_return() {
            let backend = ThreadPoolIoBackend::with_max_threads(1, 1);
            let exec = job(5_000);
            backend
                .register(IoToken(1), IoRequest::Exec(exec.clone()))
                .unwrap();
            let pid = started(&exec);
            let start = Instant::now();
            backend.cancel(IoToken(1));
            assert!(start.elapsed() < Duration::from_secs(1));
            reaped(pid);
            backend.cancel(IoToken(1));
        }

        #[test]
        fn exec_concurrent_cancel_calls_both_wait_for_cleanup() {
            let backend = ThreadPoolIoBackend::with_max_threads(1, 1);
            let exec = job(5_000);
            backend
                .register(IoToken(1), IoRequest::Exec(exec.clone()))
                .unwrap();
            let pid = started(&exec);
            // Hold cleanup at its state lock while both public cancel calls
            // begin. Neither may return while this running child still exists.
            let state = exec.inner.state.lock().unwrap();
            let start = std::sync::Barrier::new(3);
            let (tx, rx) = mpsc::channel();
            let early_return = std::thread::scope(|scope| {
                for _ in 0..2 {
                    let tx = tx.clone();
                    let backend = &backend;
                    let start = &start;
                    scope.spawn(move || {
                        start.wait();
                        backend.cancel(IoToken(1));
                        tx.send(()).unwrap();
                    });
                }
                start.wait();
                let early = rx.recv_timeout(Duration::from_millis(50)).is_ok();
                drop(state);
                early
            });
            assert!(!early_return, "a concurrent cancel returned before cleanup");
            reaped(pid);
        }

        #[test]
        fn exec_cancel_racing_start_is_idempotent() {
            let mut backend = ThreadPoolIoBackend::with_max_threads(1, 1);
            for token in 0..16 {
                let exec = job(5_000);
                backend
                    .register(IoToken(token), IoRequest::Exec(exec.clone()))
                    .unwrap();
                backend.cancel(IoToken(token));
                backend.cancel(IoToken(token));
                let state = exec.inner.state.lock().unwrap();
                assert!(state.cancelled && !state.running);
                if let Some(pid) = state.child_pid {
                    reaped(rustix::process::Pid::from_raw(pid as i32).unwrap());
                }
            }
            backend.shutdown().unwrap();
        }

        #[test]
        fn exec_queued_cancel_and_deadline_never_spawn() {
            let mut backend = ThreadPoolIoBackend::with_max_threads(1, 1);
            let running = job(5_000);
            backend
                .register(IoToken(1), IoRequest::Exec(running.clone()))
                .unwrap();
            let pid = started(&running);
            let cancelled = job(5_000);
            backend
                .register(IoToken(2), IoRequest::Exec(cancelled.clone()))
                .unwrap();
            backend.cancel(IoToken(2));
            let expired = job(30);
            backend
                .register(IoToken(3), IoRequest::Exec(expired.clone()))
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                if let Some((_, result)) = backend
                    .poll()
                    .into_iter()
                    .find(|(token, _)| *token == IoToken(3))
                {
                    assert!(
                        matches!(result, IoResult::Err(ref message) if message.contains("timed out"))
                    );
                    break;
                }
                assert!(Instant::now() < deadline, "queued deadline did not fire");
                backend.wait_for_completions(Duration::from_millis(10));
            }
            backend.cancel(IoToken(1));
            backend.shutdown().unwrap();
            reaped(pid);
            for exec in [cancelled, expired] {
                assert!(exec.inner.state.lock().unwrap().child_pid.is_none());
                assert_eq!(exec.run().unwrap_err(), "exec: cancelled");
            }
        }

        #[test]
        fn exec_running_timeout_reaps_child() {
            let mut backend = ThreadPoolIoBackend::with_max_threads(1, 1);
            let exec = job(300);
            backend
                .register(IoToken(1), IoRequest::Exec(exec.clone()))
                .unwrap();
            let pid = started(&exec);
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                if let Some((_, result)) = backend.poll().into_iter().next() {
                    assert!(
                        matches!(result, IoResult::Err(ref message) if message.contains("timed out"))
                    );
                    break;
                }
                assert!(Instant::now() < deadline);
                backend.wait_for_completions(Duration::from_millis(10));
            }
            backend.shutdown().unwrap();
            reaped(pid);
        }

        #[test]
        fn exec_shutdown_and_drop_reap_running_children() {
            for shutdown in [false, true] {
                let mut backend = ThreadPoolIoBackend::with_max_threads(1, 1);
                let exec = job(5_000);
                backend
                    .register(IoToken(1), IoRequest::Exec(exec.clone()))
                    .unwrap();
                let pid = started(&exec);
                let start = Instant::now();
                if shutdown {
                    backend.shutdown().unwrap();
                }
                drop(backend);
                assert!(start.elapsed() < Duration::from_secs(1));
                reaped(pid);
            }
        }
    }
}
