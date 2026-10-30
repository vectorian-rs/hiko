//! Loom models of the threaded runtime's lifecycle races.
//!
//! Each test runs real lifecycle transitions (`handle_await`,
//! `terminate_process`, `handle_wait_any`, `handle_cancel`,
//! `handle_io_completion`) on two or three Loom threads, each owning the
//! process it acts on, as a worker or the I/O monitor would. Loom explores
//! their interleavings; after each one the test checks the table.
//!
//! Run with:
//!
//! ```text
//! RUSTFLAGS="--cfg loom" cargo test -p hiko-vm --release loom
//! ```

use super::*;
use crate::sendable::SendableValue;
use crate::sync::Mutex;
use loom::sync::Arc as LoomArc;
use loom::thread;
use std::sync::OnceLock;

/// Bounding preemptions keeps each model to seconds. Every race below needs
/// at most two preemptions to show up.
fn model(f: impl Fn() + Sync + Send + 'static) {
    let mut builder = loom::model::Builder::new();
    if builder.preemption_bound.is_none() {
        builder.preemption_bound = Some(3);
    }
    let f = std::sync::Arc::new(f);
    builder.check(move || {
        let f = f.clone();
        spawn(move || f()).join().unwrap();
    });
}

/// Loom's default coroutine stack is too small for a VM.
fn spawn<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> thread::JoinHandle<T> {
    thread::Builder::new()
        .stack_size(4 * 1024 * 1024)
        .spawn(f)
        .unwrap()
}

fn program() -> CompiledProgram {
    static PROGRAM: OnceLock<CompiledProgram> = OnceLock::new();
    PROGRAM
        .get_or_init(|| crate::test_support::compile_source_hermetic("val _ = ()"))
        .clone()
}

/// An I/O backend that records cancellations.
#[derive(Default)]
struct RecordingIo {
    cancelled: Mutex<Vec<IoToken>>,
}

impl IoBackend for RecordingIo {
    fn register(
        &self,
        _token: IoToken,
        _request: crate::io_backend::IoRequest,
    ) -> Result<(), String> {
        Ok(())
    }

    fn poll(&self) -> Vec<(IoToken, IoResult)> {
        Vec::new()
    }

    fn cancel(&self, token: IoToken) {
        self.cancelled.lock().unwrap().push(token);
    }
}

struct World {
    table: ProcessTable,
    scheduler: FifoScheduler,
    io: RecordingIo,
}

impl World {
    fn new() -> LoomArc<Self> {
        LoomArc::new(Self {
            table: ProcessTable::new(),
            scheduler: FifoScheduler::new(1000),
            io: RecordingIo::default(),
        })
    }

    fn cx(&self) -> Ctx<'_> {
        Ctx {
            table: &self.table,
            scheduler: &self.scheduler,
            io: &self.io,
        }
    }

    /// A live process owned by the caller (out of the table, as while a
    /// worker runs it), with a placeholder on its stack for a delivery.
    fn owned(&self, parent: Option<Pid>) -> Process {
        let pid = self.table.alloc_pid();
        let mut vm = VM::new(program());
        vm.push_value(Value::Unit);
        let mut process = Process::new(pid, vm, parent);
        if let Some(parent) = parent {
            process.root = parent;
        }
        self.table.live.insert(pid);
        process
    }

    /// Put an owned process in the table, blocked for `reason`.
    fn park_blocked(&self, mut process: Process, reason: BlockReason) {
        process.status = ProcessStatus::Blocked(reason);
        let mut terminations = Terminations::new();
        park(self.cx(), process, &mut terminations);
        assert!(terminations.is_empty());
    }

    fn queued(&self) -> Vec<Pid> {
        std::iter::from_fn(|| self.scheduler.try_dequeue()).collect()
    }

    fn top_of_stack(&self, pid: Pid) -> String {
        let process = self.table.processes.get(&pid).expect("process in table");
        format!("{:?}", process.vm.stack.last())
    }

    fn assert_runnable(&self, pid: Pid) {
        let process = self.table.processes.get(&pid).expect("process in table");
        assert!(
            matches!(process.status, ProcessStatus::Runnable),
            "{pid:?} is {:?}",
            process.status
        );
    }

    fn check(&self) {
        if let Err(message) = self.table.check_invariants() {
            panic!("{message}");
        }
    }
}

