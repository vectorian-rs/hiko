---- MODULE ThreadedSchedulerImpl ----
\* # Documentation
\*
\* ## Why this spec exists
\*
\* ProcessLifecycle.tla defines the user-visible process semantics. The threaded
\* runtime has an additional implementation problem: multiple workers take
\* processes out of the table, execute them, publish terminal state, wake blocked
\* parents, and race with the monitor thread that completes I/O. This spec models
\* those implementation-level ownership and wakeup rules so we can check that the
\* threaded data structures can preserve the semantic contract.
\*
\* ## What we model
\*
\* This model focuses on the structure of crates/hiko-vm/src/threaded.rs:
\*
\*   - explicit worker ownership of running processes
\*   - scheduler queue entries, including stale entries
\*   - process take/return ownership via Running vs table-resident states
\*   - await waiters and wait_any waiters as runtime data structures
\*   - deterministic wait_any delivery from the original ordered PID list
\*   - I/O waiter registration and monitor-driven completion/failure
\*   - deadlock shutdown when no worker, queue, or I/O can make progress
\*
\* ## What we intentionally abstract
\*
\* The model abstracts away bytecode execution, VM stacks, heap objects, actual
\* DashMap locking, and concrete OS threads. A worker transition represents one
\* runtime-visible VM slice result such as yield, spawn, await, wait_any, I/O, or
\* terminal completion. This keeps the state space small enough to model-check
\* while preserving the race patterns that matter for runtime safety.
\*
\* ## Why this is a good idea
\*
\* The hardest bugs in the threaded runtime are not type errors; they are lost
\* wakeups, double deliveries, stale waiter registrations, and cancellation/I/O
\* races. These are exactly the cases where tests can be flaky or miss rare
\* interleavings. A lower-level TLA+ model complements Rust tests by exploring
\* interleavings systematically and by documenting which implementation
\* invariants must stay true as the runtime evolves.

EXTENDS Naturals, Sequences, FiniteSets, TLC

CONSTANTS
    MaxProcesses,
    MaxSteps,
    MaxIoOps,
    Workers

VARIABLES
    procs,
    queue,
    workers,
    waiters,
    any_waiters,
    tombstones,
    pending_cancels,
    io_waiters,
    next_pid,
    next_token,
    shutdown,
    step

vars == <<procs, queue, workers, waiters, any_waiters, tombstones, pending_cancels, io_waiters, next_pid, next_token, shutdown, step>>

Runnable       == "runnable"
Running        == "running"
BlockedAwait   == "blocked_await"
BlockedIo      == "blocked_io"
BlockedAny     == "blocked_any"
Done           == "done"
Failed         == "failed"

TombNone     == "tomb_none"
TombReady    == "tomb_ready"
TombConsumed == "tomb_consumed"

EmptyProc(parent) ==
    [status |-> Runnable,
     parent |-> parent,
     target |-> 0,
     targets |-> <<>>,
     delivered_pid |-> 0]

Init ==
    /\ procs = 1 :> EmptyProc(0)
    /\ queue = <<1>>
    /\ workers = [w \in Workers |-> 0]
    /\ waiters = 1 :> {}
    /\ any_waiters = 1 :> {}
    /\ tombstones = 1 :> TombNone
    /\ pending_cancels = {}
    /\ io_waiters = {}
    /\ next_pid = 2
    /\ next_token = 1
    /\ shutdown = FALSE
    /\ step = 0

\* Helpers
AtHead(pid) == queue # <<>> /\ Head(queue) = pid
Idle(w) == workers[w] = 0
StepOK == step < MaxSteps
KnownPid(pid) == pid \in DOMAIN procs

SeqSet(seq) == {seq[i] : i \in 1..Len(seq)}

WaitAnyInputs == UNION {[1..n -> 1..MaxProcesses] : n \in 1..MaxProcesses}

TerminalStatus(status) == status \in {Done, Failed}

IsTerminalAfter(done_pid, pid) == pid = done_pid \/ TerminalStatus(procs[pid].status)

