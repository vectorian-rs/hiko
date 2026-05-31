use crate::value::{GcRef, HeapObject, HostHandleId, HostHandleKind, HostResource, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
#[cfg(feature = "builtin-filesystem")]
use std::sync::Arc;
#[cfg(feature = "builtin-http")]
use url::Url;

/// Error returned when a heap allocation exceeds the configured object limit.
#[derive(Debug, Clone)]
pub enum HeapLimitExceeded {
    Objects {
        live: usize,
        limit: usize,
    },
    Bytes {
        used_bytes: usize,
        limit_bytes: usize,
        attempted_bytes: usize,
    },
}

#[derive(Debug, Clone)]
pub struct IoLimitExceeded {
    pub used_bytes: u64,
    pub limit_bytes: u64,
    pub attempted_bytes: u64,
}

impl std::fmt::Display for HeapLimitExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Objects { live, limit } => {
                write!(f, "heap limit exceeded: {live} objects (max {limit})")
            }
            Self::Bytes {
                used_bytes,
                limit_bytes,
                attempted_bytes,
            } => write!(
                f,
                "memory limit exceeded: {} bytes used + {} requested (max {})",
                used_bytes, attempted_bytes, limit_bytes
            ),
        }
    }
}

impl std::fmt::Display for IoLimitExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "io limit exceeded: {} bytes used + {} requested (max {})",
            self.used_bytes, self.attempted_bytes, self.limit_bytes
        )
    }
}

#[cfg(feature = "builtin-filesystem")]
#[derive(Clone, Debug)]
pub struct CapAllowedDir {
    pub root: PathBuf,
    pub dir: Arc<cap_std::fs::Dir>,
}

#[cfg(feature = "builtin-filesystem")]
#[derive(Clone, Debug)]
pub struct CapFsCandidate {
    pub root: PathBuf,
    pub dir: Arc<cap_std::fs::Dir>,
    pub relative_path: PathBuf,
}

pub struct Heap {
    objects: Vec<Option<HeapObject>>,
    object_bytes: Vec<usize>,
    marks: Vec<bool>,
    free_list: Vec<u32>,
    alloc_since_gc: usize,
    gc_threshold: usize,
    max_objects: Option<usize>,
    max_bytes: Option<usize>,
    current_bytes: usize,
    peak_bytes: usize,
    host_resources: HashMap<HostHandleId, HostResource>,
    next_host_handle_id: u64,
    io_bytes_used: u64,
    max_io_bytes: Option<u64>,
    host_work_used: u64,
    max_host_work: Option<u64>,
    /// Count of host resources by kind.
    host_resources_by_kind: HashMap<HostHandleKind, usize>,
    /// Maximum total host resources allowed.
    max_host_resources: Option<usize>,
    /// Per-kind host resource limits.
    host_resource_limits: HashMap<String, usize>,
    /// Filesystem root for path enforcement (empty = unrestricted).
    fs_root: String,
    /// Per-builtin filesystem folder allowlists.
    fs_builtin_folders: HashMap<String, Vec<String>>,
    /// Preopened per-builtin filesystem directory capabilities.
    #[cfg(feature = "builtin-filesystem")]
    fs_builtin_dirs: HashMap<String, Vec<CapAllowedDir>>,
    /// Allowed HTTP hosts (empty = unrestricted).
    http_allowed_hosts: Vec<String>,
    /// Per-builtin HTTP host allowlists.
    http_allowed_hosts_by_builtin: HashMap<String, Vec<String>>,
    /// Per-builtin GitHub issue repo allowlists.
    github_issue_allowed_repos: HashMap<String, Vec<String>>,
    /// Allowed AWS SSO profile names for Aws.Config creation.
    #[cfg(feature = "builtin-aws-config")]
    aws_sso_profiles: Vec<String>,
    /// Whether Aws.Config.instance_profile may use the ambient provider chain/IMDS.
    #[cfg(feature = "builtin-aws-config")]
    aws_allow_instance_profile: bool,
    /// Optional injected stdin content for embedded runtimes.
    stdin_override: Option<String>,
    stdin_override_consumed: bool,
}

impl Default for Heap {
    fn default() -> Self {
        Self::new()
    }
}

impl Heap {
    pub fn new() -> Self {
        Heap {
            objects: Vec::new(),
            object_bytes: Vec::new(),
            marks: Vec::new(),
            free_list: Vec::new(),
            alloc_since_gc: 0,
            gc_threshold: 1024,
            max_objects: None,
            max_bytes: None,
            current_bytes: 0,
            peak_bytes: 0,
            host_resources: HashMap::new(),
            next_host_handle_id: 1,
            io_bytes_used: 0,
            max_io_bytes: None,
            host_work_used: 0,
            max_host_work: None,
            host_resources_by_kind: HashMap::new(),
            max_host_resources: None,
            host_resource_limits: HashMap::new(),
            fs_root: String::new(),
            fs_builtin_folders: HashMap::new(),
            #[cfg(feature = "builtin-filesystem")]
            fs_builtin_dirs: HashMap::new(),
            http_allowed_hosts: Vec::new(),
            http_allowed_hosts_by_builtin: HashMap::new(),
            github_issue_allowed_repos: HashMap::new(),
            #[cfg(feature = "builtin-aws-config")]
            aws_sso_profiles: Vec::new(),
            #[cfg(feature = "builtin-aws-config")]
            aws_allow_instance_profile: false,
            stdin_override: None,
            stdin_override_consumed: false,
        }
    }

