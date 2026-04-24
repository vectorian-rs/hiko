use crate::builtins;
use crate::value::BuiltinFn;
use crate::vm::VM;
use hiko_builtin_meta::core_builtin_names;
use hiko_compile::chunk::CompiledProgram;
use std::collections::HashMap;

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
    fs_builtin_folders: HashMap<String, Vec<String>>,
    http_allowed_hosts: Vec<String>,
    http_allowed_hosts_by_builtin: HashMap<String, Vec<String>>,
    max_heap: Option<usize>,
    max_memory_bytes: Option<usize>,
    max_io_bytes: Option<u64>,
    max_work: Option<u64>,
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
            fs_builtin_folders: HashMap::new(),
            http_allowed_hosts: Vec::new(),
            http_allowed_hosts_by_builtin: HashMap::new(),
            max_heap: None,
            max_memory_bytes: None,
            max_io_bytes: None,
            max_work: None,
        }
    }

    fn has_builtin(&self, name: &str) -> bool {
        self.builtins.iter().any(|(existing, _)| *existing == name)
    }

    /// Register a builtin by its public name.
    pub fn register_builtin_name(mut self, name: &'static str) -> Self {
        if !self.has_builtin(name)
            && let Some(func) = find_builtin(name)
        {
            self.builtins.push((name, func));
        }
        self
    }

    /// Register a filesystem builtin with a per-builtin folder allowlist.
    pub fn allow_filesystem_builtin(mut self, name: &'static str, folders: Vec<String>) -> Self {
        if !self.has_builtin(name)
            && let Some(func) = find_builtin(name)
        {
            self.builtins.push((name, func));
        }
        self.fs_builtin_folders.insert(name.to_string(), folders);
        self
    }

    /// Register an HTTP builtin with a per-builtin host allowlist.
    pub fn allow_http_builtin(mut self, name: &'static str, allowed_hosts: Vec<String>) -> Self {
        if !self.has_builtin(name)
            && let Some(func) = find_builtin(name)
        {
            self.builtins.push((name, func));
        }
        self.http_allowed_hosts_by_builtin
            .insert(name.to_string(), allowed_hosts);
        self
    }

    /// Include all builtins with no restrictions (current behavior).
    pub fn with_all(mut self) -> Self {
        self.builtins = builtins::builtin_entries();
        self
    }

    /// Include core builtins (I/O, string ops, math, env, time).
    pub fn with_core(mut self) -> Self {
        for name in core_builtin_names() {
            self = self.register_builtin_name(name);
        }
        self
    }

    /// Include filesystem builtins filtered by policy.
    pub fn with_filesystem(mut self, policy: FilesystemPolicy) -> Self {
        self.fs_root = policy.root.clone();
        let fs_read = [
            "read_file",
            "read_file_bytes",
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

        let folders = vec![policy.root.clone()];
        for name in fs_read {
            if policy.allow_read {
                self = self.allow_filesystem_builtin(name, folders.clone());
            }
        }
        for name in fs_write {
            if policy.allow_write {
                self = self.allow_filesystem_builtin(name, folders.clone());
            }
        }
        for name in fs_delete {
            if policy.allow_delete {
                self = self.allow_filesystem_builtin(name, folders.clone());
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
        for name in http_names {
            self = self.allow_http_builtin(name, policy.allowed_hosts.clone());
        }
        self
    }

    /// Include the exit builtin.
    pub fn with_exit(self) -> Self {
        self.register_builtin_name("exit")
    }

    /// Include exec builtin with whitelisted commands and timeout.
    pub fn with_exec(mut self, policy: ExecPolicy) -> Self {
        self.exec_allowed = policy.allowed;
        self.exec_timeout = policy.timeout;
        self.register_builtin_name("exec")
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

    /// Set maximum work, currently measured in opcode executions.
    ///
    /// Separate fixed runtime guards still apply to the VM value stack and
    /// call-frame stack; see `hiko_vm::DEFAULT_MAX_STACK_SLOTS` and
    /// `hiko_vm::DEFAULT_MAX_CALL_FRAMES`.
    pub fn max_work(mut self, work: u64) -> Self {
        self.max_work = Some(work);
        self
    }

    /// Set maximum fuel, currently measured in opcode executions.
    ///
    /// This remains as a compatibility alias for `max_work`.
    pub fn max_fuel(self, fuel: u64) -> Self {
        self.max_work(fuel)
    }

    /// Set maximum tracked heap memory in bytes.
    pub fn max_memory_bytes(mut self, bytes: usize) -> Self {
        self.max_memory_bytes = Some(bytes);
        self
    }

    /// Set maximum cumulative I/O bytes charged to the VM.
    pub fn max_io_bytes(mut self, bytes: u64) -> Self {
        self.max_io_bytes = Some(bytes);
        self
    }

    /// Build the VM.
    pub fn build(self) -> VM {
        let mut vm = VM::from_program(self.program);

        for (name, func) in &self.builtins {
            vm.register_builtin(*name, *func);
        }

        vm.set_exec_allowed(self.exec_allowed);
        vm.set_exec_timeout(self.exec_timeout);
        vm.set_fs_root(self.fs_root);
        vm.set_fs_builtin_folders(self.fs_builtin_folders);
        vm.set_http_allowed_hosts(self.http_allowed_hosts);
        vm.set_http_allowed_hosts_by_builtin(self.http_allowed_hosts_by_builtin);

        if let Some(max) = self.max_heap {
            vm.set_max_heap(max);
        }
        if let Some(max_memory_bytes) = self.max_memory_bytes {
            vm.set_max_memory_bytes(max_memory_bytes);
        }
        if let Some(max_io_bytes) = self.max_io_bytes {
            vm.set_max_io_bytes(max_io_bytes);
        }
        if let Some(work) = self.max_work {
            vm.set_max_work(work);
        }

        vm
    }
}