fn ok(n: i64) -> ChildOutcome {
    ChildOutcome::Ok(SendableValue::Int(n))
}

/// A parent awaits a child while the child terminates. Whichever order
/// they run in, the parent is woken exactly once with the child's result,
/// and the tombstone is consumed.
#[test]
fn loom_await_vs_child_completion() {
    for kind in [AwaitKind::Raw, AwaitKind::Result] {
        model(move || {
            let world = World::new();
            let mut parent = world.owned(None);
            let child = world.owned(Some(parent.pid));
            let (parent_pid, child_pid) = (parent.pid, child.pid);
            parent.children.insert(child_pid);

            let w = world.clone();
            let child_thread = spawn(move || {
                terminate_process(w.cx(), child, ok(7));
            });
            handle_await(world.cx(), parent, child_pid, kind);
            child_thread.join().unwrap();

            world.assert_runnable(parent_pid);
            assert_eq!(world.queued(), vec![parent_pid]);
            if kind == AwaitKind::Raw {
                assert_eq!(
                    world.top_of_stack(parent_pid),
                    format!("{:?}", Some(Value::Int(7)))
                );
            }
            assert!(world.table.tombstones.is_empty());
            assert!(world.table.waiters.is_empty());
            assert!(!world.table.live.contains(&child_pid));
            world.check();
        });
    }
}

/// A parent waits on two children that terminate at the same time. It is
/// woken exactly once, with one of them, and keeps both tombstones (only
/// `await` consumes a tombstone).
#[test]
fn loom_wait_any_two_children_complete_at_once() {
    model(|| {
        let world = World::new();
        let mut parent = world.owned(None);
        let first = world.owned(Some(parent.pid));
        let second = world.owned(Some(parent.pid));
        let (parent_pid, first_pid, second_pid) = (parent.pid, first.pid, second.pid);
        parent.children.extend([first_pid, second_pid]);

        let w = world.clone();
        let first_thread = spawn(move || terminate_process(w.cx(), first, ok(1)));
        let w = world.clone();
        let second_thread = spawn(move || terminate_process(w.cx(), second, ok(2)));
        handle_wait_any(world.cx(), parent, vec![first_pid, second_pid]);
        first_thread.join().unwrap();
        second_thread.join().unwrap();

        world.assert_runnable(parent_pid);
        assert_eq!(world.queued(), vec![parent_pid]);
        let winner = world.top_of_stack(parent_pid);
        assert!(
            [first_pid, second_pid]
                .iter()
                .any(|pid| winner == format!("{:?}", Some(Value::Pid(pid.0)))),
            "{winner:?}"
        );
        assert!(world.table.any_waiters.is_empty());
        assert_eq!(world.table.tombstones.len(), 2);
        world.check();
    });
}

/// A parent cancels a child that is finishing a slice and terminating on
/// another worker. The child terminates exactly once, no cancellation
/// request is left behind, and the parent continues.
#[test]
fn loom_cancel_vs_completion() {
    model(|| {
        let world = World::new();
        let mut parent = world.owned(None);
        let child = world.owned(Some(parent.pid));
        let (parent_pid, child_pid) = (parent.pid, child.pid);
        parent.children.insert(child_pid);

        let w = world.clone();
        let child_thread = spawn(move || terminate_process(w.cx(), child, ok(7)));
        handle_cancel(world.cx(), parent, child_pid);
        child_thread.join().unwrap();

        world.assert_runnable(parent_pid);
        assert_eq!(world.queued(), vec![parent_pid]);
        assert!(!world.table.live.contains(&child_pid));
        assert!(!world.table.processes.contains_key(&child_pid));
        assert!(world.table.tombstones.contains_key(&child_pid));
        assert!(world.table.pending_cancels.is_empty());
        world.check();
    });
}

