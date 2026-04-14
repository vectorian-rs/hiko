---- MODULE ProcessLifecycle ----
\* TLA+ specification for Hiko runtime process lifecycle.
\*
\* Models the full concurrency surface:
\*   - Scheduler with runnable queue, worker dequeue, yield
\*   - Spawn with parent-child relationship
\*   - Await with single-consumption, result delivery to parent
\*   - I/O blocking, backend completion, resume
\*   - Send/Receive with FIFO mailbox, direct delivery to blocked receiver
\*   - Failure propagation from child to awaiting parent
\*   - Deadlock detection (permanently blocked with no waker)
\*   - Unknown pid and non-child await cases
\*
\* Liveness properties with weak fairness.

EXTENDS Naturals, Sequences, FiniteSets, TLC

CONSTANTS
    MaxProcesses,
    MaxSteps,
    MaxMessages,
    MaxIoOps

VARIABLES
    \* Process state: Pid -> record
    procs,
    \* Scheduler: runnable queue (sequence of pids)
    runqueue,
    \* Mailboxes: Pid -> Seq(Nat)
    mailboxes,
    \* Await waiters: child Pid -> Set of parent Pids waiting
    waiters,
    \* I/O: set of [pid, token] records for in-flight operations
    io_pending,
    \* Allocation counters
    next_pid,
    io_next_token,
    \* Bounds
    msg_count,
    io_count,
    step

vars == <<procs, runqueue, mailboxes, waiters, io_pending,
          next_pid, io_next_token, msg_count, io_count, step>>

\* ── Status and block reasons ────────────────────────────────

Runnable == "runnable"
Done     == "done"
Failed   == "failed"
Blocked  == "blocked"

BkNone    == "none"
BkAwait   == "await"
BkIo      == "io"
BkReceive == "receive"

\* ── Process record constructor ──────────────────────────────

EmptyProc(parent) ==
    [status       |-> Runnable,
     parent       |-> parent,
     result       |-> -1,          \* -1 = no result yet
     delivered_to |-> 0,           \* pid that consumed the result, 0 = nobody
     block_reason |-> BkNone,
     block_target |-> 0]

\* ── Initial state ───────────────────────────────────────────

Init ==
    /\ procs = 1 :> EmptyProc(0)
    /\ runqueue = <<1>>
    /\ mailboxes = 1 :> <<>>
    /\ waiters = 1 :> {}
    /\ io_pending = {}
    /\ next_pid = 2
    /\ io_next_token = 1
    /\ msg_count = 0
    /\ io_count = 0
    /\ step = 0

\* ── Helpers ─────────────────────────────────────────────────

\* Remove first occurrence of e from sequence s
RemoveFirst(s, e) ==
    IF s = <<>> THEN <<>>
    ELSE IF Head(s) = e THEN Tail(s)
    ELSE <<Head(s)>> \o RemoveFirst(Tail(s), e)

InRunqueue(pid) == \E i \in 1..Len(runqueue) : runqueue[i] = pid

\* Scheduler actions execute the pid at the front of the queue.
AtHead(pid) == runqueue # <<>> /\ Head(runqueue) = pid

Enqueue(pid) == Append(runqueue, pid)

\* Dequeue: pick from front (FIFO scheduling)
Dequeue == IF runqueue = <<>> THEN 0 ELSE Head(runqueue)

\* ── Guard: step bound ───────────────────────────────────────

Step == step < MaxSteps

\* ── Spawn ───────────────────────────────────────────────────

Spawn(pid) ==
    /\ Step
    /\ procs[pid].status = Runnable
    /\ AtHead(pid)
    /\ next_pid <= MaxProcesses
    /\ LET child == next_pid IN
        /\ procs' = procs @@ (child :> EmptyProc(pid))
        \* Matches runtime: child is enqueued first, then parent resumes.
        /\ runqueue' = Tail(runqueue) \o <<child, pid>>
        /\ mailboxes' = mailboxes @@ (child :> <<>>)
        /\ waiters' = waiters @@ (child :> {})
        /\ next_pid' = next_pid + 1
        /\ step' = step + 1
        /\ UNCHANGED <<io_pending, io_next_token, msg_count, io_count>>