LeftmostTerminalAfter(done_pid, children) ==
    CHOOSE child \in SeqSet(children) :
        /\ IsTerminalAfter(done_pid, child)
        /\ \E i \in 1..Len(children) :
            /\ children[i] = child
            /\ \A j \in 1..(i - 1) : ~IsTerminalAfter(done_pid, children[j])

AnyWaiter(child) ==
    IF any_waiters[child] = {} THEN 0 ELSE CHOOSE parent \in any_waiters[child] : TRUE

ClearAnyWaiterRegistrations(parent, targets) ==
    [child \in DOMAIN any_waiters |->
        IF child \in SeqSet(targets)
        THEN any_waiters[child] \ {parent}
        ELSE any_waiters[child]]

HeldBy(pid) == {w \in Workers : workers[w] = pid}
IsHeld(pid) == HeldBy(pid) # {}

StuckPids ==
    {p \in DOMAIN procs :
        /\ procs[p].status = BlockedAwait
        /\ procs[p].target \in DOMAIN procs
        /\ procs[procs[p].target].status \in {BlockedAwait, Failed}}

\* Rebuild waiters for deadlock cleanup.
FilteredWaiters(dead) ==
    [child \in DOMAIN procs |->
        {parent \in DOMAIN procs :
            /\ parent \notin dead
            /\ procs[parent].status = BlockedAwait
            /\ procs[parent].target = child}]

\* Queue / worker actions
DequeueRunnable(w, pid) ==
    /\ StepOK
    /\ ~shutdown
    /\ Idle(w)
    /\ KnownPid(pid)
    /\ AtHead(pid)
    /\ procs[pid].status = Runnable
    /\ procs' = [procs EXCEPT ![pid].status = Running]
    /\ queue' = Tail(queue)
    /\ workers' = [workers EXCEPT ![w] = pid]
    /\ step' = step + 1
    /\ UNCHANGED <<waiters, any_waiters, tombstones, pending_cancels, io_waiters, next_pid, next_token, shutdown>>

\* Scheduler entries can become stale relative to the process table/worker ownership.
DequeueStale(w, pid) ==
    /\ StepOK
    /\ ~shutdown
    /\ Idle(w)
    /\ KnownPid(pid)
    /\ AtHead(pid)
    /\ procs[pid].status # Runnable
    /\ queue' = Tail(queue)
    /\ step' = step + 1
    /\ UNCHANGED <<procs, workers, waiters, any_waiters, tombstones, pending_cancels, io_waiters, next_pid, next_token, shutdown>>

WorkerYield(w) ==
    /\ StepOK
    /\ ~shutdown
    /\ workers[w] # 0
    /\ LET pid == workers[w] IN
        /\ procs[pid].status = Running
        /\ procs' = [procs EXCEPT ![pid].status = Runnable]
        /\ queue' = Append(queue, pid)
        /\ workers' = [workers EXCEPT ![w] = 0]
        /\ step' = step + 1
        /\ UNCHANGED <<waiters, any_waiters, tombstones, pending_cancels, io_waiters, next_pid, next_token, shutdown>>

WorkerSpawn(w) ==
    /\ StepOK
    /\ ~shutdown
    /\ workers[w] # 0
    /\ next_pid <= MaxProcesses
    /\ LET pid == workers[w]
           child == next_pid
       IN /\ procs[pid].status = Running
          /\ procs' = [procs EXCEPT ![pid].status = Runnable] @@ (child :> EmptyProc(pid))
          /\ queue' = queue \o <<child, pid>>
          /\ workers' = [workers EXCEPT ![w] = 0]
          /\ waiters' = waiters @@ (child :> {})
          /\ any_waiters' = any_waiters @@ (child :> {})
          /\ tombstones' = tombstones @@ (child :> TombNone)
          /\ pending_cancels' = pending_cancels
          /\ next_pid' = next_pid + 1
          /\ step' = step + 1
          /\ UNCHANGED <<tombstones, pending_cancels, io_waiters, next_token, shutdown>>

