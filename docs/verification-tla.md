
# TLA+ Verification

## Two specs, two levels of abstraction

### `ProcessLifecycle.tla` — intended semantics

Models **what should happen**: FIFO scheduler, spawn/await with single-consumption
result delivery, I/O blocking/completion,
failure propagation, and deadlock detection. No worker threads — the scheduler
picks from the front of the queue. Smaller state space, stronger invariants.

### `ThreadedSchedulerImpl.tla` — implementation structure

Models **how the Rust code works**: explicit worker set, worker-held running
processes, stale queue entries, `io_waiters` as flat records, monitor-driven
deadlock shutdown. Closer to the data structures in `threaded.rs` (`DashMap`
for processes, waiters, io_waiters).

The split is useful because:
- If `ProcessLifecycle` has a bug → intended semantics are wrong
- If `ThreadedSchedulerImpl` has a bug that `ProcessLifecycle` does not → implementation diverges from intent
- If both pass → evidence that the implementation faithfully realizes the design

---

## Findings

### Finding 1: Fail/Complete must atomically wake waiters

**Invariant violated:** `WaitersConsistent`

**Trace:**
```
State 0: root (pid=1) Runnable
State 1: root spawns child (pid=2)
State 2: root awaits child → Blocked(Await, 2), waiters[2] = {1}
State 3: child fails → Failed
State 4: DetectDeadlock on root → root Failed
         BUT waiters[2] still has {1}, process 1 is Failed not Blocked
```

**Root cause:** `Fail` and waiter wakeup were modeled as separate non-deterministic
steps. `DetectDeadlock` could fire between them.

**Spec before:**
```tla
Fail(pid) ==
    /\ procs' = [procs EXCEPT ![pid].status = Failed]
    /\ UNCHANGED <<waiters>>   \* waiters NOT cleared

WakeFailed(child) ==           \* separate action, races with DetectDeadlock
    /\ procs[child].status = Failed
    /\ waiters[child] # {}
    /\ ...
```

**Spec after:**
```tla
Fail(pid) ==
    /\ LET wake_set == waiters[pid] IN
        /\ procs' = [p \in DOMAIN procs |->
            IF p = pid THEN [procs[p] EXCEPT !.status = Failed]
            ELSE IF p \in wake_set
            THEN [procs[p] EXCEPT !.status = Failed,
                                   !.block_reason = BkNone,
                                   !.block_target = 0]
            ELSE procs[p]]
        /\ waiters' = [waiters EXCEPT ![pid] = {}]   \* cleared atomically
\* WakeFailed removed from Next
```

**Implementation:** Already correct — `wake_waiters()` is called synchronously
in the worker loop after setting `Failed`.

```rust
// threaded.rs worker_loop
RunResult::Failed(msg) => {
    process.status = ProcessStatus::Failed(msg);
    table.return_process(process);
    scheduler.remove(pid);
    wake_waiters(table, scheduler, pid);  // atomic
}
```

**Status:** Fixed in spec. Implementation was already correct.

---

### Finding 2: DetectDeadlock did not clean up waiters

**Invariant violated:** `WaitersConsistent`

**Trace:**
```
State 0: root (pid=1) Runnable
State 1: root spawns child (pid=2)
State 2: root awaits child → Blocked(Await, 2), waiters[2] = {1}
State 3: child does ReceiveBlock → Blocked(Receive), runqueue empty
State 4: DetectDeadlock on root → root Failed
         BUT waiters[2] still has {1}, child still Blocked
```

**Root cause:** Both the spec AND the code had this bug. The deadlock detector
marked processes Failed but did not clean up `waiters` or cascade failure.
The code then called `shutdown()` and `break` — no "next iteration" to clean up.

**Code before:**
```rust
if self.table.has_permanently_blocked() {
    for mut entry in self.table.processes.iter_mut() {
        if is_permanently_stuck(&entry) {
            entry.status = ProcessStatus::Failed("deadlock".into());
        }
    }
    // BUG: no waiter cleanup, no cascade
    self.scheduler.shutdown();
    break;
}
```

**Code after:**
```rust
if self.table.has_permanently_blocked() {
    let stuck: Vec<Pid> = /* collect stuck pids */;
    for &pid in &stuck {
        // 1. Mark Failed
        entry.status = ProcessStatus::Failed("deadlock: ...".into());
        // 2. Clear waiters and cascade
        if let Some((_, waiter_pids)) = self.table.waiters.remove(&pid) {
            for waiter_pid in waiter_pids {
                waiter.status = ProcessStatus::Failed("deadlock: child deadlocked".into());
            }
        }
    }
    self.scheduler.shutdown();
    break;
}
```

**Status:** Fixed in both spec and code.

**Test:** `test_deadlock_cleanup_clears_waiters_and_fails_processes` in `threaded.rs`

---

## Safety invariants

