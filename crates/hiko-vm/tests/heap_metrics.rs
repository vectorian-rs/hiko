//! One test owns the process-wide collector; no concurrent test VM can enter it.
#![cfg(all(
    feature = "heap-metrics",
    feature = "builtin-process",
    feature = "builtin-testing"
))]
use hiko_vm::heap_metrics::{Collection, ExportConfig, read_artifact};
use hiko_vm::{runtime::Runtime, threaded::ThreadedRuntime};

mod common;

#[test]
fn single_and_threaded_runtime_lifecycle_summaries_include_every_heap() {
    let root = std::env::temp_dir().join(format!("hiko-metrics-runtime-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let fixtures = [
        (
            "val p = spawn (fn () => (1, 2))\nval _ = await_process p",
            2,
        ),
        (
            "fun go n = if n = 0 then () else let val p = spawn (fn () => (1, 2)) val _ = await_process p in go (n - 1) end\nval _ = go 20",
            21,
        ),
        (
            "fun loop n = loop (n + 1)\nval p = spawn (fn () => loop 0)\nval _ = cancel p",
            2,
        ),
        (
            "fun loop n = loop (n + 1)\nval p = spawn (fn () => loop 0)",
            2,
        ),
        (
            "val p = spawn (fn () => panic \"handled\")\nval _ = await_process_result p",
            2,
        ),
    ];
    for (case, (source, heaps)) in fixtures.iter().enumerate() {
        let program = common::compile_source(source);
        for workers in 0..=2 {
            let path = root.join(format!("{case}-{workers}.json"));
            let collection = Collection::start(ExportConfig {
                path: path.clone(),
                nonce: "runtime-test".into(),
                profile: "untimed".into(),
            })
            .unwrap();
            if workers == 0 {
                let mut runtime = Runtime::new();
                runtime.spawn_root(program.clone());
                runtime.run_to_completion().unwrap();
                drop(runtime);
            } else {
                let runtime = ThreadedRuntime::new(workers);
                let pid = runtime.spawn_root(program.clone());
                runtime.run_to_completion().unwrap();
                assert!(runtime.failure(pid).is_none());
                drop(runtime);
            }
            let artifact = collection.finish().unwrap();
            assert_eq!(
                artifact.heaps_created, *heaps,
                "workers={workers}, {source}"
            );
            assert_eq!(artifact.heaps_dropped, *heaps);
            assert_eq!(artifact.heaps_active, 0);
            assert_eq!(
                read_artifact(&path, "runtime-test", "untimed")
                    .unwrap()
                    .heaps_created,
                *heaps
            );
        }
    }
    // A public finalization with a still-owned root rejects the entire run,
    // emits nothing, and cannot be rescued by dropping that root afterward.
    let path = root.join("unfinished.json");
    let collection = Collection::start(ExportConfig {
        path: path.clone(),
        nonce: "unfinished".into(),
        profile: "untimed".into(),
    })
    .unwrap();
    let heap = hiko_vm::heap::Heap::new();
    assert!(
        Collection::start(ExportConfig {
            path: root.join("other.json"),
            nonce: "other".into(),
            profile: "untimed".into()
        })
        .is_err()
    );
    assert!(collection.finish().is_err());
    assert!(!path.exists());
    drop(heap);
    assert!(!path.exists());
    std::fs::remove_dir_all(root).unwrap();
}