WorkerAwaitBlock(w, child) ==
    /\ StepOK
    /\ ~shutdown
    /\ workers[w] # 0
    /\ KnownPid(child)
    /\ LET parent == workers[w] IN
        /\ procs[parent].status = Running
        /\ procs[child].parent = parent
        /\ procs[child].status \in {Runnable, Running, BlockedAwait, BlockedIo}
        /\ procs' = [procs EXCEPT ![parent].status = BlockedAwait,
                                   ![parent].target = child]
        /\ workers' = [workers EXCEPT ![w] = 0]
        /\ waiters' = [waiters EXCEPT ![child] = @ \union {parent}]
        /\ step' = step + 1
        /\ UNCHANGED <<queue, any_waiters, tombstones, pending_cancels, io_waiters, next_pid, next_token, shutdown>>

WorkerAwaitReadyTombstone(w, child) ==
    /\ StepOK
    /\ ~shutdown
    /\ workers[w] # 0
    /\ KnownPid(child)
    /\ tombstones[child] = TombReady
    /\ LET parent == workers[w] IN
        /\ procs[parent].status = Running
        /\ procs[child].parent = parent
        /\ procs' = [procs EXCEPT ![parent].status = Runnable,
                                   ![parent].target = 0]
        /\ queue' = Append(queue, parent)
        /\ workers' = [workers EXCEPT ![w] = 0]
        /\ tombstones' = [tombstones EXCEPT ![child] = TombConsumed]
        /\ step' = step + 1
        /\ UNCHANGED <<waiters, any_waiters, pending_cancels, io_waiters, next_pid, next_token, shutdown>>

WorkerWaitAnyReady(w, children) ==
    /\ StepOK
    /\ ~shutdown
    /\ workers[w] # 0
    /\ children \in WaitAnyInputs
    /\ LET parent == workers[w] IN
        /\ procs[parent].status = Running
        /\ \A i \in 1..Len(children) :
            /\ KnownPid(children[i])
            /\ procs[children[i]].parent = parent
        /\ \E i \in 1..Len(children) : TerminalStatus(procs[children[i]].status)
        /\ procs' = [procs EXCEPT ![parent].status = Runnable,
                                   ![parent].delivered_pid = LeftmostTerminalAfter(0, children)]
        /\ queue' = Append(queue, parent)
        /\ workers' = [workers EXCEPT ![w] = 0]
        /\ step' = step + 1
        /\ UNCHANGED <<waiters, any_waiters, tombstones, pending_cancels, io_waiters, next_pid, next_token, shutdown>>

WorkerWaitAnyBlock(w, children) ==
    /\ StepOK
    /\ ~shutdown
    /\ workers[w] # 0
    /\ children \in WaitAnyInputs
    /\ LET parent == workers[w] IN
        /\ procs[parent].status = Running
        /\ \A i \in 1..Len(children) :
            /\ KnownPid(children[i])
            /\ procs[children[i]].parent = parent
            /\ ~TerminalStatus(procs[children[i]].status)
        /\ procs' = [procs EXCEPT ![parent].status = BlockedAny,
                                   ![parent].targets = children,
                                   ![parent].target = 0]
        /\ workers' = [workers EXCEPT ![w] = 0]
        /\ any_waiters' = [child \in DOMAIN any_waiters |->
            IF child \in SeqSet(children)
            THEN any_waiters[child] \union {parent}
            ELSE any_waiters[child]]
        /\ step' = step + 1
        /\ UNCHANGED <<queue, waiters, tombstones, pending_cancels, io_waiters, next_pid, next_token, shutdown>>

WorkerCancelRunningChild(w, child) ==
    /\ StepOK
    /\ ~shutdown
    /\ workers[w] # 0
    /\ KnownPid(child)
    /\ procs[child].status = Running
    /\ LET parent == workers[w] IN
        /\ procs[parent].status = Running
        /\ procs[child].parent = parent
        /\ procs' = [procs EXCEPT ![parent].status = Runnable]
        /\ queue' = Append(queue, parent)
        /\ workers' = [workers EXCEPT ![w] = 0]
        /\ pending_cancels' = pending_cancels \union {child}
        /\ step' = step + 1
        /\ UNCHANGED <<waiters, any_waiters, tombstones, io_waiters, next_pid, next_token, shutdown>>

