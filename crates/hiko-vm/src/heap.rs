use crate::value::{GcRef, HeapObject};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
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
    io_bytes_used: u64,
    max_io_bytes: Option<u64>,
    /// Filesystem root for path enforcement (empty = unrestricted).
    pub fs_root: String,
    /// Per-builtin filesystem folder allowlists.
    pub fs_builtin_folders: HashMap<String, Vec<String>>,
    /// Allowed HTTP hosts (empty = unrestricted).
    pub http_allowed_hosts: Vec<String>,
    /// Per-builtin HTTP host allowlists.
    pub http_allowed_hosts_by_builtin: HashMap<String, Vec<String>>,
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
            io_bytes_used: 0,
            max_io_bytes: None,
            fs_root: String::new(),
            fs_builtin_folders: HashMap::new(),
            http_allowed_hosts: Vec::new(),
            http_allowed_hosts_by_builtin: HashMap::new(),
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
        let mut worklist: Vec<GcRef> = Vec::new();
        let mut children: Vec<GcRef> = Vec::new();

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
        heap.fs_builtin_folders.insert(
            "read_file".to_string(),
            vec![allowed.to_string_lossy().to_string()],
        );

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
        heap.http_allowed_hosts_by_builtin
            .insert("http_get".to_string(), vec!["api.example.com".to_string()]);

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
        heap.http_allowed_hosts = vec!["localhost".to_string()];

        let err = heap
            .check_http_host("http://localhost:80@evil.example/path")
            .unwrap_err();
        assert!(err.contains("evil.example"));
    }

    #[test]
    fn test_allowed_fs_folders_for_allows_missing_configured_folder() {
        let base = temp_dir("fs-allowed-missing-folder");
        let missing = base.join("build").join("nested");

        let mut heap = Heap::new();
        heap.fs_builtin_folders.insert(
            "create_dir".to_string(),
            vec![missing.to_string_lossy().to_string()],
        );

        let folders = heap
            .allowed_fs_folders_for("create_dir")
            .expect("missing configured folder should still resolve");
        assert_eq!(folders.len(), 1);
        assert!(folders[0].ends_with("build/nested"));

        let _ = fs::remove_dir_all(base);
    }
}