/// A child blocked on I/O is cancelled by its parent while the I/O monitor
/// delivers the completion. Either the completion wins (the child is
/// runnable, and the cancel is either marked on its VM or left in
/// `pending_cancels` for the worker that dequeues it) or the cancel wins (the
/// child terminates, its request is cancelled with the backend, and the late
/// completion is dropped). A cancelled child is never resumed.
#[test]
fn loom_cancel_vs_io_completion() {
    model(|| {
        let world = World::new();
        let mut parent = world.owned(None);
        let child = world.owned(Some(parent.pid));
        let (parent_pid, child_pid) = (parent.pid, child.pid);
        parent.children.insert(child_pid);
        let token = IoToken(1);
        world.table.io_waiters.insert(token, child_pid);
        world.park_blocked(child, BlockReason::Io(token));

        let w = world.clone();
        let monitor = spawn(move || {
            let result = IoResult::Ok {
                value: SendableValue::Unit,
                io_bytes: 0,
            };
            handle_io_completion(w.cx(), token, result);
        });
        handle_cancel(world.cx(), parent, child_pid);
        monitor.join().unwrap();

        world.assert_runnable(parent_pid);
        assert!(world.table.io_waiters.is_empty());
        let cancelled_with_backend = world.io.cancelled.lock().unwrap().contains(&token);
        if world.table.live.contains(&child_pid) {
            // The completion won.
            let child = world
                .table
                .processes
                .get(&child_pid)
                .expect("child in table");
            assert!(matches!(child.status, ProcessStatus::Runnable));
            assert!(
                child.vm.cancellation_requested()
                    || world.table.pending_cancels.contains(&child_pid),
                "cancellation lost"
            );
            assert!(!cancelled_with_backend);
            drop(child);
            let mut queued = world.queued();
            queued.sort_by_key(|pid| pid.0);
            assert_eq!(queued, vec![parent_pid, child_pid]);
        } else {
            // The cancel won.
            assert!(!world.table.processes.contains_key(&child_pid));
            let record = world.table.tombstones.get(&child_pid).expect("tombstone");
            assert!(matches!(
                record.outcome,
                ChildOutcome::Err(ProcessFailure::Cancelled)
            ));
            drop(record);
            assert!(cancelled_with_backend);
            assert_eq!(world.queued(), vec![parent_pid]);
            assert!(world.table.pending_cancels.is_empty());
        }
        world.check();
    });
}

/// A parent blocked in `await` is cancelled by the grandparent while the
/// awaited child terminates. Whoever wins, the child's tombstone does not
/// outlive the parent, and no registration is left behind.
#[test]
fn loom_tombstone_removal_vs_late_await() {
    model(|| {
        let world = World::new();
        let mut grandparent = world.owned(None);
        let mut parent = world.owned(Some(grandparent.pid));
        let child = world.owned(Some(parent.pid));
        let (grandparent_pid, parent_pid, child_pid) = (grandparent.pid, parent.pid, child.pid);
        grandparent.children.insert(parent_pid);
        parent.children.insert(child_pid);
        world
            .table
            .waiters
            .entry(child_pid)
            .or_default()
            .push(parent_pid);
        world.park_blocked(
            parent,
            BlockReason::Await {
                child: child_pid,
                kind: AwaitKind::Raw,
            },
        );

        let w = world.clone();
        let child_thread = spawn(move || terminate_process(w.cx(), child, ok(7)));
        handle_cancel(world.cx(), grandparent, parent_pid);
        child_thread.join().unwrap();

        world.assert_runnable(grandparent_pid);
        assert!(!world.table.live.contains(&child_pid));
        assert!(world.table.waiters.is_empty());
        if world.table.live.contains(&parent_pid) {
            // The child's result reached the parent first, consuming the
            // tombstone. The cancel then marked the parent's VM, or, if the
            // parent was out of the table being woken, left a request for
            // the worker that dequeues it.
            let parent = world
                .table
                .processes
                .get(&parent_pid)
                .expect("parent in table");
            assert!(
                parent.vm.cancellation_requested()
                    || world.table.pending_cancels.contains(&parent_pid),
                "cancellation lost"
            );
            drop(parent);
            assert_eq!(
                world.top_of_stack(parent_pid),
                format!("{:?}", Some(Value::Int(7)))
            );
            assert!(!world.table.tombstones.contains_key(&child_pid));
        } else {
            // The parent was cancelled while blocked: its tombstone is kept
            // for the grandparent, the child's was reaped.
            assert!(world.table.tombstones.contains_key(&parent_pid));
            assert!(!world.table.tombstones.contains_key(&child_pid));
        }
        world.check();
    });
}