    pub fn set_stdin_override(&mut self, input: String) {
        self.stdin_override = Some(input);
        self.stdin_override_consumed = false;
    }

    pub fn read_stdin(&mut self) -> Result<String, String> {
        if let Some(input) = self.stdin_override.take() {
            self.stdin_override_consumed = true;
            self.charge_io_bytes(input.len() as u64)
                .map_err(|e| format!("read_stdin: {e}"))?;
            return Ok(input);
        }
        if self.stdin_override_consumed {
            return Ok(String::new());
        }

        use std::io::Read as _;

        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| format!("read_stdin: {e}"))?;
        self.charge_io_bytes(buf.len() as u64)
            .map_err(|e| format!("read_stdin: {e}"))?;
        Ok(buf)
    }

    pub fn set_fs_root(&mut self, root: String) {
        self.fs_root = root;
        #[cfg(feature = "builtin-filesystem")]
        {
            self.rebuild_cap_dirs();
        }
    }

    pub fn fs_root(&self) -> &str {
        &self.fs_root
    }

    pub fn set_fs_builtin_folders(&mut self, folders: HashMap<String, Vec<String>>) {
        self.fs_builtin_folders = folders;
        #[cfg(feature = "builtin-filesystem")]
        {
            self.rebuild_cap_dirs();
        }
    }

    pub fn fs_builtin_folders(&self) -> &HashMap<String, Vec<String>> {
        &self.fs_builtin_folders
    }

    #[cfg(feature = "builtin-filesystem")]
    pub fn has_cap_fs_policy(&self) -> bool {
        !self.fs_root.is_empty() || !self.fs_builtin_folders.is_empty()
    }

    #[cfg(feature = "builtin-filesystem")]
    pub fn cap_candidates_for(
        &self,
        builtin: &str,
        path: &str,
    ) -> Result<Vec<CapFsCandidate>, String> {
        let dirs = self
            .fs_builtin_dirs
            .get(builtin)
            .ok_or_else(|| format!("builtin '{builtin}' has no filesystem permission"))?;
        if dirs.is_empty() {
            return Err(format!("builtin '{builtin}' has no allowed folders"));
        }
        let mut candidates = Vec::new();
        for allowed in dirs {
            if let Some(relative_path) = cap_relative_path_for(&allowed.root, path)? {
                candidates.push(CapFsCandidate {
                    root: allowed.root.clone(),
                    dir: Arc::clone(&allowed.dir),
                    relative_path,
                });
            }
        }
        if candidates.is_empty() {
            Err(format!("path '{path}' is outside allowed root/folders"))
        } else {
            Ok(candidates)
        }
    }

    #[cfg(feature = "builtin-filesystem")]
    fn rebuild_cap_dirs(&mut self) {
        self.fs_builtin_dirs.clear();
        if self.fs_builtin_folders.is_empty() {
            if !self.fs_root.is_empty()
                && let Ok((root, dir)) = open_cap_dir(&self.fs_root)
            {
                let entry = CapAllowedDir {
                    root,
                    dir: Arc::new(dir),
                };
                for builtin in FILESYSTEM_BUILTIN_NAMES {
                    self.fs_builtin_dirs
                        .insert((*builtin).to_string(), vec![entry.clone()]);
                }
            }
            return;
        }

        for (builtin, folders) in &self.fs_builtin_folders {
            let dirs = folders
                .iter()
                .filter_map(|folder| {
                    open_cap_dir(folder).ok().map(|(root, dir)| CapAllowedDir {
                        root,
                        dir: Arc::new(dir),
                    })
                })
                .collect();
            self.fs_builtin_dirs.insert(builtin.clone(), dirs);
        }
    }

    pub fn set_http_allowed_hosts(&mut self, hosts: Vec<String>) {
        self.http_allowed_hosts = hosts;
    }

    pub fn http_allowed_hosts(&self) -> &[String] {
        &self.http_allowed_hosts
    }

    pub fn set_http_allowed_hosts_by_builtin(&mut self, hosts: HashMap<String, Vec<String>>) {
        self.http_allowed_hosts_by_builtin = hosts;
    }

    pub fn http_allowed_hosts_by_builtin(&self) -> &HashMap<String, Vec<String>> {
        &self.http_allowed_hosts_by_builtin
    }

    #[cfg(feature = "builtin-aws-config")]
    pub fn set_aws_sso_profiles(&mut self, profiles: Vec<String>) {
        self.aws_sso_profiles = profiles;
    }

    #[cfg(feature = "builtin-aws-config")]
    pub fn aws_sso_profiles(&self) -> &[String] {
        &self.aws_sso_profiles
    }

    #[cfg(feature = "builtin-aws-config")]
    pub fn set_aws_allow_instance_profile(&mut self, allowed: bool) {
        self.aws_allow_instance_profile = allowed;
    }

    #[cfg(feature = "builtin-aws-config")]
    pub fn aws_allow_instance_profile(&self) -> bool {
        self.aws_allow_instance_profile
    }

    #[cfg(feature = "builtin-aws-config")]
    pub fn check_aws_instance_profile(&self) -> Result<(), String> {
        if self.aws_allow_instance_profile {
            Ok(())
        } else {
            Err("AWS instance profile auth is not allowed".into())
        }
    }

    #[cfg(feature = "builtin-aws-config")]
    pub fn check_aws_sso_profile(&self, profile: &str) -> Result<(), String> {
        if self
            .aws_sso_profiles
            .iter()
            .any(|allowed| allowed == profile)
        {
            Ok(())
        } else {
            Err(format!("AWS SSO profile '{profile}' is not allowed"))
        }
    }

    /// Check if a path is within the allowed filesystem root.
    pub fn check_fs_path(&self, path: &str) -> Result<PathBuf, String> {
        resolve_fs_path(&self.fs_root, path)
    }

    /// Check if a path is allowed for a specific filesystem builtin.
    pub fn check_fs_path_for(&self, builtin: &str, path: &str) -> Result<PathBuf, String> {
        if self.fs_builtin_folders.is_empty() {
            return resolve_fs_path(&self.fs_root, path);
        }

        let folders = self
            .fs_builtin_folders
            .get(builtin)
            .ok_or_else(|| format!("builtin '{builtin}' has no filesystem permission"))?;
        resolve_fs_path_in_folders(folders, path).map_err(|e| format!("{builtin}: {e}"))
    }

    /// Resolve the configured folder roots for a specific filesystem builtin.
    pub fn allowed_fs_folders_for(&self, builtin: &str) -> Result<Vec<PathBuf>, String> {
        if self.fs_builtin_folders.is_empty() {
            if self.fs_root.is_empty() {
                return Ok(Vec::new());
            }
            return Ok(vec![
                canonicalize_with_missing_tail(Path::new(&self.fs_root))
                    .map_err(|e| format!("cannot resolve fs root '{}': {e}", self.fs_root))?,
            ]);
        }

        let folders = self
            .fs_builtin_folders
            .get(builtin)
            .ok_or_else(|| format!("builtin '{builtin}' has no filesystem permission"))?;
        if folders.is_empty() {
            return Err(format!("builtin '{builtin}' has no allowed folders"));
        }

        folders
            .iter()
            .map(|folder| {
                canonicalize_with_missing_tail(Path::new(folder))
                    .map_err(|e| format!("cannot resolve folder '{}': {e}", folder))
            })
            .collect()
    }

    /// Check if a URL's host is allowed.
    pub fn check_http_host(&self, url: &str) -> Result<(), String> {
        if self.http_allowed_hosts.is_empty() {
            return Ok(());
        }
        let host = parse_http_url_host(url)?;
        if self.http_allowed_hosts.iter().any(|h| h == &host) {
            Ok(())
        } else {
            Err(format!(
                "host '{}' not in allowed hosts: {:?}",
                host, self.http_allowed_hosts
            ))
        }
    }

    /// Check if a URL's host is allowed for a specific HTTP builtin.
    pub fn check_http_host_for(&self, builtin: &str, url: &str) -> Result<(), String> {
        if self.http_allowed_hosts_by_builtin.is_empty() {
            return self.check_http_host(url);
        }

        let allowed_hosts = self
            .http_allowed_hosts_by_builtin
            .get(builtin)
            .ok_or_else(|| format!("builtin '{builtin}' has no HTTP permission"))?;

        let host = parse_http_url_host(url)?;
        if allowed_hosts.iter().any(|h| h == &host) {
            Ok(())
        } else {
            Err(format!(
                "host '{}' not in allowed hosts for '{}': {:?}",
                host, builtin, allowed_hosts
            ))
        }
    }

    pub fn set_github_issue_allowed_repos(&mut self, repos: HashMap<String, Vec<String>>) {
        self.github_issue_allowed_repos = repos;
    }

    pub fn github_issue_allowed_repos(&self) -> &HashMap<String, Vec<String>> {
        &self.github_issue_allowed_repos
    }

    pub fn check_github_repo_for(&self, builtin: &str, repo: &str) -> Result<(), String> {
        let allowed_repos = self
            .github_issue_allowed_repos
            .get(builtin)
            .ok_or_else(|| format!("builtin '{builtin}' has no GitHub permission"))?;

        if allowed_repos.iter().any(|r| r == repo) {
            Ok(())
        } else {
            Err(format!(
                "repo '{}' not in allowed repos for '{}': {:?}",
                repo, builtin, allowed_repos
            ))
        }
    }

    pub fn set_max_objects(&mut self, max: usize) {
        self.max_objects = Some(max);
    }

    pub fn max_objects(&self) -> Option<usize> {
        self.max_objects
    }

    pub fn set_max_bytes(&mut self, max: usize) {
        self.max_bytes = Some(max);
    }

    pub fn max_bytes(&self) -> Option<usize> {
        self.max_bytes
    }

    pub fn live_bytes(&self) -> usize {
        self.current_bytes
    }

    pub fn peak_bytes(&self) -> usize {
        self.peak_bytes
    }

    pub fn set_max_io_bytes(&mut self, max: u64) {
        self.max_io_bytes = Some(max);
    }

    pub fn io_bytes_used(&self) -> u64 {
        self.io_bytes_used
    }

    pub fn max_io_bytes(&self) -> Option<u64> {
        self.max_io_bytes
    }

    pub fn remaining_io_bytes(&self) -> Option<u64> {
        self.max_io_bytes
            .map(|max| max.saturating_sub(self.io_bytes_used))
    }

    pub fn charge_io_bytes(&mut self, bytes: u64) -> Result<(), IoLimitExceeded> {
        let next = self.io_bytes_used.saturating_add(bytes);
        if let Some(limit_bytes) = self.max_io_bytes
            && next > limit_bytes
        {
            return Err(IoLimitExceeded {
                used_bytes: self.io_bytes_used,
                limit_bytes,
                attempted_bytes: bytes,
            });
        }
        self.io_bytes_used = next;
        Ok(())
    }

    pub fn set_max_host_work(&mut self, max: u64) {
        self.max_host_work = Some(max);
    }

    pub fn set_max_host_resources(&mut self, max: usize) {
        self.max_host_resources = Some(max);
    }

    pub fn set_host_resource_limits(&mut self, limits: HashMap<String, usize>) {
        self.host_resource_limits = limits;
    }

    pub fn max_host_work(&self) -> Option<u64> {
        self.max_host_work
    }

    pub fn host_work_used(&self) -> u64 {
        self.host_work_used
    }

    pub fn charge_host_work(&mut self, work: u64) -> Result<(), String> {
        let next = self.host_work_used.saturating_add(work);
        if let Some(limit) = self.max_host_work
            && next > limit
        {
            return Err(format!(
                "host work budget exceeded: used {} limit {}",
                self.host_work_used, limit
            ));
        }
        self.host_work_used = next;
        Ok(())
    }

    pub fn ensure_can_allocate_bytes(
        &self,
        attempted_bytes: usize,
    ) -> Result<(), HeapLimitExceeded> {
        if let Some(limit_bytes) = self.max_bytes {
            let next_bytes = self.current_bytes.saturating_add(attempted_bytes);
            if next_bytes > limit_bytes {
                return Err(HeapLimitExceeded::Bytes {
                    used_bytes: self.current_bytes,
                    limit_bytes,
                    attempted_bytes,
                });
            }
        }
        Ok(())
    }

    pub(crate) fn allocation_would_exceed_limits(&self, object_bytes: usize) -> bool {
        if let Some(max) = self.max_objects {
            let live = self.objects.len() - self.free_list.len();
            if live >= max {
                return true;
            }
        }
        if let Some(limit_bytes) = self.max_bytes {
            let next_bytes = self.current_bytes.saturating_add(object_bytes);
            if next_bytes > limit_bytes {
                return true;
            }
        }
        false
    }

    pub fn alloc_host_resource(
        &mut self,
        resource: HostResource,
    ) -> Result<Value, HeapLimitExceeded> {
        let kind = resource.kind();
        
        // Check total host resources limit
        if let Some(max) = self.max_host_resources {
            if self.host_resources.len() >= max {
                return Err(HeapLimitExceeded::Objects {
                    live: self.host_resources.len(),
                    limit: max,
                });
            }
        }
        
        // Check per-kind host resource limits
        let kind_name = self.host_handle_kind_name(kind);
        if let Some(&max) = self.host_resource_limits.get(&kind_name) {
            let current_count = *self.host_resources_by_kind.get(&kind).unwrap_or(&0);
            if current_count >= max {
                return Err(HeapLimitExceeded::Objects {
                    live: current_count,
                    limit: max,
                });
            }
        }
        
        let id = HostHandleId(self.next_host_handle_id);
        self.next_host_handle_id = self.next_host_handle_id.saturating_add(1);
        self.host_resources.insert(id, resource);
        
        // Update per-kind count
        *self.host_resources_by_kind.entry(kind).or_insert(0) += 1;
        
        match self.alloc(HeapObject::HostHandle { kind, id }) {
            Ok(r) => Ok(Value::Heap(r)),
            Err(err) => {
                self.host_resources.remove(&id);
                // Decrement the count since we failed to allocate
                let count = self.host_resources_by_kind.entry(kind).or_insert(0);
                *count = count.saturating_sub(1);
                if *count == 0 {
                    self.host_resources_by_kind.remove(&kind);
                }
                Err(err)
            }
        }
    }

    pub fn host_resource_count(&self) -> usize {
        self.host_resources.len()
    }

    pub fn host_resource_count_by_kind(&self, kind: HostHandleKind) -> usize {
        *self.host_resources_by_kind.get(&kind).unwrap_or(&0)
    }

    fn host_handle_kind_name(&self, kind: HostHandleKind) -> String {
        match kind {
            #[cfg(feature = "builtin-aws-config")]
            HostHandleKind::AwsConfig => "aws_config".to_string(),
            #[cfg(feature = "builtin-aws-s3")]
            HostHandleKind::AwsS3Client => "aws_s3_client".to_string(),
            #[cfg(feature = "builtin-aws-sqs")]
            HostHandleKind::AwsSqsClient => "aws_sqs_client".to_string(),
            HostHandleKind::Unsupported => "unsupported".to_string(),
        }
    }

    pub fn host_handle_from_value(
        &self,
        value: Value,
        expected_kind: HostHandleKind,
        expected_type: &str,
        context: &str,
    ) -> Result<HostHandleId, String> {
        match value {
            Value::Heap(r) => match self.get(r).map_err(|e| format!("{context}: {e}"))? {
                HeapObject::HostHandle { kind, id } if *kind == expected_kind => {
                    self.get_host_resource(*id, expected_kind, expected_type, context)?;
                    Ok(*id)
                }
                HeapObject::HostHandle { kind, .. } => Err(format!(
                    "{context}: expected {expected_type}, got host handle kind {kind:?}"
                )),
                _ => Err(format!("{context}: expected {expected_type}")),
            },
            _ => Err(format!("{context}: expected {expected_type}")),
        }
    }

    pub fn get_host_resource(
        &self,
        id: HostHandleId,
        expected_kind: HostHandleKind,
        expected_type: &str,
        context: &str,
    ) -> Result<&HostResource, String> {
        let resource = self
            .host_resources
            .get(&id)
            .ok_or_else(|| format!("{context}: dangling {expected_type} host handle"))?;
        if resource.kind() == expected_kind {
            Ok(resource)
        } else {
            Err(format!(
                "{context}: expected {expected_type}, host resource table entry is {:?}",
                resource.kind()
            ))
        }
    }

    #[cfg(feature = "builtin-aws-config")]
    pub fn aws_config_handle_from_value(
        &self,
        value: Value,
        context: &str,
    ) -> Result<&crate::value::AwsConfigHandle, String> {
        let id =
            self.host_handle_from_value(value, HostHandleKind::AwsConfig, "aws_config", context)?;
        match self.get_host_resource(id, HostHandleKind::AwsConfig, "aws_config", context)? {
            HostResource::AwsConfig(handle) => Ok(handle),
            #[allow(unreachable_patterns)]
            _ => Err(format!("{context}: expected aws_config")),
        }
    }

    #[cfg(feature = "builtin-aws-s3")]
    pub fn aws_s3_client_handle_from_value(
        &self,
        value: Value,
        context: &str,
    ) -> Result<&crate::value::AwsS3ClientHandle, String> {
        let id = self.host_handle_from_value(
            value,
            HostHandleKind::AwsS3Client,
            "aws_s3_client",
            context,
        )?;
        match self.get_host_resource(id, HostHandleKind::AwsS3Client, "aws_s3_client", context)? {
            HostResource::AwsS3Client(handle) => Ok(handle),
            #[allow(unreachable_patterns)]
            _ => Err(format!("{context}: expected aws_s3_client")),
        }
    }

    #[cfg(feature = "builtin-aws-sqs")]
    pub fn aws_sqs_client_handle_from_value(
        &self,
        value: Value,
        context: &str,
    ) -> Result<&crate::value::AwsSqsClientHandle, String> {
        let id = self.host_handle_from_value(
            value,
            HostHandleKind::AwsSqsClient,
            "aws_sqs_client",
            context,
        )?;
        match self.get_host_resource(id, HostHandleKind::AwsSqsClient, "aws_sqs_client", context)? {
            HostResource::AwsSqsClient(handle) => Ok(handle),
            #[allow(unreachable_patterns)]
            _ => Err(format!("{context}: expected aws_sqs_client")),
        }
    }

    pub fn alloc(&mut self, obj: HeapObject) -> Result<GcRef, HeapLimitExceeded> {
        let object_bytes = obj.estimated_bytes();
        if let Some(max) = self.max_objects {
            let live = self.objects.len() - self.free_list.len();
            if live >= max {
                return Err(HeapLimitExceeded::Objects { live, limit: max });
            }
        }
        if let Some(limit_bytes) = self.max_bytes {
            let next_bytes = self.current_bytes.saturating_add(object_bytes);
            if next_bytes > limit_bytes {
                return Err(HeapLimitExceeded::Bytes {
                    used_bytes: self.current_bytes,
                    limit_bytes,
                    attempted_bytes: object_bytes,
                });
            }
        }
        self.alloc_since_gc += 1;
        let idx = if let Some(idx) = self.free_list.pop() {
            self.objects[idx as usize] = Some(obj);
            self.object_bytes[idx as usize] = object_bytes;
            idx
        } else {
            let idx = self.objects.len() as u32;
            self.objects.push(Some(obj));
            self.object_bytes.push(object_bytes);
            self.marks.push(false);
            idx
        };
        self.current_bytes = self.current_bytes.saturating_add(object_bytes);
        self.peak_bytes = self.peak_bytes.max(self.current_bytes);
        Ok(GcRef(idx))
    }

    pub fn get(&self, r: GcRef) -> Result<&HeapObject, &'static str> {
        self.objects
            .get(r.0 as usize)
            .and_then(|slot| slot.as_ref())
            .ok_or("dangling GcRef")
    }

    pub fn should_collect(&self) -> bool {
        self.alloc_since_gc >= self.gc_threshold
    }

    /// Suspension-boundary collection for long-lived processes.
    ///
    /// This runs earlier than the normal allocation-driven threshold so a
    /// process that allocates request-local garbage and then blocks or yields
    /// can reclaim it before the next allocation burst. Keep the floor high
    /// enough to avoid collecting on every tiny boundary.
    pub fn should_collect_at_boundary(&self) -> bool {
        let boundary_threshold = (self.gc_threshold / 4).max(256);
        self.alloc_since_gc >= boundary_threshold
    }

    /// Mark a single ref. Returns true if it was newly marked.
    fn mark(&mut self, r: GcRef) -> bool {
        let idx = r.0 as usize;
        if self.marks[idx] {
            return false;
        }
        self.marks[idx] = true;
        true
    }

    /// Run mark-and-sweep. `roots` is an iterator of all root GcRefs.
    pub fn collect(&mut self, roots: impl Iterator<Item = GcRef>) {
        for m in self.marks.iter_mut() {
            *m = false;
        }

        // Worklist avoids stack overflow on deep object graphs
        let mut worklist: Vec<GcRef> = Vec::with_capacity(self.objects.len() / 4);
        let mut children: Vec<GcRef> = Vec::with_capacity(8);

        for r in roots {
            if self.mark(r) {
                worklist.push(r);
            }
        }

        while let Some(r) = worklist.pop() {
            children.clear();
            if let Some(obj) = self.objects[r.0 as usize].as_ref() {
                obj.for_each_gc_ref(|c| children.push(c));
            }
            for &child in &children {
                if self.mark(child) {
                    worklist.push(child);
                }
            }
        }

        self.free_list.clear();
        for i in 0..self.objects.len() {
            if self.objects[i].is_some() && !self.marks[i] {
                if let Some(HeapObject::HostHandle { id, kind }) = self.objects[i].as_ref() {
                    self.host_resources.remove(id);
                    // Decrement the per-kind count
                    let count = self.host_resources_by_kind.entry(*kind).or_insert(0);
                    *count = count.saturating_sub(1);
                    if *count == 0 {
                        self.host_resources_by_kind.remove(kind);
                    }
                }
                self.current_bytes = self.current_bytes.saturating_sub(self.object_bytes[i]);
                self.objects[i] = None;
                self.object_bytes[i] = 0;
                self.free_list.push(i as u32);
            }
        }

        self.alloc_since_gc = 0;
        let live_count = self.objects.len() - self.free_list.len();
        self.gc_threshold = (live_count * 2).max(1024);
    }

    pub fn live_count(&self) -> usize {
        self.objects.iter().filter(|o| o.is_some()).count()
    }
}