\* ── Complete ────────────────────────────────────────────────

\* Complete: atomically wake waiters with result delivery.
\* Matches implementation: worker sets Done, serializes result,
\* then calls wake_waiters in the same loop iteration.
Complete(pid) ==
    /\ Step
    /\ procs[pid].status = Runnable
    /\ AtHead(pid)
    /\ IF waiters[pid] = {}
       THEN
          /\ procs' = [procs EXCEPT ![pid].status = Done,
                                     ![pid].result = pid * 10]
          /\ runqueue' = Tail(runqueue)
          /\ waiters' = waiters
       ELSE
          /\ LET parent == CHOOSE p \in waiters[pid] : TRUE IN
              \* Matches runtime: completion wakes the blocked parent and
              \* consumes the child's result in the same worker iteration.
              /\ procs' = [p \in DOMAIN procs |->
                  IF p = pid
                  THEN [procs[p] EXCEPT !.status = Done,
                                         !.result = -1,
                                         !.delivered_to = parent]
                  ELSE IF p = parent
                  THEN [procs[p] EXCEPT !.status = Runnable,
                                         !.block_reason = BkNone,
                                         !.block_target = 0]
                  ELSE procs[p]]
              /\ runqueue' = Tail(runqueue) \o <<parent>>
              /\ waiters' = [waiters EXCEPT ![pid] = {}]
    /\ step' = step + 1
    /\ UNCHANGED <<next_pid, mailboxes, io_pending, io_next_token, msg_count, io_count>>

\* ── CompleteBadResult ────────────────────────────────────────
\* Process completes but its return value is not sendable
\* (closure, continuation, Rng). Serialization fails.
\* The child is marked Failed, NOT Done with Unit.
\* Matches implementation: serialize() returns Err, worker sets
\* Failed and calls wake_waiters.
\*
\* This prevents the old bug where unwrap_or(SendableValue::Unit)
\* silently delivered Unit to the parent instead of failing.

CompleteBadResult(pid) ==
    /\ Step
    /\ procs[pid].status = Runnable
    /\ AtHead(pid)
    /\ LET wake_set == waiters[pid] IN
        /\ procs' = [p \in DOMAIN procs |->
            IF p = pid
            THEN [procs[p] EXCEPT !.status = Failed]
            ELSE IF p \in wake_set
            THEN [procs[p] EXCEPT !.status = Failed,
                                   !.block_reason = BkNone,
                                   !.block_target = 0]
            ELSE procs[p]]
        /\ runqueue' = Tail(runqueue)
        /\ waiters' = [waiters EXCEPT ![pid] = {}]
        /\ step' = step + 1
        /\ UNCHANGED <<next_pid, mailboxes, io_pending, io_next_token, msg_count, io_count>>

\* ── Fail ────────────────────────────────────────────────────
\* Fail: atomically propagate failure to all waiters.
\* Matches implementation: worker sets Failed, then calls
\* wake_waiters which marks waiters as Failed.

Fail(pid) ==
    /\ Step
    /\ procs[pid].status = Runnable
    /\ AtHead(pid)
    /\ LET wake_set == waiters[pid] IN
        /\ procs' = [p \in DOMAIN procs |->
            IF p = pid
            THEN [procs[p] EXCEPT !.status = Failed]
            ELSE IF p \in wake_set
            THEN [procs[p] EXCEPT !.status = Failed,
                                   !.block_reason = BkNone,
                                   !.block_target = 0]
            ELSE procs[p]]
        /\ runqueue' = Tail(runqueue)
        /\ waiters' = [waiters EXCEPT ![pid] = {}]
        /\ step' = step + 1
        /\ UNCHANGED <<next_pid, mailboxes, io_pending, io_next_token, msg_count, io_count>>