WorkerObservePendingCancel(w) ==
    /\ StepOK
    /\ ~shutdown
    /\ workers[w] # 0
    /\ LET pid == workers[w]
           any_parent == AnyWaiter(pid)
       IN
        /\ pid \in pending_cancels
        /\ procs[pid].status = Running
        /\ IF any_parent # 0
           THEN /\ procs' = [procs EXCEPT ![pid].status = Failed,
                                          ![any_parent].status = Runnable,
                                          ![any_parent].targets = <<>>,
                                          ![any_parent].delivered_pid = LeftmostTerminalAfter(pid, procs[any_parent].targets)]
                /\ queue' = Append(queue, any_parent)
                /\ any_waiters' = ClearAnyWaiterRegistrations(any_parent, procs[any_parent].targets)
           ELSE /\ procs' = [procs EXCEPT ![pid].status = Failed]
                /\ queue' = queue
                /\ any_waiters' = any_waiters
        /\ workers' = [workers EXCEPT ![w] = 0]
        /\ tombstones' = [tombstones EXCEPT ![pid] = IF procs[pid].parent = 0 THEN TombNone ELSE TombReady]
        /\ pending_cancels' = pending_cancels \ {pid}
        /\ step' = step + 1
        /\ UNCHANGED <<waiters, io_waiters, next_pid, next_token, shutdown>>

WorkerRequestIo(w) ==
    /\ StepOK
    /\ ~shutdown
    /\ workers[w] # 0
    /\ next_token <= MaxIoOps
    /\ LET pid == workers[w]
           tok == next_token
       IN /\ procs[pid].status = Running
          /\ procs' = [procs EXCEPT ![pid].status = BlockedIo,
                                     ![pid].target = tok]
          /\ workers' = [workers EXCEPT ![w] = 0]
          /\ io_waiters' = io_waiters \union {[pid |-> pid, token |-> tok]}
          /\ next_token' = next_token + 1
          /\ step' = step + 1
          /\ UNCHANGED <<queue, waiters, any_waiters, tombstones, pending_cancels, next_pid, shutdown>>

WorkerDone(w) ==
    /\ StepOK
    /\ ~shutdown
    /\ workers[w] # 0
    /\ LET pid == workers[w]
           any_parent == AnyWaiter(pid)
       IN
        /\ procs[pid].status = Running
        /\ IF waiters[pid] # {}
           THEN LET parent == CHOOSE p \in waiters[pid] : TRUE IN
                /\ procs' = [procs EXCEPT ![pid].status = Done,
                                           ![parent].status = Runnable,
                                           ![parent].target = 0]
                /\ queue' = Append(queue, parent)
                /\ waiters' = [waiters EXCEPT ![pid] = {}]
                /\ any_waiters' = any_waiters
                /\ tombstones' = [tombstones EXCEPT ![pid] = IF procs[pid].parent = 0 THEN TombNone ELSE TombConsumed]
           ELSE IF any_parent # 0
                THEN /\ procs' = [procs EXCEPT ![pid].status = Done,
                                               ![any_parent].status = Runnable,
                                               ![any_parent].targets = <<>>,
                                               ![any_parent].delivered_pid = LeftmostTerminalAfter(pid, procs[any_parent].targets)]
                     /\ queue' = Append(queue, any_parent)
                     /\ waiters' = waiters
                     /\ any_waiters' = ClearAnyWaiterRegistrations(any_parent, procs[any_parent].targets)
                     /\ tombstones' = [tombstones EXCEPT ![pid] = IF procs[pid].parent = 0 THEN TombNone ELSE TombReady]
                ELSE /\ procs' = [procs EXCEPT ![pid].status = Done]
                     /\ queue' = queue
                     /\ waiters' = waiters
                     /\ any_waiters' = any_waiters
                     /\ tombstones' = [tombstones EXCEPT ![pid] = IF procs[pid].parent = 0 THEN TombNone ELSE TombReady]
        /\ workers' = [workers EXCEPT ![w] = 0]
        /\ pending_cancels' = pending_cancels
        /\ step' = step + 1
        /\ UNCHANGED <<io_waiters, next_pid, next_token, shutdown>>