#[cfg(feature = "builtin-filesystem")]
const FILESYSTEM_BUILTIN_NAMES: &[&str] = &[
    "read_file",
    "read_file_bytes",
    "write_file",
    "file_exists",
    "list_dir",
    "remove_file",
    "create_dir",
    "is_dir",
    "is_file",
    "read_file_tagged",
    "edit_file_tagged",
    "glob",
    "walk_dir",
];

#[cfg(feature = "builtin-filesystem")]
fn open_cap_dir(path: &str) -> std::io::Result<(PathBuf, cap_std::fs::Dir)> {
    let root = std::fs::canonicalize(path)?;
    let dir = cap_std::fs::Dir::open_ambient_dir(&root, cap_std::ambient_authority())?;
    Ok((root, dir))
}

#[cfg(feature = "builtin-filesystem")]
fn cap_relative_path_for(root: &Path, path: &str) -> Result<Option<PathBuf>, String> {
    let path = Path::new(path);
    if path.is_absolute() {
        if let Some(relative) = cap_relative_path_for_absolute_fast(root, path) {
            return Ok(Some(relative));
        }

        let resolved = canonicalize_with_missing_tail(path)
            .map_err(|e| format!("cannot resolve path '{}': {e}", path.display()))?;
        return match resolved.strip_prefix(root) {
            Ok(relative) => {
                validate_cap_relative_path(relative)?;
                Ok(Some(normalize_cap_relative_path(relative)))
            }
            Err(_) => Ok(None),
        };
    }
    validate_cap_relative_path(path)?;
    Ok(Some(normalize_cap_relative_path(path)))
}

