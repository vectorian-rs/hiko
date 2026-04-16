use crate::builtins;
use crate::value::BuiltinFn;
use crate::vm::VM;
use hiko_compile::chunk::CompiledProgram;

/// Policy for filesystem access.
pub struct FilesystemPolicy {
    pub root: String,
    pub allow_read: bool,
    pub allow_write: bool,
    pub allow_delete: bool,
}

/// Policy for HTTP access.
pub struct HttpPolicy {
    pub allowed_hosts: Vec<String>,
}

/// Policy for direct command execution.
pub struct ExecPolicy {
    pub allowed: Vec<String>,
    /// Timeout in seconds for each exec call (default 30).
    pub timeout: u64,
}

/// Builder for creating VMs with specific capabilities.
pub struct VMBuilder {
    program: CompiledProgram,
    builtins: Vec<(&'static str, BuiltinFn)>,
    exec_allowed: Vec<String>,
    exec_timeout: u64,
    fs_root: String,
    http_allowed_hosts: Vec<String>,
    max_heap: Option<usize>,
    max_fuel: Option<u64>,
}

fn find_builtin(name: &str) -> Option<BuiltinFn> {
    builtins::builtin_entries()
        .into_iter()
        .find(|(n, _)| *n == name)
        .map(|(_, f)| f)
}

impl VMBuilder {
    pub fn new(program: CompiledProgram) -> Self {
        Self {
            program,
            builtins: Vec::new(),
            exec_allowed: Vec::new(),
            exec_timeout: 30,
            fs_root: String::new(),
            http_allowed_hosts: Vec::new(),
            max_heap: None,
            max_fuel: None,
        }
    }

    /// Include all builtins with no restrictions (current behavior).
    pub fn with_all(mut self) -> Self {
        self.builtins = builtins::builtin_entries();
        self
    }

    /// Include core builtins (I/O, string ops, math, env, time).
    pub fn with_core(mut self) -> Self {
        let core_names = [
            "print",
            "println",
            "read_line",
            "int_to_string",
            "float_to_string",
            "string_to_int",
            "char_to_int",
            "int_to_char",
            "int_to_float",
            "string_length",
            "substring",
            "string_contains",
            "trim",
            "split",
            "string_replace",
            "regex_match",
            "regex_replace",
            "json_parse",
            "json_to_string",
            "json_get",
            "json_keys",
            "json_length",
            "sqrt",
            "abs_int",
            "abs_float",
            "floor",
            "ceil",
            "getenv",
            "starts_with",
            "ends_with",
            "to_upper",
            "to_lower",
            "epoch",
            "epoch_ms",
            "monotonic_ms",
            "bytes_length",
            "bytes_to_string",
            "string_to_bytes",
            "bytes_get",
            "bytes_slice",
            "blake3",
            "random_bytes",
            "rng_seed",
            "rng_bytes",
            "rng_int",
            "sleep",
            "string_join",
            "spawn",
            "await_process",
            // send_message and receive_message removed from user model (v1)
            // structured concurrency via spawn/await only
            "panic",
            "assert",
            "assert_eq",
        ];
        for entry in builtins::builtin_entries() {
            if core_names.contains(&entry.0) {
                self.builtins.push(entry);
            }
        }
        self
    }

    /// Include filesystem builtins filtered by policy.
    pub fn with_filesystem(mut self, policy: FilesystemPolicy) -> Self {
        self.fs_root = policy.root.clone();
        let fs_read = [
            "read_file",
            "file_exists",
            "is_dir",
            "is_file",
            "list_dir",
            "path_join",
            "read_file_tagged",
            "glob",
            "walk_dir",
        ];
        let fs_write = ["write_file", "create_dir", "edit_file_tagged"];
        let fs_delete = ["remove_file"];

        for entry in builtins::builtin_entries() {
            let included = (policy.allow_read && fs_read.contains(&entry.0))
                || (policy.allow_write && fs_write.contains(&entry.0))
                || (policy.allow_delete && fs_delete.contains(&entry.0));
            if included {
                self.builtins.push(entry);
            }
        }
        self
    }

    /// Include HTTP builtins.
    pub fn with_http(mut self, policy: HttpPolicy) -> Self {
        self.http_allowed_hosts = policy.allowed_hosts.clone();
        let http_names = [
            "http_get",
            "http",
            "http_json",
            "http_msgpack",
            "http_bytes",
        ];
        for entry in builtins::builtin_entries() {
            if http_names.contains(&entry.0) {
                self.builtins.push(entry);
            }
        }
        self
    }

    /// Include the exit builtin.
    pub fn with_exit(mut self) -> Self {
        if let Some(f) = find_builtin("exit") {
            self.builtins.push(("exit", f));
        }
        self
    }

    /// Include exec builtin with whitelisted commands and timeout.
    pub fn with_exec(mut self, policy: ExecPolicy) -> Self {
        self.exec_allowed = policy.allowed;
        self.exec_timeout = policy.timeout;
        if let Some(f) = find_builtin("exec") {
            self.builtins.push(("exec", f));
        }
        self
    }

    /// Register a custom host function.
    pub fn register(mut self, name: &'static str, func: BuiltinFn) -> Self {
        self.builtins.push((name, func));
        self
    }

    /// Set maximum heap size (in number of objects).
    ///
    /// Separate fixed runtime guards still apply to the VM value stack and
    /// call-frame stack; see `hiko_vm::DEFAULT_MAX_STACK_SLOTS` and
    /// `hiko_vm::DEFAULT_MAX_CALL_FRAMES`.
    pub fn max_heap(mut self, objects: usize) -> Self {
        self.max_heap = Some(objects);
        self
    }

    /// Set maximum fuel (opcode executions before timeout).
    ///
    /// Separate fixed runtime guards still apply to the VM value stack and
    /// call-frame stack; see `hiko_vm::DEFAULT_MAX_STACK_SLOTS` and
    /// `hiko_vm::DEFAULT_MAX_CALL_FRAMES`.
    pub fn max_fuel(mut self, fuel: u64) -> Self {
        self.max_fuel = Some(fuel);
        self
    }

    /// Build the VM.
    pub fn build(self) -> VM {
        let mut vm = VM::from_program(self.program);

        for (name, func) in &self.builtins {
            vm.register_builtin(name, *func);
        }

        vm.set_exec_allowed(self.exec_allowed);
        vm.set_exec_timeout(self.exec_timeout);
        vm.set_fs_root(self.fs_root);
        vm.set_http_allowed_hosts(self.http_allowed_hosts);

        if let Some(max) = self.max_heap {
            vm.set_max_heap(max);
        }
        if let Some(fuel) = self.max_fuel {
            vm.set_fuel(fuel);
        }

        vm
    }
}