WorkerFail(w) ==
    /\ StepOK
    /\ ~shutdown
    /\ workers[w] # 0
    /\ LET pid == workers[w]
           any_parent == AnyWaiter(pid)
       IN
        /\ procs[pid].status = Running
        /\ IF waiters[pid] # {}
           THEN LET parent == CHOOSE p \in waiters[pid] : TRUE IN
                /\ procs' = [procs EXCEPT ![pid].status = Failed,
                                           ![parent].status = Failed,
                                           ![parent].target = 0]
                /\ waiters' = [waiters EXCEPT ![pid] = {}]
                /\ any_waiters' = any_waiters
                /\ tombstones' = [tombstones EXCEPT ![pid] = IF procs[pid].parent = 0 THEN TombNone ELSE TombConsumed]
           ELSE IF any_parent # 0
                THEN /\ procs' = [procs EXCEPT ![pid].status = Failed,
                                               ![any_parent].status = Runnable,
                                               ![any_parent].targets = <<>>,
                                               ![any_parent].delivered_pid = LeftmostTerminalAfter(pid, procs[any_parent].targets)]
                     /\ queue' = Append(queue, any_parent)
                     /\ waiters' = waiters
                     /\ any_waiters' = ClearAnyWaiterRegistrations(any_parent, procs[any_parent].targets)
                     /\ tombstones' = [tombstones EXCEPT ![pid] = IF procs[pid].parent = 0 THEN TombNone ELSE TombReady]
                ELSE /\ procs' = [procs EXCEPT ![pid].status = Failed]
                     /\ queue' = queue
                     /\ waiters' = waiters
                     /\ any_waiters' = any_waiters
                     /\ tombstones' = [tombstones EXCEPT ![pid] = IF procs[pid].parent = 0 THEN TombNone ELSE TombReady]
        /\ workers' = [workers EXCEPT ![w] = 0]
        /\ pending_cancels' = pending_cancels
        /\ step' = step + 1
        /\ UNCHANGED <<queue, io_waiters, next_pid, next_token, shutdown>>

\* Monitor actions
MonitorIoComplete(entry) ==
    /\ StepOK
    /\ ~shutdown
    /\ entry \in io_waiters
    /\ KnownPid(entry.pid)
    /\ procs[entry.pid].status = BlockedIo
    /\ procs[entry.pid].target = entry.token
    /\ procs' = [procs EXCEPT ![entry.pid].status = Runnable,
                               ![entry.pid].target = 0]
    /\ queue' = Append(queue, entry.pid)
    /\ io_waiters' = io_waiters \ {entry}
    /\ step' = step + 1
    /\ UNCHANGED <<workers, waiters, any_waiters, tombstones, pending_cancels, next_pid, next_token, shutdown>>

MonitorIoFail(entry) ==
    /\ StepOK
    /\ ~shutdown
    /\ entry \in io_waiters
    /\ KnownPid(entry.pid)
    /\ procs[entry.pid].status = BlockedIo
    /\ procs[entry.pid].target = entry.token
    /\ LET any_parent == AnyWaiter(entry.pid) IN
        /\ IF waiters[entry.pid] # {}
           THEN LET parent == CHOOSE p \in waiters[entry.pid] : TRUE IN
                /\ procs' = [procs EXCEPT ![entry.pid].status = Failed,
                                           ![entry.pid].target = 0,
                                           ![parent].status = Failed,
                                           ![parent].target = 0]
                /\ queue' = queue
                /\ waiters' = [waiters EXCEPT ![entry.pid] = {}]
                /\ any_waiters' = any_waiters
                /\ tombstones' = [tombstones EXCEPT ![entry.pid] = IF procs[entry.pid].parent = 0 THEN TombNone ELSE TombConsumed]
           ELSE IF any_parent # 0
                THEN /\ procs' = [procs EXCEPT ![entry.pid].status = Failed,
                                               ![entry.pid].target = 0,
                                               ![any_parent].status = Runnable,
                                               ![any_parent].targets = <<>>,
                                               ![any_parent].delivered_pid = LeftmostTerminalAfter(entry.pid, procs[any_parent].targets)]
                     /\ queue' = Append(queue, any_parent)
                     /\ waiters' = waiters
                     /\ any_waiters' = ClearAnyWaiterRegistrations(any_parent, procs[any_parent].targets)
                     /\ tombstones' = [tombstones EXCEPT ![entry.pid] = IF procs[entry.pid].parent = 0 THEN TombNone ELSE TombReady]
                ELSE /\ procs' = [procs EXCEPT ![entry.pid].status = Failed,
                                                ![entry.pid].target = 0]
                     /\ queue' = queue
                     /\ waiters' = waiters
                     /\ any_waiters' = any_waiters
                     /\ tombstones' = [tombstones EXCEPT ![entry.pid] = IF procs[entry.pid].parent = 0 THEN TombNone ELSE TombReady]
    /\ io_waiters' = io_waiters \ {entry}
    /\ pending_cancels' = pending_cancels \ {entry.pid}
    /\ step' = step + 1
    /\ UNCHANGED <<workers, next_pid, next_token, shutdown>>