#[cfg(feature = "builtin-filesystem")]
fn cap_relative_path_for_absolute_fast(root: &Path, path: &Path) -> Option<PathBuf> {
    let relative = path.strip_prefix(root).ok()?;
    validate_cap_relative_path(relative).ok()?;
    Some(normalize_cap_relative_path(relative))
}

#[cfg(feature = "builtin-filesystem")]
fn normalize_cap_relative_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        if let std::path::Component::Normal(part) = component {
            normalized.push(part);
        }
    }
    if normalized.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        normalized
    }
}

#[cfg(feature = "builtin-filesystem")]
pub(crate) fn validate_cap_relative_path(path: impl AsRef<Path>) -> Result<(), String> {
    let path = path.as_ref();
    if path.as_os_str().is_empty() {
        return Ok(());
    }
    if path.is_absolute() {
        return Err("absolute paths are not allowed for filesystem capabilities".into());
    }
    for component in path.components() {
        match component {
            std::path::Component::Normal(_) | std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                return Err("path contains parent-directory component outside allowed root".into());
            }
            std::path::Component::RootDir | std::path::Component::Prefix(_) => {
                return Err("absolute paths are not allowed for filesystem capabilities".into());
            }
        }
    }
    Ok(())
}

pub(crate) fn resolve_fs_path(fs_root: &str, path: &str) -> Result<PathBuf, String> {
    if fs_root.is_empty() {
        return Ok(PathBuf::from(path));
    }

    let root = std::fs::canonicalize(fs_root)
        .map_err(|e| format!("cannot resolve fs root '{}': {e}", fs_root))?;
    let target = if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        root.join(path)
    };

    let resolved = canonicalize_with_missing_tail(&target)
        .map_err(|e| format!("cannot resolve path '{path}': {e}"))?;

    if !resolved.starts_with(&root) {
        return Err(format!(
            "path '{}' is outside allowed root '{}'",
            resolved.display(),
            root.display()
        ));
    }

    Ok(resolved)
}