| Invariant | What it prevents | Spec |
|---|---|---|
| `TypeOK` | Corrupt state fields | Both |
| `ResultDeliveredToParentOnly` | Result delivered to wrong process | Lifecycle |
| `ResultDeliveredAtMostOnce` | Double-await delivers stale value | Lifecycle |
| `RunnableInQueue` | Runnable process missing from scheduler | Both |
| `BlockedNotInQueue` | Blocked/Done/Failed in runqueue | Both |
| `BlockedIsConsistent` | Block reason set without Blocked status | Both |
| `OnlyParentAwaits` | Awaiting someone else's child | Lifecycle |
| `NoCircularAwait` | A↔B deadlock via mutual await | Lifecycle |
| `IoBlockedHasPending` | I/O-blocked with no pending entry | Both |
| `NoOrphanedIo` | Pending I/O with no blocked process | Both |
| `WaitersConsistent` | Stale waiter entries | Both |
| `BlockedAwaitListed` | Blocked-await parent not in waiters map | Lifecycle |
| `AtMostOneWaiterPerChild` | Multiple parents await same child | Lifecycle |
| `NoSilentSerializationFailure` | Non-sendable result sneaks through as Done | Lifecycle |

Implementation-only invariants (`ThreadedSchedulerImpl`):

| Invariant | What it prevents |
|---|---|
| `RunningHeldByExactlyOneWorker` | Two workers run same process |
| `OnlyHeldProcessesAreRunning` | Worker holds non-Running process |
| `BlockedNeverHeld` | Worker holds Blocked/Done/Failed process |
| `QueueKnownPids` | Queue contains non-existent pid |
| `ShutdownImpliesNoRunnableOrRunning` | Processes still schedulable after shutdown |

---

## Liveness (bounded)

Checked via `--check-liveness` with `ProcessLifecycleLive.cfg`:

| Property | Meaning |
|---|---|
| `ParentEventuallyWoken` | Blocked-await parent eventually unblocks when child Done/Failed |
| `IoEventuallyCompletes` | Every pending I/O entry eventually resolves |

Uses weak fairness (`WF_vars`) on `Complete`, `DetectDeadlock`, `IoComplete`.

Also checked for `ThreadedSchedulerImpl` via `ThreadedSchedulerImplLive.cfg`:

| Property | Meaning |
|---|---|
| `ImplParentEventuallyWoken` | Blocked-await parent eventually unblocks at worker level |
| `ImplIoEventuallyCompletes` | Every I/O waiter entry eventually resolves at monitor level |

---

## Verification results

```
ProcessLifecycle.tla      safety   MaxProcesses=2 MaxSteps=8   PASSED (1533 states)
ProcessLifecycle.tla      liveness MaxProcesses=2 MaxSteps=8   PASSED (0 non-trivial SCCs)
ThreadedSchedulerImpl.tla safety   MaxProcesses=4 MaxSteps=10  PASSED (2502 states)
ThreadedSchedulerImpl.tla liveness MaxProcesses=2 MaxSteps=8   PASSED (0 non-trivial SCCs)
```

---

## What this does NOT prove

1. **Bounded, not unbounded.** We check small process counts. A proof for arbitrary N
   requires inductive invariants or k-induction with Z3.
2. **Cancellation/scopes.** Not modeled.
3. **Memory model.** Specs model logical interleaving, not `Ordering::Relaxed` semantics.
   See [issue #25](https://github.com/vectorian-rs/hiko/issues/25) for Loom-based testing.

---

## How to run

```sh
# Safety — lifecycle spec
tla specs/tla/ProcessLifecycle.tla \
  --config specs/tla/ProcessLifecycle.cfg \
  -c MaxProcesses=2 -c MaxSteps=8 -c MaxMessages=1 -c MaxIoOps=1 \
  --allow-deadlock

# Liveness — lifecycle spec
tla specs/tla/ProcessLifecycle.tla \
  --config specs/tla/ProcessLifecycleLive.cfg \
  -c MaxProcesses=2 -c MaxSteps=8 -c MaxMessages=1 -c MaxIoOps=1 \
  --allow-deadlock --check-liveness

# Safety — implementation spec
tla specs/tla/ThreadedSchedulerImpl.tla \
  --config specs/tla/ThreadedSchedulerImpl.cfg \
  --allow-deadlock

# Larger bounds
tla specs/tla/ThreadedSchedulerImpl.tla \
  --config specs/tla/ThreadedSchedulerImpl.cfg \
  -c MaxProcesses=4 -c MaxSteps=10 -c MaxIoOps=2 \
  --allow-deadlock
```

---

## Next steps

1. **Refinement check** — prove every `ThreadedSchedulerImpl` behavior is a valid
   `ProcessLifecycle` behavior. Highest value addition.
2. **Increase bounds** — push to MaxProcesses=4+ with symmetry reduction.
3. **Model send-to-blocked-receiver race** — add "stale wake" action where target
   status changes between return_process and get_mut.
4. **Loom tests** — test Rust atomic orderings. See [issue #25](https://github.com/vectorian-rs/hiko/issues/25).
