//! Opt-in, untimed heap estimates. Heap drops only submit in-memory summaries;
//! the CLI explicitly publishes once, after execution owners have been dropped.
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};

const SCHEMA_VERSION: u32 = 1;
type Shared = Arc<Mutex<State>>;
static ACTIVE: Mutex<Option<Weak<Mutex<State>>>> = Mutex::new(None);
static TEMP_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone)]
pub struct ExportConfig {
    pub path: PathBuf,
    pub nonce: String,
    pub profile: String,
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

impl ExportConfig {
    fn validate(&self) -> Result<(), String> {
        if self.path.file_name().is_none() || !identifier(&self.nonce) || !identifier(&self.profile)
        {
            return Err("heap-metrics: require a destination file and 1-128 character ASCII nonce/profile identifiers".into());
        }
        let parent = parent(&self.path);
        if !parent.is_dir() {
            return Err(format!(
                "heap-metrics: output parent is not a directory: {}",
                parent.display()
            ));
        }
        Ok(())
    }
}
fn parent(path: &Path) -> &Path {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeapMetrics {
    pub heap_id: u64,
    pub successful_gc_object_allocations: u64,
    pub estimated_retained_bytes_peak: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    pub schema_version: u32,
    pub nonce: String,
    pub profile: String,
    pub complete: bool,
    pub heaps_created: u64,
    pub heaps_dropped: u64,
    pub heaps_active: u64,
    pub successful_gc_object_allocations: u64,
    pub max_per_heap_estimated_retained_bytes: u64,
    pub heaps: Vec<HeapMetrics>,
}

impl Artifact {
    /// Structural validation is necessary, but callers must ALSO require a
    /// normally successful CLI process and fresh expected run identifiers.
    pub fn validate(&self, nonce: &str, profile: &str) -> Result<(), String> {
        if self.schema_version != SCHEMA_VERSION
            || !self.complete
            || !identifier(nonce)
            || !identifier(profile)
            || self.nonce != nonce
            || self.profile != profile
        {
            return Err("heap-metrics: unsupported, incomplete or stale artifact identity".into());
        }
        if self.heaps_created == 0
            || self.heaps_created != self.heaps_dropped
            || self.heaps_active != 0
            || u64::try_from(self.heaps.len()).ok() != Some(self.heaps_created)
        {
            return Err("heap-metrics: unbalanced heap lifecycle".into());
        }
        let mut ids = HashSet::new();
        let mut total = 0u64;
        let mut peak = 0u64;
        for heap in &self.heaps {
            if heap.heap_id == 0 || heap.heap_id > self.heaps_created || !ids.insert(heap.heap_id) {
                return Err("heap-metrics: duplicate or invalid heap id".into());
            }
            total = total
                .checked_add(heap.successful_gc_object_allocations)
                .ok_or("heap-metrics: allocation total overflow")?;
            peak = peak.max(heap.estimated_retained_bytes_peak);
        }
        if total != self.successful_gc_object_allocations
            || peak != self.max_per_heap_estimated_retained_bytes
        {
            return Err("heap-metrics: inconsistent totals or peak definition".into());
        }
        Ok(())
    }
}

pub fn read_artifact(path: &Path, nonce: &str, profile: &str) -> Result<Artifact, String> {
    let file = fs::File::open(path)
        .map_err(|e| format!("heap-metrics: cannot read {}: {e}", path.display()))?;
    let artifact: Artifact = serde_json::from_reader(file)
        .map_err(|e| format!("heap-metrics: malformed artifact: {e}"))?;
    artifact.validate(nonce, profile)?;
    Ok(artifact)
}

#[derive(Default)]
struct State {
    created: u64,
    dropped: u64,
    heaps: Vec<HeapMetrics>,
    invalid: Option<&'static str>,
    finalized: bool,
}

#[derive(Clone, Default)]
pub(crate) struct Collector {
    state: Shared,
}
impl Collector {
    fn lock(&self) -> MutexGuard<'_, State> {
        match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => {
                let mut state = poisoned.into_inner();
                state.invalid.get_or_insert("collector lock poisoned");
                state
            }
        }
    }
    pub(crate) fn register(&self) -> HeapRegistration {
        let mut state = self.lock();
        if state.finalized {
            state
                .invalid
                .get_or_insert("heap created after finalization");
        }
        let id = match state.created.checked_add(1) {
            Some(id) => {
                state.created = id;
                id
            }
            None => {
                state.invalid.get_or_insert("heap count overflow");
                0
            }
        };
        HeapRegistration {
            collector: self.clone(),
            id,
        }
    }
    pub(crate) fn snapshot(&self, config: &ExportConfig) -> Result<Artifact, String> {
        let mut state = self.lock();
        if state.finalized {
            return Err("heap-metrics: collection already finalized".into());
        }
        state.finalized = true;
        if let Some(reason) = state.invalid {
            return Err(format!("heap-metrics: incomplete collection: {reason}"));
        }
        let active = state
            .created
            .checked_sub(state.dropped)
            .ok_or("heap-metrics: invalid heap counts")?;
        let mut total = 0u64;
        let mut peak = 0u64;
        for heap in &state.heaps {
            total = total
                .checked_add(heap.successful_gc_object_allocations)
                .ok_or("heap-metrics: allocation total overflow")?;
            peak = peak.max(heap.estimated_retained_bytes_peak);
        }
        let mut heaps = std::mem::take(&mut state.heaps);
        heaps.sort_by_key(|h| h.heap_id);
        let artifact = Artifact {
            schema_version: SCHEMA_VERSION,
            nonce: config.nonce.clone(),
            profile: config.profile.clone(),
            complete: true,
            heaps_created: state.created,
            heaps_dropped: state.dropped,
            heaps_active: active,
            successful_gc_object_allocations: total,
            max_per_heap_estimated_retained_bytes: peak,
            heaps,
        };
        artifact.validate(&config.nonce, &config.profile)?;
        Ok(artifact)
    }
}