\* ── Yield ───────────────────────────────────────────────────
\* Process gives up its time slice, goes to back of queue.

Yield(pid) ==
    /\ Step
    /\ procs[pid].status = Runnable
    /\ AtHead(pid)
    /\ runqueue' = Tail(runqueue) \o <<pid>>
    /\ step' = step + 1
    /\ UNCHANGED <<procs, next_pid, mailboxes, waiters, io_pending, io_next_token, msg_count, io_count>>

\* ── Await (child done, result available) ────────────────────

AwaitDone(parent, child) ==
    /\ Step
    /\ procs[parent].status = Runnable
    /\ AtHead(parent)
    /\ child \in DOMAIN procs
    /\ procs[child].parent = parent
    /\ procs[child].status = Done
    /\ procs[child].result # -1
    /\ procs[child].delivered_to = 0
    \* Deliver: parent stays runnable, child result consumed
    /\ procs' = [procs EXCEPT ![child].result = -1,
                               ![child].delivered_to = parent]
    /\ runqueue' = Tail(runqueue) \o <<parent>>
    /\ step' = step + 1
    /\ UNCHANGED <<next_pid, mailboxes, waiters, io_pending, io_next_token, msg_count, io_count>>

\* ── Await (child done, result already consumed) ─────────────

AwaitConsumed(parent, child) ==
    /\ Step
    /\ procs[parent].status = Runnable
    /\ AtHead(parent)
    /\ child \in DOMAIN procs
    /\ procs[child].parent = parent
    /\ procs[child].status = Done
    /\ procs[child].delivered_to # 0     \* already consumed
    /\ procs' = [procs EXCEPT ![parent].status = Failed]
    /\ runqueue' = Tail(runqueue)
    /\ step' = step + 1
    /\ UNCHANGED <<next_pid, mailboxes, waiters, io_pending, io_next_token, msg_count, io_count>>

\* ── Await (child failed) ────────────────────────────────────

AwaitFailed(parent, child) ==
    /\ Step
    /\ procs[parent].status = Runnable
    /\ AtHead(parent)
    /\ child \in DOMAIN procs
    /\ procs[child].parent = parent
    /\ procs[child].status = Failed
    /\ procs' = [procs EXCEPT ![parent].status = Failed]
    /\ runqueue' = Tail(runqueue)
    /\ step' = step + 1
    /\ UNCHANGED <<next_pid, mailboxes, waiters, io_pending, io_next_token, msg_count, io_count>>

\* ── Await (child still running — block parent) ──────────────

AwaitBlock(parent, child) ==
    /\ Step
    /\ procs[parent].status = Runnable
    /\ AtHead(parent)
    /\ child \in DOMAIN procs
    /\ procs[child].parent = parent
    /\ procs[child].status \in {Runnable, Blocked}
    /\ procs' = [procs EXCEPT ![parent].status = Blocked,
                               ![parent].block_reason = BkAwait,
                               ![parent].block_target = child]
    /\ runqueue' = Tail(runqueue)
    /\ waiters' = [waiters EXCEPT ![child] = @ \union {parent}]
    /\ step' = step + 1
    /\ UNCHANGED <<next_pid, mailboxes, io_pending, io_next_token, msg_count, io_count>>

\* ── Await (unknown pid — not in process table) ──────────────

AwaitUnknown(parent, child_pid) ==
    /\ Step
    /\ procs[parent].status = Runnable
    /\ AtHead(parent)
    /\ child_pid \notin DOMAIN procs
    /\ child_pid \in 1..MaxProcesses      \* plausible pid range
    /\ procs' = [procs EXCEPT ![parent].status = Failed]
    /\ runqueue' = Tail(runqueue)
    /\ step' = step + 1
    /\ UNCHANGED <<next_pid, mailboxes, waiters, io_pending, io_next_token, msg_count, io_count>>