pub(crate) fn resolve_fs_path_in_folders(
    folders: &[String],
    path: &str,
) -> Result<PathBuf, String> {
    if folders.is_empty() {
        return Err("no allowed folders configured".into());
    }

    let mut last_err = None;
    for folder in folders {
        match resolve_fs_path(folder, path) {
            Ok(resolved) => return Ok(resolved),
            Err(err) => last_err = Some(err),
        }
    }

    Err(last_err.unwrap_or_else(|| "path is outside allowed folders".into()))
}

pub(crate) fn parse_http_url_host(url: &str) -> Result<String, String> {
    #[cfg(feature = "builtin-http")]
    {
        let parsed = Url::parse(url).map_err(|e| format!("invalid URL '{url}': {e}"))?;
        match parsed.scheme() {
            "http" | "https" => {}
            scheme => {
                return Err(format!(
                    "URL '{}' uses unsupported scheme '{}'; expected http or https",
                    url, scheme
                ));
            }
        }

        parsed
            .host_str()
            .map(ToOwned::to_owned)
            .ok_or_else(|| format!("URL '{url}' has no host"))
    }

    #[cfg(not(feature = "builtin-http"))]
    {
        let _ = url;
        Err("HTTP builtins are not enabled in this build".into())
    }
}

fn canonicalize_with_missing_tail(path: &Path) -> std::io::Result<PathBuf> {
    if path.exists() {
        return std::fs::canonicalize(path);
    }

    let mut tail = Vec::new();
    let mut cursor = path;

    loop {
        if cursor.exists() {
            let mut resolved = std::fs::canonicalize(cursor)?;
            for component in tail.iter().rev() {
                resolved.push(component);
            }
            return Ok(resolved);
        }

        let name = cursor.file_name().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no existing ancestor for path",
            )
        })?;
        tail.push(name.to_os_string());
        cursor = cursor.parent().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no existing ancestor for path",
            )
        })?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        let unique = format!(
            "hiko-{}-{}-{}",
            name,
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let dir = std::env::temp_dir().join(unique);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[cfg(feature = "builtin-filesystem")]
    #[test]
    fn test_cap_relative_path_for_absolute_under_root() {
        let root = temp_dir("cap-fast-root");
        let canonical_root = std::fs::canonicalize(&root).unwrap();
        let path = canonical_root.join("nested").join("file.txt");

        let relative = cap_relative_path_for(&canonical_root, path.to_str().unwrap())
            .unwrap()
            .expect("path under root should match");
        assert_eq!(relative, PathBuf::from("nested").join("file.txt"));

        let _ = fs::remove_dir_all(root);
    }

    #[cfg(feature = "builtin-filesystem")]
    #[test]
    fn test_cap_relative_path_for_absolute_parent_component_falls_back() {
        let root = temp_dir("cap-fast-parent-root");
        let canonical_root = std::fs::canonicalize(&root).unwrap();
        fs::create_dir_all(canonical_root.join("nested")).unwrap();
        fs::write(canonical_root.join("file.txt"), "ok").unwrap();
        let path = canonical_root.join("nested").join("..").join("file.txt");

        let relative = cap_relative_path_for(&canonical_root, path.to_str().unwrap())
            .unwrap()
            .expect("canonical fallback should keep valid path under root");
        assert_eq!(relative, PathBuf::from("file.txt"));

        let _ = fs::remove_dir_all(root);
    }

    #[cfg(feature = "builtin-filesystem")]
    #[test]
    fn test_cap_relative_path_for_absolute_outside_root_returns_none() {
        let base = temp_dir("cap-fast-outside-base");
        let root = base.join("root");
        let outside = base.join("outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let canonical_root = std::fs::canonicalize(&root).unwrap();
        let path = outside.join("file.txt");
        fs::write(&path, "nope").unwrap();

        let relative = cap_relative_path_for(&canonical_root, path.to_str().unwrap()).unwrap();
        assert!(relative.is_none());

        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn test_resolve_fs_path_rejects_parent_traversal() {
        let root = temp_dir("fs-traversal-root");
        let err = resolve_fs_path(root.to_str().unwrap(), "../escape.txt").unwrap_err();
        assert!(err.contains("outside allowed root"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn test_resolve_fs_path_allows_missing_child_within_root() {
        let root = temp_dir("fs-missing-root");
        let resolved = resolve_fs_path(root.to_str().unwrap(), "nested/new.txt").unwrap();
        let canonical_root = std::fs::canonicalize(&root).unwrap();
        assert_eq!(resolved, canonical_root.join("nested").join("new.txt"));
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn test_resolve_fs_path_rejects_symlink_escape() {
        use std::os::unix::fs::symlink;

        let base = temp_dir("fs-symlink-base");
        let root = base.join("root");
        let outside = base.join("outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("secret.txt"), "secret").unwrap();
        symlink(&outside, root.join("link")).unwrap();

        let err = resolve_fs_path(root.to_str().unwrap(), "link/secret.txt").unwrap_err();
        assert!(err.contains("outside allowed root"));

        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn test_check_fs_path_for_uses_builtin_folders() {
        let base = temp_dir("fs-builtin-folders");
        let allowed = base.join("allowed");
        let blocked = base.join("blocked");
        fs::create_dir_all(&allowed).unwrap();
        fs::create_dir_all(&blocked).unwrap();
        fs::write(allowed.join("file.txt"), "ok").unwrap();

        let mut heap = Heap::new();
        let mut folders = HashMap::new();
        folders.insert(
            "read_file".to_string(),
            vec![allowed.to_string_lossy().to_string()],
        );
        heap.set_fs_builtin_folders(folders);

        let resolved = heap
            .check_fs_path_for("read_file", "file.txt")
            .expect("file under allowed folder should resolve");
        assert!(resolved.ends_with("allowed/file.txt"));

        let err = heap
            .check_fs_path_for(
                "read_file",
                blocked.join("file.txt").to_string_lossy().as_ref(),
            )
            .unwrap_err();
        assert!(err.contains("outside allowed root"));

        let _ = fs::remove_dir_all(base);
    }

    #[cfg(feature = "builtin-http")]
    #[test]
    fn test_check_http_host_for_uses_builtin_hosts() {
        let mut heap = Heap::new();
        let mut hosts = HashMap::new();
        hosts.insert("http_get".to_string(), vec!["api.example.com".to_string()]);
        heap.set_http_allowed_hosts_by_builtin(hosts);

        heap.check_http_host_for("http_get", "https://api.example.com/v1/ok")
            .expect("allowed host should pass");

        let err = heap
            .check_http_host_for("http_get", "https://evil.example.com/nope")
            .unwrap_err();
        assert!(err.contains("not in allowed hosts"));
    }

    #[cfg(feature = "builtin-http")]
    #[test]
    fn test_check_http_host_rejects_userinfo_spoofed_url() {
        let mut heap = Heap::new();
        heap.set_http_allowed_hosts(vec!["localhost".to_string()]);

        let err = heap
            .check_http_host("http://localhost:80@evil.example/path")
            .unwrap_err();
        assert!(err.contains("evil.example"));
    }

    #[cfg(feature = "builtin-aws-config")]
    #[test]
    fn aws_sso_profile_policy_allows_only_configured_profiles() {
        let mut heap = Heap::new();
        heap.set_aws_sso_profiles(vec!["dev".to_string()]);

        assert!(heap.check_aws_sso_profile("dev").is_ok());
        let err = heap.check_aws_sso_profile("prod").unwrap_err();
        assert!(
            err.contains("AWS SSO profile 'prod' is not allowed"),
            "{err}"
        );
    }

    #[cfg(feature = "builtin-aws-s3")]
    #[test]
    fn host_resource_table_validates_kind_and_drops_on_gc() {
        use crate::value::{AwsConfigAuthMethod, AwsConfigHandle, HostHandleKind, HostResource};
        use std::sync::Arc;

        let mut heap = Heap::new();
        let config = heap
            .alloc_host_resource(HostResource::AwsConfig(AwsConfigHandle {
                auth: AwsConfigAuthMethod::InstanceProfile,
                sdk_config: Arc::new(aws_config::SdkConfig::builder().build()),
            }))
            .unwrap();
        let id = heap
            .host_handle_from_value(config, HostHandleKind::AwsConfig, "aws_config", "test")
            .unwrap();
        assert_eq!(heap.host_resource_count(), 1);
        assert!(
            heap.get_host_resource(id, HostHandleKind::AwsS3Client, "aws_s3_client", "test")
                .unwrap_err()
                .contains("expected aws_s3_client")
        );

        heap.collect(std::iter::empty());
        assert_eq!(heap.host_resource_count(), 0);
        assert!(
            heap.get_host_resource(id, HostHandleKind::AwsConfig, "aws_config", "test")
                .unwrap_err()
                .contains("dangling")
        );
    }

    #[cfg(feature = "builtin-aws-config")]
    #[test]
    fn host_resource_limits_enforced() {
        use crate::value::{AwsConfigAuthMethod, AwsConfigHandle, HostResource};
        use std::sync::Arc;

        let mut heap = Heap::new();
        
        // Set limit of 1 AWS config
        let mut limits = HashMap::new();
        limits.insert("aws_config".to_string(), 1);
        heap.set_host_resource_limits(limits);
        
        // First allocation should succeed
        let config1 = heap
            .alloc_host_resource(HostResource::AwsConfig(AwsConfigHandle {
                auth: AwsConfigAuthMethod::InstanceProfile,
                sdk_config: Arc::new(aws_config::SdkConfig::builder().build()),
            }));
        assert!(config1.is_ok());
        assert_eq!(heap.host_resource_count_by_kind(HostHandleKind::AwsConfig), 1);
        
        // Second allocation should fail
        let config2 = heap
            .alloc_host_resource(HostResource::AwsConfig(AwsConfigHandle {
                auth: AwsConfigAuthMethod::InstanceProfile,
                sdk_config: Arc::new(aws_config::SdkConfig::builder().build()),
            }));
        assert!(config2.is_err());
        
        // Check that the count is still 1
        assert_eq!(heap.host_resource_count_by_kind(HostHandleKind::AwsConfig), 1);
    }

    #[cfg(feature = "builtin-aws-config")]
    #[test]
    fn total_host_resource_limit_enforced() {
        use crate::value::{AwsConfigAuthMethod, AwsConfigHandle, HostResource};
        use std::sync::Arc;

        let mut heap = Heap::new();
        
        // Set total limit of 1 host resource
        heap.set_max_host_resources(1);
        
        // First allocation should succeed
        let config = heap
            .alloc_host_resource(HostResource::AwsConfig(AwsConfigHandle {
                auth: AwsConfigAuthMethod::InstanceProfile,
                sdk_config: Arc::new(aws_config::SdkConfig::builder().build()),
            }));
        assert!(config.is_ok());
        assert_eq!(heap.host_resource_count(), 1);
        
        // Second allocation should fail
        let config2 = heap
            .alloc_host_resource(HostResource::AwsConfig(AwsConfigHandle {
                auth: AwsConfigAuthMethod::InstanceProfile,
                sdk_config: Arc::new(aws_config::SdkConfig::builder().build()),
            }));
        assert!(config2.is_err());
        
        // Check that the count is still 1
        assert_eq!(heap.host_resource_count(), 1);
    }

    #[test]
    fn test_allowed_fs_folders_for_allows_missing_configured_folder() {
        let base = temp_dir("fs-allowed-missing-folder");
        let missing = base.join("build").join("nested");

        let mut heap = Heap::new();
        let mut folders = HashMap::new();
        folders.insert(
            "create_dir".to_string(),
            vec![missing.to_string_lossy().to_string()],
        );
        heap.set_fs_builtin_folders(folders);

        let folders = heap
            .allowed_fs_folders_for("create_dir")
            .expect("missing configured folder should still resolve");
        assert_eq!(folders.len(), 1);
        assert!(folders[0].ends_with("build/nested"));

        let _ = fs::remove_dir_all(base);
    }
}
