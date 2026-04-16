---- MODULE ThreadedSchedulerImpl ----
\* Lower-level TLA+ model of the threaded runtime structure.
\*
\* Focus:
\*   - explicit worker ownership of running processes
\*   - scheduler queue semantics
\*   - waiters/io waiters as runtime data structures
\*   - monitor-driven I/O resolution and deadlock shutdown
\*
\* This complements ProcessLifecycle.tla:
\*   - ProcessLifecycle models user-visible process semantics
\*   - ThreadedSchedulerImpl models the implementation structure
\*     in crates/hiko-vm/src/threaded.rs

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
    io_waiters,
    next_pid,
    next_token,
    shutdown,
    step

vars == <<procs, queue, workers, waiters, io_waiters, next_pid, next_token, shutdown, step>>

Runnable       == "runnable"
Running        == "running"
BlockedAwait   == "blocked_await"
BlockedReceive == "blocked_receive"
BlockedIo      == "blocked_io"
Done           == "done"
Failed         == "failed"

EmptyProc(parent) ==
    [status |-> Runnable,
     parent |-> parent,
     target |-> 0]

Init ==
    /\ procs = 1 :> EmptyProc(0)
    /\ queue = <<1>>
    /\ workers = [w \in Workers |-> 0]
    /\ waiters = 1 :> {}
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

HeldBy(pid) == {w \in Workers : workers[w] = pid}
IsHeld(pid) == HeldBy(pid) # {}

StuckPids ==
    {p \in DOMAIN procs :
        \/ procs[p].status = BlockedReceive
        \/ /\ procs[p].status = BlockedAwait
           /\ procs[p].target \in DOMAIN procs
           /\ procs[procs[p].target].status \in {BlockedAwait, BlockedReceive, Failed}}

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
    /\ UNCHANGED <<waiters, io_waiters, next_pid, next_token, shutdown>>

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
    /\ UNCHANGED <<procs, workers, waiters, io_waiters, next_pid, next_token, shutdown>>

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
        /\ UNCHANGED <<waiters, io_waiters, next_pid, next_token, shutdown>>

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
          /\ next_pid' = next_pid + 1
          /\ step' = step + 1
          /\ UNCHANGED <<io_waiters, next_token, shutdown>>

WorkerAwaitBlock(w, child) ==
    /\ StepOK
    /\ ~shutdown
    /\ workers[w] # 0
    /\ KnownPid(child)
    /\ LET parent == workers[w] IN
        /\ procs[parent].status = Running
        /\ procs[child].parent = parent
        /\ procs[child].status \in {Runnable, Running, BlockedAwait, BlockedReceive, BlockedIo}
        /\ procs' = [procs EXCEPT ![parent].status = BlockedAwait,
                                   ![parent].target = child]
        /\ workers' = [workers EXCEPT ![w] = 0]
        /\ waiters' = [waiters EXCEPT ![child] = @ \union {parent}]
        /\ step' = step + 1
        /\ UNCHANGED <<queue, io_waiters, next_pid, next_token, shutdown>>

WorkerBlockReceive(w) ==
    /\ StepOK
    /\ ~shutdown
    /\ workers[w] # 0
    /\ LET pid == workers[w] IN
        /\ procs[pid].status = Running
        /\ procs' = [procs EXCEPT ![pid].status = BlockedReceive]
        /\ workers' = [workers EXCEPT ![w] = 0]
        /\ step' = step + 1
        /\ UNCHANGED <<queue, waiters, io_waiters, next_pid, next_token, shutdown>>

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
          /\ UNCHANGED <<queue, waiters, next_pid, shutdown>>