\* ── Await (not parent's child) ──────────────────────────────

AwaitNotChild(parent, child) ==
    /\ Step
    /\ procs[parent].status = Runnable
    /\ AtHead(parent)
    /\ child \in DOMAIN procs
    /\ procs[child].parent # parent
    /\ procs' = [procs EXCEPT ![parent].status = Failed]
    /\ runqueue' = Tail(runqueue)
    /\ step' = step + 1
    /\ UNCHANGED <<next_pid, mailboxes, waiters, io_pending, io_next_token, msg_count, io_count>>

\* Note: WakeDone/WakeFailed are handled atomically inside
\* Complete/Fail above, matching the implementation where
\* wake_waiters is called in the same worker loop iteration.

\* ── I/O: request ────────────────────────────────────────────

RequestIo(pid) ==
    /\ Step
    /\ procs[pid].status = Runnable
    /\ AtHead(pid)
    /\ io_count < MaxIoOps
    /\ LET token == io_next_token IN
        /\ procs' = [procs EXCEPT ![pid].status = Blocked,
                                   ![pid].block_reason = BkIo,
                                   ![pid].block_target = token]
        /\ runqueue' = Tail(runqueue)
        /\ io_pending' = io_pending \union {[pid |-> pid, token |-> token]}
        /\ io_next_token' = io_next_token + 1
        /\ io_count' = io_count + 1
        /\ step' = step + 1
        /\ UNCHANGED <<next_pid, mailboxes, waiters, msg_count>>

\* ── I/O: backend completes (success) ────────────────────────

IoComplete(entry) ==
    /\ Step
    /\ entry \in io_pending
    /\ procs[entry.pid].status = Blocked
    /\ procs[entry.pid].block_reason = BkIo
    /\ procs[entry.pid].block_target = entry.token
    /\ procs' = [procs EXCEPT ![entry.pid].status = Runnable,
                               ![entry.pid].block_reason = BkNone,
                               ![entry.pid].block_target = 0]
    /\ runqueue' = Append(runqueue, entry.pid)
    /\ io_pending' = io_pending \ {entry}
    /\ step' = step + 1
    /\ UNCHANGED <<next_pid, mailboxes, waiters, io_next_token, msg_count, io_count>>

\* ── I/O: backend completes (failure) ────────────────────────

IoFail(entry) ==
    /\ Step
    /\ entry \in io_pending
    /\ procs[entry.pid].status = Blocked
    /\ procs[entry.pid].block_reason = BkIo
    /\ procs[entry.pid].block_target = entry.token
    /\ LET wake_set == waiters[entry.pid] IN
        /\ procs' = [p \in DOMAIN procs |->
            IF p = entry.pid
            THEN [procs[p] EXCEPT !.status = Failed,
                                   !.block_reason = BkNone,
                                   !.block_target = 0]
            ELSE IF p \in wake_set
            THEN [procs[p] EXCEPT !.status = Failed,
                                   !.block_reason = BkNone,
                                   !.block_target = 0]
            ELSE procs[p]]
    /\ io_pending' = io_pending \ {entry}
    /\ waiters' = [waiters EXCEPT ![entry.pid] = {}]
    /\ step' = step + 1
    /\ UNCHANGED <<next_pid, runqueue, mailboxes, io_next_token, msg_count, io_count>>

\* ── Send ────────────────────────────────────────────────────

Send(sender, target) ==
    /\ Step
    /\ procs[sender].status = Runnable
    /\ AtHead(sender)
    /\ target \in DOMAIN procs
    /\ msg_count < MaxMessages
    /\ LET msg == sender * 100 IN
        \* Self-send queues into the sender's own mailbox.
        /\ IF target = sender
           THEN
              /\ UNCHANGED procs
              /\ runqueue' = Tail(runqueue) \o <<sender>>
              /\ mailboxes' = [mailboxes EXCEPT ![sender] = Append(@, msg)]
           \* If target is blocked on receive, deliver directly and wake.
           ELSE IF procs[target].status = Blocked
                   /\ procs[target].block_reason = BkReceive
           THEN
              /\ procs' = [procs EXCEPT ![target].status = Runnable,
                                         ![target].block_reason = BkNone,
                                         ![target].block_target = 0]
              /\ runqueue' = Tail(runqueue) \o <<target, sender>>
              /\ mailboxes' = mailboxes
           ELSE
              /\ UNCHANGED procs
              /\ runqueue' = Tail(runqueue) \o <<sender>>
              /\ mailboxes' = [mailboxes EXCEPT ![target] = Append(@, msg)]
        /\ msg_count' = msg_count + 1
        /\ step' = step + 1
        /\ UNCHANGED <<next_pid, waiters, io_pending, io_next_token, io_count>>