/// A once-only registration. Forgetting a heap/token leaves an active count;
/// only Heap::drop reports a completed lifetime, never token Drop itself.
pub(crate) struct HeapRegistration {
    collector: Collector,
    id: u64,
}
impl HeapRegistration {
    pub(crate) fn finish(self, allocations: u64, peak: usize, overflow: bool) {
        let mut state = self.collector.lock();
        if overflow {
            state
                .invalid
                .get_or_insert("per-heap allocation count overflow");
        }
        if state.finalized {
            state
                .invalid
                .get_or_insert("heap dropped after finalization");
        }
        let Ok(peak) = u64::try_from(peak) else {
            state.invalid.get_or_insert("peak does not fit schema");
            return;
        };
        match state.dropped.checked_add(1) {
            Some(dropped) => state.dropped = dropped,
            None => {
                state.invalid.get_or_insert("dropped heap count overflow");
                return;
            }
        }
        state.heaps.push(HeapMetrics {
            heap_id: self.id,
            successful_gc_object_allocations: allocations,
            estimated_retained_bytes_peak: peak,
        });
    }
}

pub(crate) fn register_heap() -> Option<HeapRegistration> {
    let (active, poisoned) = match ACTIVE.lock() {
        Ok(active) => (active, false),
        Err(poisoned) => (poisoned.into_inner(), true),
    };
    let collector = Collector {
        state: active.as_ref()?.upgrade()?,
    };
    if poisoned {
        collector
            .lock()
            .invalid
            .get_or_insert("active registry lock poisoned");
    }
    // Register while holding the registry lock, so finalization cannot miss
    // a creation that already observed this collection.
    Some(collector.register())
}