WorkerDone(w) ==
    /\ StepOK
    /\ ~shutdown
    /\ workers[w] # 0
    /\ LET pid == workers[w] IN
        /\ procs[pid].status = Running
        /\ IF waiters[pid] = {}
           THEN /\ procs' = [procs EXCEPT ![pid].status = Done]
                /\ queue' = queue
                /\ waiters' = waiters
           ELSE LET parent == CHOOSE p \in waiters[pid] : TRUE IN
                /\ procs' = [procs EXCEPT ![pid].status = Done,
                                           ![parent].status = Runnable,
                                           ![parent].target = 0]
                /\ queue' = Append(queue, parent)
                /\ waiters' = [waiters EXCEPT ![pid] = {}]
        /\ workers' = [workers EXCEPT ![w] = 0]
        /\ step' = step + 1
        /\ UNCHANGED <<io_waiters, next_pid, next_token, shutdown>>

WorkerFail(w) ==
    /\ StepOK
    /\ ~shutdown
    /\ workers[w] # 0
    /\ LET pid == workers[w] IN
        /\ procs[pid].status = Running
        /\ IF waiters[pid] = {}
           THEN /\ procs' = [procs EXCEPT ![pid].status = Failed]
                /\ waiters' = waiters
           ELSE LET parent == CHOOSE p \in waiters[pid] : TRUE IN
                /\ procs' = [procs EXCEPT ![pid].status = Failed,
                                           ![parent].status = Failed,
                                           ![parent].target = 0]
                /\ waiters' = [waiters EXCEPT ![pid] = {}]
        /\ workers' = [workers EXCEPT ![w] = 0]
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
    /\ UNCHANGED <<workers, waiters, next_pid, next_token, shutdown>>

MonitorIoFail(entry) ==
    /\ StepOK
    /\ ~shutdown
    /\ entry \in io_waiters
    /\ KnownPid(entry.pid)
    /\ procs[entry.pid].status = BlockedIo
    /\ procs[entry.pid].target = entry.token
    /\ IF waiters[entry.pid] = {}
       THEN /\ procs' = [procs EXCEPT ![entry.pid].status = Failed,
                                        ![entry.pid].target = 0]
            /\ waiters' = waiters
       ELSE LET parent == CHOOSE p \in waiters[entry.pid] : TRUE IN
            /\ procs' = [procs EXCEPT ![entry.pid].status = Failed,
                                       ![entry.pid].target = 0,
                                       ![parent].status = Failed,
                                       ![parent].target = 0]
            /\ waiters' = [waiters EXCEPT ![entry.pid] = {}]
    /\ io_waiters' = io_waiters \ {entry}
    /\ step' = step + 1
    /\ UNCHANGED <<queue, workers, next_pid, next_token, shutdown>>

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
    /\ shutdown' = TRUE
    /\ step' = step + 1
    /\ UNCHANGED <<queue, workers, io_waiters, next_pid, next_token>>

Next ==
    \/ \E w \in Workers :
        \/ \E pid \in DOMAIN procs : DequeueRunnable(w, pid)
        \/ \E pid \in DOMAIN procs : DequeueStale(w, pid)
        \/ WorkerYield(w)
        \/ WorkerSpawn(w)
        \/ WorkerBlockReceive(w)
        \/ WorkerRequestIo(w)
        \/ WorkerDone(w)
        \/ WorkerFail(w)
        \/ \E child \in DOMAIN procs : WorkerAwaitBlock(w, child)
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
        /\ procs[p].status \in {Runnable, Running, BlockedAwait, BlockedReceive, BlockedIo, Done, Failed}
        /\ procs[p].parent \in (DOMAIN procs) \union {0}
        /\ procs[p].target \in Nat \union {0}
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
        (procs[p].status \in {BlockedAwait, BlockedReceive, BlockedIo, Done, Failed}) =>
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

\* If a parent is blocked awaiting a child that finished, it eventually unblocks
ImplParentEventuallyWoken ==
    \A child \in 1..MaxProcesses :
        \A parent \in 1..MaxProcesses :
            [](procs[child].status \in {Done, Failed}
               /\ parent \in waiters[child]
               ~> procs[parent].status \in {Runnable, Running, Failed})

\* Every pending I/O entry eventually resolves
ImplIoEventuallyCompletes ==
    \A entry \in [pid : 1..MaxProcesses, token : 1..MaxIoOps] :
        [](entry \in io_waiters ~> entry \notin io_waiters)

====