SendUnknown(sender, target_pid) ==
    /\ Step
    /\ procs[sender].status = Runnable
    /\ AtHead(sender)
    /\ target_pid \notin DOMAIN procs
    /\ target_pid \in 1..MaxProcesses
    /\ procs' = [procs EXCEPT ![sender].status = Failed]
    /\ runqueue' = Tail(runqueue)
    /\ step' = step + 1
    /\ UNCHANGED <<next_pid, mailboxes, waiters, io_pending, io_next_token, msg_count, io_count>>

\* ── Receive (mailbox non-empty) ─────────────────────────────

ReceiveReady(pid) ==
    /\ Step
    /\ procs[pid].status = Runnable
    /\ AtHead(pid)
    /\ Len(mailboxes[pid]) > 0
    /\ mailboxes' = [mailboxes EXCEPT ![pid] = Tail(@)]
    /\ runqueue' = Tail(runqueue) \o <<pid>>
    /\ step' = step + 1
    /\ UNCHANGED <<procs, next_pid, waiters, io_pending, io_next_token, msg_count, io_count>>

\* ── Receive (mailbox empty — block) ─────────────────────────

ReceiveBlock(pid) ==
    /\ Step
    /\ procs[pid].status = Runnable
    /\ AtHead(pid)
    /\ Len(mailboxes[pid]) = 0
    /\ procs' = [procs EXCEPT ![pid].status = Blocked,
                               ![pid].block_reason = BkReceive,
                               ![pid].block_target = 0]
    /\ runqueue' = Tail(runqueue)
    /\ step' = step + 1
    /\ UNCHANGED <<next_pid, mailboxes, waiters, io_pending, io_next_token, msg_count, io_count>>

\* ── Deadlock detection ──────────────────────────────────────
\* If a process is blocked and cannot possibly be woken:
\*   - Blocked on Await: child is also blocked/failed with no waker
\*   - Blocked on Receive: no runnable process can send to it
\*   - Blocked on Io: always has a waker (io_pending entry), so never permanently stuck
\* The monitor marks permanently blocked processes as Failed.

DetectDeadlock(pid) ==
    /\ Step
    /\ procs[pid].status = Blocked
    /\ runqueue = <<>>          \* no runnable processes
    /\ io_pending = {}          \* no pending I/O that could resume anything
    \* This process has no possible waker
    /\ \/ procs[pid].block_reason = BkReceive
       \/ /\ procs[pid].block_reason = BkAwait
          /\ LET child == procs[pid].block_target IN
              procs[child].status \in {Blocked, Failed}
    /\ procs' = [procs EXCEPT ![pid].status = Failed,
                               ![pid].block_reason = BkNone,
                               ![pid].block_target = 0]
    /\ waiters' =
        IF procs[pid].block_reason = BkAwait
        THEN LET child == procs[pid].block_target IN
             [waiters EXCEPT ![child] = @ \ {pid}]
        ELSE waiters
    /\ step' = step + 1
    /\ UNCHANGED <<next_pid, runqueue, mailboxes, io_pending, io_next_token, msg_count, io_count>>

\* ── Next state relation ─────────────────────────────────────