/// One process-wide collection epoch. Start before constructing ANY VM/heap;
/// finish only after all execution owners and workers have been torn down.
/// A dropped collection never exports; failed/aborted runs remain incomplete.
pub struct Collection {
    collector: Collector,
    config: ExportConfig,
}
impl Collection {
    pub fn start(config: ExportConfig) -> Result<Self, String> {
        config.validate()?;
        let mut active = ACTIVE
            .lock()
            .map_err(|_| "heap-metrics: active registry lock poisoned")?;
        if active.as_ref().and_then(Weak::upgrade).is_some() {
            return Err("heap-metrics: another collection is active".into());
        }
        // Remove old evidence BEFORE script execution, including exit(0).
        match fs::symlink_metadata(&config.path) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                fs::remove_file(&config.path)
                    .map_err(|e| format!("heap-metrics: cannot remove stale artifact: {e}"))?;
            }
            Ok(_) => {
                return Err(
                    "heap-metrics: destination must be a regular file, not a symlink/directory"
                        .into(),
                );
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("heap-metrics: cannot inspect destination: {e}")),
        }
        let collector = Collector::default();
        *active = Some(Arc::downgrade(&collector.state));
        Ok(Self { collector, config })
    }

    pub fn from_env() -> Result<Option<Self>, String> {
        let path = std::env::var_os("HIKO_HEAP_METRICS");
        let nonce = std::env::var_os("HIKO_HEAP_METRICS_NONCE");
        let profile = std::env::var_os("HIKO_HEAP_METRICS_PROFILE");
        match (path, nonce, profile) {
            (None, None, None) => Ok(None),
            (Some(path), Some(nonce), Some(profile)) => {
                let nonce = nonce.into_string().map_err(|_| "heap-metrics: nonce must be UTF-8")?;
                let profile = profile.into_string().map_err(|_| "heap-metrics: profile must be UTF-8")?;
                Self::start(ExportConfig { path: path.into(), nonce, profile }).map(Some)
            }
            _ => Err("heap-metrics: require HIKO_HEAP_METRICS, HIKO_HEAP_METRICS_NONCE and HIKO_HEAP_METRICS_PROFILE together".into()),
        }
    }

    fn uninstall(&self) {
        let mut active = ACTIVE.lock().unwrap_or_else(|e| e.into_inner());
        if active
            .as_ref()
            .is_some_and(|weak| weak.ptr_eq(&Arc::downgrade(&self.collector.state)))
        {
            *active = None;
        }
    }

    pub fn finish(self) -> Result<Artifact, String> {
        self.uninstall();
        let artifact = self.collector.snapshot(&self.config)?;
        publish(&self.config.path, &artifact)?;
        Ok(artifact)
    }
}
impl Drop for Collection {
    fn drop(&mut self) {
        self.uninstall();
    }
}

fn publish(path: &Path, artifact: &Artifact) -> Result<(), String> {
    publish_with(path, artifact, |file, bytes| {
        file.write_all(bytes).and_then(|()| file.sync_all())
    })
}