MonitorDetectDeadlock ==
    /\ StepOK
    /\ ~shutdown
    /\ queue = <<>>
    /\ io_waiters = {}
    /\ \A w \in Workers : Idle(w)
    /\ StuckPids # {}
    /\ procs' = [p \in DOMAIN procs |->
        IF p \in StuckPids
        THEN [procs[p] EXCEPT !.status = Failed, !.target = 0]
        ELSE procs[p]]
    /\ waiters' = [child \in DOMAIN procs |->
        {parent \in DOMAIN procs :
            /\ parent \notin StuckPids
            /\ procs[parent].status = BlockedAwait
            /\ procs[parent].target = child}]
    /\ any_waiters' = any_waiters
    /\ shutdown' = TRUE
    /\ step' = step + 1
    /\ UNCHANGED <<queue, workers, tombstones, pending_cancels, io_waiters, next_pid, next_token>>

Next ==
    \/ \E w \in Workers :
        \/ \E pid \in DOMAIN procs : DequeueRunnable(w, pid)
        \/ \E pid \in DOMAIN procs : DequeueStale(w, pid)
        \/ WorkerYield(w)
        \/ WorkerSpawn(w)
        \/ WorkerRequestIo(w)
        \/ WorkerObservePendingCancel(w)
        \/ WorkerDone(w)
        \/ WorkerFail(w)
        \/ \E child \in DOMAIN procs : WorkerAwaitBlock(w, child)
        \/ \E child \in DOMAIN procs : WorkerAwaitReadyTombstone(w, child)
        \/ \E child \in DOMAIN procs : WorkerCancelRunningChild(w, child)
        \/ \E children \in WaitAnyInputs :
            \/ WorkerWaitAnyReady(w, children)
            \/ WorkerWaitAnyBlock(w, children)
    \/ \E entry \in io_waiters :
        \/ MonitorIoComplete(entry)
        \/ MonitorIoFail(entry)
    \/ MonitorDetectDeadlock

\* Invariants
TypeOK ==
    /\ next_pid \in Nat
    /\ next_token \in Nat
    /\ step \in Nat
    /\ shutdown \in BOOLEAN
    /\ \A p \in DOMAIN procs :
        /\ procs[p].status \in {Runnable, Running, BlockedAwait, BlockedAny, BlockedIo, Done, Failed}
        /\ procs[p].parent \in (DOMAIN procs) \union {0}
        /\ procs[p].target \in Nat \union {0}
        /\ IF procs[p].status = BlockedAny
           THEN procs[p].targets \in WaitAnyInputs
           ELSE procs[p].targets \in {<<>>} \union WaitAnyInputs
        /\ procs[p].delivered_pid \in Nat \union {0}
    /\ DOMAIN tombstones = DOMAIN procs
    /\ \A p \in DOMAIN tombstones : tombstones[p] \in {TombNone, TombReady, TombConsumed}
    /\ pending_cancels \subseteq DOMAIN procs
    /\ \A child \in DOMAIN any_waiters : any_waiters[child] \subseteq DOMAIN procs
    /\ \A w \in Workers : workers[w] \in (DOMAIN procs) \union {0}

QueueKnownPids ==
    \A i \in 1..Len(queue) : queue[i] \in DOMAIN procs

RunningHeldByExactlyOneWorker ==
    \A p \in DOMAIN procs :
        (procs[p].status = Running) => Cardinality(HeldBy(p)) = 1

