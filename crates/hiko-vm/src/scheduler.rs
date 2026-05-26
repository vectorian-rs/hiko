//! Pluggable scheduler trait and default FIFO implementation.

use crate::process::Pid;
use std::collections::VecDeque;
use std::sync::{Condvar, Mutex};

/// Scheduling decisions are isolated behind this trait.
/// The runtime calls into the scheduler; the scheduler never
/// reaches into runtime internals.
pub trait Scheduler: Send + Sync {
    /// A process became runnable (new, yielded, or unblocked).
    fn enqueue(&self, pid: Pid);

    /// Block until a runnable process is available, then return it.
    /// Returns None on shutdown.
    fn dequeue(&self) -> Option<Pid>;

    /// Remove queued runnable entries for a process that finished, failed, or
    /// became non-runnable through another runtime transition.
    ///
    /// Implementations may still treat the process table as the lifecycle
    /// source of truth, but this method must not promise stronger behavior than
    /// it provides. The default FIFO scheduler performs a best-effort queue
    /// cleanup by removing all currently queued entries for `pid`.
    fn remove(&self, pid: Pid);

    /// Hint: how many reductions to grant this process.
    fn reductions(&self, pid: Pid) -> u64;

    /// Non-blocking dequeue. Returns None if no process is ready.
    fn try_dequeue(&self) -> Option<Pid>;

    /// Signal all waiting workers to shut down.
    fn shutdown(&self);
}

/// Simple FIFO scheduler. Fixed reduction count.
pub struct FifoScheduler {
    state: Mutex<FifoState>,
    notify: Condvar,
    reductions_per_slice: u64,
}

struct FifoState {
    queue: VecDeque<Pid>,
    shutdown: bool,
}

impl FifoScheduler {
    pub fn new(reductions_per_slice: u64) -> Self {
        Self {
            state: Mutex::new(FifoState {
                queue: VecDeque::new(),
                shutdown: false,
            }),
            notify: Condvar::new(),
            reductions_per_slice,
        }
    }
}

impl Scheduler for FifoScheduler {
    fn enqueue(&self, pid: Pid) {
        let mut state = self.state.lock().unwrap();
        state.queue.push_back(pid);
        self.notify.notify_one();
    }

    fn dequeue(&self) -> Option<Pid> {
        let mut state = self.state.lock().unwrap();
        loop {
            if state.shutdown {
                return None;
            }
            if let Some(pid) = state.queue.pop_front() {
                return Some(pid);
            }
            state = self.notify.wait(state).unwrap();
        }
    }

    fn remove(&self, pid: Pid) {
        let mut state = self.state.lock().unwrap();
        state.queue.retain(|queued| *queued != pid);
    }

    fn reductions(&self, _pid: Pid) -> u64 {
        self.reductions_per_slice
    }

    fn try_dequeue(&self) -> Option<Pid> {
        let mut state = self.state.lock().unwrap();
        if state.shutdown {
            return None;
        }
        state.queue.pop_front()
    }

    fn shutdown(&self) {
        let mut state = self.state.lock().unwrap();
        state.shutdown = true;
        self.notify.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fifo_enqueue_dequeue() {
        let sched = FifoScheduler::new(1000);
        sched.enqueue(Pid(1));
        sched.enqueue(Pid(2));
        sched.enqueue(Pid(3));
        assert_eq!(sched.dequeue(), Some(Pid(1)));
        assert_eq!(sched.dequeue(), Some(Pid(2)));
        assert_eq!(sched.dequeue(), Some(Pid(3)));
    }

    #[test]
    fn test_fifo_shutdown() {
        let sched = FifoScheduler::new(1000);
        sched.shutdown();
        assert_eq!(sched.dequeue(), None);
    }

    #[test]
    fn test_fifo_reductions() {
        let sched = FifoScheduler::new(500);
        assert_eq!(sched.reductions(Pid(1)), 500);
    }

    #[test]
    fn test_fifo_remove_drops_queued_entries() {
        let sched = FifoScheduler::new(1000);
        sched.enqueue(Pid(1));
        sched.enqueue(Pid(2));
        sched.enqueue(Pid(1));
        sched.enqueue(Pid(3));

        sched.remove(Pid(1));

        assert_eq!(sched.try_dequeue(), Some(Pid(2)));
        assert_eq!(sched.try_dequeue(), Some(Pid(3)));
        assert_eq!(sched.try_dequeue(), None);
    }
}
