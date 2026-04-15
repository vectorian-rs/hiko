use crate::value::{GcRef, HeapObject};
use std::path::{Path, PathBuf};

pub struct Heap {
    objects: Vec<Option<HeapObject>>,
    marks: Vec<bool>,
    free_list: Vec<u32>,
    alloc_since_gc: usize,
    gc_threshold: usize,
    max_objects: Option<usize>,
    /// Filesystem root for path enforcement (empty = unrestricted).
    pub fs_root: String,
    /// Allowed HTTP hosts (empty = unrestricted).
    pub http_allowed_hosts: Vec<String>,
}

impl Default for Heap {
    fn default() -> Self {
        Self::new()
    }
}

impl Heap {
    pub fn new() -> Self {
        Heap {
            objects: Vec::with_capacity(4096),
            marks: Vec::with_capacity(4096),
            free_list: Vec::new(),
            alloc_since_gc: 0,
            gc_threshold: 1024,
            max_objects: None,
            fs_root: String::new(),
            http_allowed_hosts: Vec::new(),
        }
    }

    /// Check if a path is within the allowed filesystem root.
    pub fn check_fs_path(&self, path: &str) -> Result<PathBuf, String> {
        resolve_fs_path(&self.fs_root, path)
    }

    /// Check if a URL's host is allowed.
    pub fn check_http_host(&self, url: &str) -> Result<(), String> {
        if self.http_allowed_hosts.is_empty() {
            return Ok(());
        }
        let host = url
            .strip_prefix("https://")
            .or_else(|| url.strip_prefix("http://"))
            .and_then(|rest| rest.split('/').next())
            .and_then(|host_port| host_port.split(':').next())
            .unwrap_or("");
        if self.http_allowed_hosts.iter().any(|h| h == host) {
            Ok(())
        } else {
            Err(format!(
                "host '{}' not in allowed hosts: {:?}",
                host, self.http_allowed_hosts
            ))
        }
    }

    pub fn set_max_objects(&mut self, max: usize) {
        self.max_objects = Some(max);
    }

    pub fn alloc(&mut self, obj: HeapObject) -> GcRef {
        if let Some(max) = self.max_objects {
            let live = self.objects.len() - self.free_list.len();
            if live >= max {
                panic!("heap limit exceeded: {live} objects (max {max})");
            }
        }
        self.alloc_since_gc += 1;
        let idx = if let Some(idx) = self.free_list.pop() {
            self.objects[idx as usize] = Some(obj);
            idx
        } else {
            let idx = self.objects.len() as u32;
            self.objects.push(Some(obj));
            self.marks.push(false);
            idx
        };
        GcRef(idx)
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
                self.objects[i] = None;
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
}