Next ==
    \/ \E pid \in DOMAIN procs :
        \/ Spawn(pid)
        \/ Complete(pid)
        \/ CompleteBadResult(pid)
        \/ Fail(pid)
        \/ Yield(pid)
        \/ RequestIo(pid)
        \/ ReceiveReady(pid)
        \/ ReceiveBlock(pid)
        \/ DetectDeadlock(pid)
        \/ \E child \in DOMAIN procs : AwaitDone(pid, child)
        \/ \E child \in DOMAIN procs : AwaitConsumed(pid, child)
        \/ \E child \in DOMAIN procs : AwaitFailed(pid, child)
        \/ \E child \in DOMAIN procs : AwaitBlock(pid, child)
        \/ \E child \in DOMAIN procs : AwaitNotChild(pid, child)
        \/ \E target \in DOMAIN procs : Send(pid, target)
        \/ \E child_pid \in 1..MaxProcesses : AwaitUnknown(pid, child_pid)
        \/ \E target_pid \in 1..MaxProcesses : SendUnknown(pid, target_pid)
    \/ \E entry \in io_pending :
        \/ IoComplete(entry)
        \/ IoFail(entry)

\* ── Safety invariants ───────────────────────────────────────

\* All fields in valid ranges
TypeOK ==
    /\ next_pid \in Nat
    /\ io_next_token \in Nat
    /\ msg_count \in Nat
    /\ io_count \in Nat
    /\ step \in Nat
    /\ \A p \in DOMAIN procs :
        /\ procs[p].status \in {Runnable, Done, Failed, Blocked}
        /\ procs[p].parent \in (DOMAIN procs) \union {0}
        /\ procs[p].result \in Nat \union {-1}
        /\ procs[p].delivered_to \in (DOMAIN procs) \union {0}
        /\ procs[p].block_reason \in {BkNone, BkAwait, BkIo, BkReceive}
        /\ procs[p].block_target \in Nat \union {0}

\* Result delivered to exactly one parent (if delivered at all)
ResultDeliveredToParentOnly ==
    \A p \in DOMAIN procs :
        procs[p].delivered_to # 0 =>
            procs[p].delivered_to = procs[p].parent

\* Result delivered at most once
ResultDeliveredAtMostOnce ==
    \A p \in DOMAIN procs :
        procs[p].status = Done /\ procs[p].delivered_to # 0 =>
            procs[p].result = -1

\* Runnable processes are in the runqueue
RunnableInQueue ==
    \A p \in DOMAIN procs :
        procs[p].status = Runnable => InRunqueue(p)

\* Blocked processes are NOT in the runqueue
BlockedNotInQueue ==
    \A p \in DOMAIN procs :
        procs[p].status \in {Blocked, Done, Failed} => ~InRunqueue(p)

\* Blocked process has a valid block reason
BlockedIsConsistent ==
    \A p \in DOMAIN procs :
        /\ procs[p].status = Blocked =>
            procs[p].block_reason \in {BkAwait, BkIo, BkReceive}
        /\ procs[p].status # Blocked =>
            procs[p].block_reason = BkNone