OnlyHeldProcessesAreRunning ==
    \A w \in Workers :
        workers[w] # 0 => procs[workers[w]].status = Running

BlockedNeverHeld ==
    \A p \in DOMAIN procs :
        (procs[p].status \in {BlockedAwait, BlockedAny, BlockedIo, Done, Failed}) =>
            ~IsHeld(p)

AtMostOneWaiterPerChild ==
    \A child \in DOMAIN waiters :
        Cardinality(waiters[child]) <= 1

WaitersConsistent ==
    \A child \in DOMAIN waiters :
        \A parent \in waiters[child] :
            /\ parent \in DOMAIN procs
            /\ procs[parent].status = BlockedAwait
            /\ procs[parent].target = child

AnyWaitersConsistent ==
    \A child \in DOMAIN any_waiters :
        \A parent \in any_waiters[child] :
            /\ parent \in DOMAIN procs
            /\ procs[parent].status = BlockedAny
            /\ child \in SeqSet(procs[parent].targets)
            /\ procs[child].parent = parent

PendingCancelsConsistent ==
    \A p \in pending_cancels :
        /\ procs[p].status = Running
        /\ procs[p].parent # 0

TombstonesConsistent ==
    \A p \in DOMAIN procs :
        /\ procs[p].parent = 0 => tombstones[p] = TombNone
        /\ tombstones[p] \in {TombReady, TombConsumed} =>
            /\ procs[p].parent # 0
            /\ TerminalStatus(procs[p].status)

IoWaitersConsistent ==
    \A entry \in io_waiters :
        /\ entry.pid \in DOMAIN procs
        /\ procs[entry.pid].status = BlockedIo
        /\ procs[entry.pid].target = entry.token

ShutdownImpliesNoRunnableOrRunning ==
    shutdown =>
        /\ queue = <<>>
        /\ io_waiters = {}
        /\ \A w \in Workers : Idle(w)
        /\ \A p \in DOMAIN procs :
            procs[p].status \notin {Runnable, Running}

SafetyInvariant ==
    /\ TypeOK
    /\ QueueKnownPids
    /\ RunningHeldByExactlyOneWorker
    /\ OnlyHeldProcessesAreRunning
    /\ BlockedNeverHeld
    /\ AtMostOneWaiterPerChild
    /\ WaitersConsistent
    /\ AnyWaitersConsistent
    /\ PendingCancelsConsistent
    /\ TombstonesConsistent
    /\ IoWaitersConsistent
    /\ ShutdownImpliesNoRunnableOrRunning

Spec == Init /\ [][Next]_vars

\* ── Liveness ────────────────────────────────────────────────
\* Fairness: workers eventually run, monitor eventually polls

ImplFairness ==
    /\ \A w \in Workers :
        /\ WF_vars(WorkerDone(w))
        /\ WF_vars(WorkerFail(w))
    /\ \A entry \in [pid : 1..MaxProcesses, token : 1..MaxIoOps] :
        WF_vars(MonitorIoComplete(entry))
    /\ WF_vars(MonitorDetectDeadlock)

LiveSpec == Init /\ [][Next]_vars /\ ImplFairness

ImplParentWaitingOnTerminalChild(parent, child) ==
    IF /\ parent \in DOMAIN procs
       /\ child \in DOMAIN procs
    THEN /\ procs[child].status \in {Done, Failed}
         /\ parent \in waiters[child]
    ELSE FALSE

ImplParentUnblockedOrFailed(parent) ==
    IF parent \in DOMAIN procs
    THEN procs[parent].status \in {Runnable, Running, Failed}
    ELSE TRUE

\* If a parent is blocked awaiting a child that finished, it eventually unblocks
ImplParentEventuallyWoken ==
    \A child \in 1..MaxProcesses :
        \A parent \in 1..MaxProcesses :
            ImplParentWaitingOnTerminalChild(parent, child)
                ~> ImplParentUnblockedOrFailed(parent)

\* Every pending I/O entry eventually resolves
ImplIoEventuallyCompletes ==
    \A entry \in [pid : 1..MaxProcesses, token : 1..MaxIoOps] :
        [](entry \in io_waiters ~> entry \notin io_waiters)

====