fn publish_with(
    path: &Path,
    artifact: &Artifact,
    write: impl FnOnce(&mut fs::File, &[u8]) -> std::io::Result<()>,
) -> Result<(), String> {
    let id = TEMP_ID
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
        .map_err(|_| "heap-metrics: temporary id overflow")?;
    let temporary = parent(path).join(format!(
        ".hiko-heap-metrics-{}-{id}.tmp",
        std::process::id()
    ));
    let bytes = serde_json::to_vec_pretty(artifact)
        .map_err(|e| format!("heap-metrics: cannot encode artifact: {e}"))?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|e| format!("heap-metrics: cannot create {}: {e}", temporary.display()))?;
    // Cleanup is allowed only after create_new succeeded: a colliding file
    // belongs to someone else and must never be removed on an open error.
    let written = write(&mut file, &bytes).map_err(|e| format!("cannot write artifact: {e}"));
    drop(file);
    let result = written.and_then(|()| {
        fs::rename(&temporary, path).map_err(|e| format!("cannot rename artifact: {e}"))
    });
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.map_err(|e| format!("heap-metrics: export failed: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> ExportConfig {
        ExportConfig {
            path: PathBuf::from("unused.json"),
            nonce: "nonce-232".into(),
            profile: "untimed".into(),
        }
    }
    fn sample() -> Artifact {
        let collector = Collector::default();
        collector.register().finish(3, 20, false);
        collector.register().finish(5, 30, false);
        collector.snapshot(&config()).unwrap()
    }
    #[test]
    fn counts_sum_but_separate_heap_peaks_take_max_not_sum() {
        let artifact = sample();
        assert_eq!(artifact.successful_gc_object_allocations, 8);
        assert_eq!(artifact.max_per_heap_estimated_retained_bytes, 30);
        assert_eq!(artifact.heaps_created, 2);
        assert_eq!(artifact.heaps_dropped, 2);
        assert_eq!(artifact.heaps_active, 0);
    }
    #[test]
    fn active_and_forgotten_heaps_cannot_finalize_or_retry() {
        let collector = Collector::default();
        let active = collector.register();
        assert!(collector.snapshot(&config()).is_err());
        active.finish(0, 0, false);
        assert!(collector.snapshot(&config()).is_err());
        let empty = Collector::default();
        assert!(
            empty.snapshot(&config()).is_err(),
            "cannot invent a zero-heap CLI run"
        );
    }
    #[test]
    fn concurrent_summaries_are_not_lost() {
        let collector = Collector::default();
        let barrier = Arc::new(std::sync::Barrier::new(8));
        let threads: Vec<_> = (0..8)
            .map(|worker| {
                let collector = collector.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let registrations: Vec<_> = (0..100).map(|_| collector.register()).collect();
                    barrier.wait();
                    for heap in registrations {
                        heap.finish(1, worker * 10, false);
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        let artifact = collector.snapshot(&config()).unwrap();
        assert_eq!(artifact.heaps_created, 800);
        assert_eq!(artifact.successful_gc_object_allocations, 800);
        assert_eq!(artifact.max_per_heap_estimated_retained_bytes, 70);
    }
    #[test]
    fn overflow_and_poison_are_incomplete_not_wrapped() {
        let collector = Collector::default();
        collector.register().finish(u64::MAX, 0, false);
        collector.register().finish(1, 0, false);
        assert!(collector.snapshot(&config()).is_err());
        let collector = Collector::default();
        collector.register().finish(u64::MAX, 0, true);
        assert!(collector.snapshot(&config()).is_err());
        let collector = Collector::default();
        collector.lock().created = u64::MAX;
        collector.register().finish(0, 0, false);
        assert!(collector.snapshot(&config()).is_err());
        let collector = Collector::default();
        let poison = collector.clone();
        assert!(
            std::thread::spawn(move || {
                let _lock = poison.state.lock().unwrap();
                panic!("test poison");
            })
            .join()
            .is_err()
        );
        assert!(collector.snapshot(&config()).is_err());
    }
    #[test]
    fn schema_rejects_stale_partial_malformed_and_inconsistent_evidence() {
        let original = sample();
        let mutations: [fn(&mut Artifact); 10] = [
            |a| a.schema_version += 1,
            |a| a.complete = false,
            |a| a.nonce = "stale".into(),
            |a| a.profile = "wrong".into(),
            |a| a.heaps_active = 1,
            |a| a.heaps_created += 1,
            |a| a.heaps_dropped = 0,
            |a| a.successful_gc_object_allocations += 1,
            |a| a.max_per_heap_estimated_retained_bytes += 1,
            |a| a.heaps[1].heap_id = a.heaps[0].heap_id,
        ];
        for mutate in mutations {
            let mut artifact = original.clone();
            mutate(&mut artifact);
            assert!(artifact.validate("nonce-232", "untimed").is_err());
        }
        let mut value = serde_json::to_value(&original).unwrap();
        value.as_object_mut().unwrap().remove("complete");
        assert!(serde_json::from_value::<Artifact>(value).is_err());
        assert!(serde_json::from_str::<Artifact>("{partial").is_err());
        let mut value = serde_json::to_value(original).unwrap();
        value["unknown"] = serde_json::json!(true);
        assert!(serde_json::from_value::<Artifact>(value).is_err());
    }
    #[test]
    fn atomic_publication_and_write_rename_collision_errors() {
        let root = std::env::temp_dir().join(format!("hiko-metrics-io-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("result.json");
        let artifact = sample();
        assert!(read_artifact(&path, "nonce-232", "untimed").is_err());
        fs::write(&path, "{partial").unwrap();
        assert!(read_artifact(&path, "nonce-232", "untimed").is_err());
        fs::remove_file(&path).unwrap();
        publish(&path, &artifact).unwrap();
        assert_eq!(
            read_artifact(&path, "nonce-232", "untimed")
                .unwrap()
                .successful_gc_object_allocations,
            8
        );
        fs::remove_file(&path).unwrap();
        // Partial write injection exercises the actual cleanup/publish path.
        assert!(
            publish_with(&path, &artifact, |file, _| {
                file.write_all(b"partial")?;
                Err(std::io::Error::other("injected write failure"))
            })
            .is_err()
        );
        assert!(!path.exists());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
        fs::create_dir(&path).unwrap();
        assert!(publish(&path, &artifact).unwrap_err().contains("rename"));
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        fs::remove_dir(&path).unwrap();
        assert!(publish(&root.join("missing/out.json"), &artifact).is_err());
        let id = TEMP_ID.load(Ordering::Relaxed);
        let collision = root.join(format!(
            ".hiko-heap-metrics-{}-{id}.tmp",
            std::process::id()
        ));
        fs::write(&collision, "someone else's file").unwrap();
        assert!(publish(&path, &artifact).is_err());
        assert_eq!(
            fs::read_to_string(&collision).unwrap(),
            "someone else's file"
        );
        fs::remove_dir_all(&root).unwrap();
    }
}