\* Only parent can await (await target is parent's child)
OnlyParentAwaits ==
    \A p \in DOMAIN procs :
        (procs[p].status = Blocked /\ procs[p].block_reason = BkAwait) =>
            LET child == procs[p].block_target IN
                /\ child \in DOMAIN procs
                /\ procs[child].parent = p

\* No circular await
NoCircularAwait ==
    \A p, q \in DOMAIN procs :
        ~(procs[p].status = Blocked /\ procs[p].block_reason = BkAwait
          /\ procs[p].block_target = q
          /\ procs[q].status = Blocked /\ procs[q].block_reason = BkAwait
          /\ procs[q].block_target = p)

\* Every I/O-blocked process has a matching pending entry
IoBlockedHasPending ==
    \A p \in DOMAIN procs :
        (procs[p].status = Blocked /\ procs[p].block_reason = BkIo) =>
            \E entry \in io_pending :
                entry.pid = p /\ entry.token = procs[p].block_target

\* Every pending I/O has a blocked process
NoOrphanedIo ==
    \A entry \in io_pending :
        /\ entry.pid \in DOMAIN procs
        /\ procs[entry.pid].status = Blocked
        /\ procs[entry.pid].block_reason = BkIo

\* Every process has a mailbox
MailboxExists ==
    \A p \in DOMAIN procs : p \in DOMAIN mailboxes

\* Every process has a waiters set
WaitersExists ==
    \A p \in DOMAIN procs : p \in DOMAIN waiters

\* Waiters are consistent: every waiter is blocked on that child
WaitersConsistent ==
    \A child \in DOMAIN waiters :
        \A parent \in waiters[child] :
            /\ parent \in DOMAIN procs
            /\ procs[parent].status = Blocked
            /\ procs[parent].block_reason = BkAwait
            /\ procs[parent].block_target = child

\* A non-sendable result never sneaks through as Done.
\* If serialization fails, the process must be Failed, not Done.
\* This prevents the old bug: unwrap_or(Unit) silently delivering Unit.
NoSilentSerializationFailure ==
    \A p \in DOMAIN procs :
        procs[p].status = Done =>
            \/ procs[p].result # -1          \* has a valid result
            \/ procs[p].delivered_to # 0     \* result was already consumed

\* Every blocked await is registered under its child.
BlockedAwaitListed ==
    \A parent \in DOMAIN procs :
        (procs[parent].status = Blocked /\ procs[parent].block_reason = BkAwait) =>
            LET child == procs[parent].block_target IN
                /\ child \in DOMAIN waiters
                /\ parent \in waiters[child]

\* Parent-only await implies at most one waiter per child.
AtMostOneWaiterPerChild ==
    \A child \in DOMAIN waiters :
        Cardinality(waiters[child]) <= 1

\* ── Liveness (use --check-liveness for SCC analysis) ────────

\* Any pending I/O eventually resolves, either with success or failure.
ResolveIo(entry) == IoComplete(entry) \/ IoFail(entry)

Fairness ==
    /\ \A pid \in 1..MaxProcesses :
        /\ WF_vars(DetectDeadlock(pid))
    /\ \A entry \in [pid : 1..MaxProcesses, token : 1..MaxIoOps] :
        WF_vars(ResolveIo(entry))

\* Safety-only step relation for cfg files that use INIT/NEXT.
SafetySpec == Init /\ [][Next]_vars

\* Fair spec used by tla-checker liveness extraction.
Spec == SafetySpec /\ Fairness

\* Helpers keep the liveness formulas defined for as-yet-unallocated pids.
ParentWaitingOnTerminalChild(parent, child) ==
    IF /\ parent \in DOMAIN procs
       /\ child \in DOMAIN procs
    THEN /\ procs[parent].status = Blocked
         /\ procs[parent].block_reason = BkAwait
         /\ procs[parent].block_target = child
         /\ procs[child].status \in {Done, Failed}
    ELSE FALSE

ParentUnblockedOrGone(parent) ==
    IF parent \in DOMAIN procs
    THEN procs[parent].status # Blocked
    ELSE TRUE

ParentEventuallyWoken ==
    \A child \in 1..MaxProcesses :
        \A parent \in 1..MaxProcesses :
            ParentWaitingOnTerminalChild(parent, child)
                ~> ParentUnblockedOrGone(parent)

IoEventuallyCompletes ==
    \A entry \in [pid : 1..MaxProcesses, token : 1..MaxIoOps] :
        entry \in io_pending ~> entry \notin io_pending

\* ── Combined invariant ──────────────────────────────────────

SafetyInvariant ==
    /\ TypeOK
    /\ ResultDeliveredToParentOnly
    /\ ResultDeliveredAtMostOnce
    /\ RunnableInQueue
    /\ BlockedNotInQueue
    /\ BlockedIsConsistent
    /\ OnlyParentAwaits
    /\ NoCircularAwait
    /\ IoBlockedHasPending
    /\ NoOrphanedIo
    /\ MailboxExists
    /\ WaitersExists
    /\ WaitersConsistent
    /\ BlockedAwaitListed
    /\ AtMostOneWaiterPerChild
    /\ NoSilentSerializationFailure

====
