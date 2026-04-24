---- MODULE ProcessLifecycle ----
\* Semantic TLA+ specification for Hiko process lifecycle.
\*
\* Focus:
\*   - FIFO scheduler with runnable queue
\*   - Spawn with parent-child ownership
\*   - Await/AwaitResult with single-consumption join state
\*   - wait_any over a parent-owned child set
\*   - Cooperative cancellation and parent-exit scope cleanup
\*   - I/O blocking and completion
\*   - Deadlock detection for permanently blocked processes
\*
\* This spec models user-visible behavior. It does not model the threaded
\* runtime tables (`child_parents`, tombstones, pending_cancels, publishing
\* windows); those belong in ThreadedSchedulerImpl.tla.

EXTENDS Naturals, Sequences, FiniteSets, TLC

CONSTANTS
    MaxProcesses,
    MaxSteps,
    MaxIoOps

VARIABLES
    procs,
    runqueue,
    waiters,
    any_waiters,
    io_pending,
    next_pid,
    io_next_token,
    io_count,
    step

vars ==
    <<procs, runqueue, waiters, any_waiters, io_pending,
      next_pid, io_next_token, io_count, step>>

\* ── Status / runtime surface ────────────────────────────────

Runnable == "runnable"
Blocked  == "blocked"
Done     == "done"
Failed   == "failed"

BkNone        == "none"
BkAwaitRaw    == "await_raw"
BkAwaitResult == "await_result"
BkWaitAny     == "wait_any"
BkIo          == "io"

JoinNone           == "join_none"
JoinReadyOk        == "join_ready_ok"
JoinReadyErr       == "join_ready_err"
JoinReadyCancelled == "join_ready_cancelled"
JoinConsumed       == "join_consumed"

DelNone                 == "del_none"
DelValue                == "del_value"
DelPid                  == "del_pid"
DelJoinOk               == "del_join_ok"
DelJoinErrRuntime       == "del_join_err_runtime"
DelJoinErrCancelled     == "del_join_err_cancelled"
DelJoinErrAlreadyJoined == "del_join_err_already_joined"
DelUnit                 == "del_unit"

\* ── Process record ──────────────────────────────────────────

EmptyProc(parent) ==
    [status           |-> Runnable,
     parent           |-> parent,
     join_state       |-> JoinNone,
     block_reason     |-> BkNone,
     block_target     |-> 0,
     block_targets    |-> {},
     cancel_requested |-> FALSE,
     delivered_kind   |-> DelNone,
     delivered_pid    |-> 0]

ClearBlock(proc) ==
    [proc EXCEPT
        !.block_reason = BkNone,
        !.block_target = 0,
        !.block_targets = {}]

WithDelivered(proc, kind, pid) ==
    [proc EXCEPT
        !.delivered_kind = kind,
        !.delivered_pid = pid]

SetStatus(proc, status) == [proc EXCEPT !.status = status]

SetJoinState(proc, join_state) == [proc EXCEPT !.join_state = join_state]

SetCancelRequested(proc, requested) == [proc EXCEPT !.cancel_requested = requested]

BlockOnAwait(proc, reason, child) ==
    [proc EXCEPT
        !.status = Blocked,
        !.block_reason = reason,
        !.block_target = child,
        !.block_targets = {}]

BlockOnWaitAny(proc, children) ==
    [proc EXCEPT
        !.status = Blocked,
        !.block_reason = BkWaitAny,
        !.block_target = 0,
        \* wait_any preserves the caller's original PID order. Completion is
        \* only a wakeup signal; the winner is selected by LeftmostTerminal.
        !.block_targets = children]

BlockOnIo(proc, token) ==
    [proc EXCEPT
        !.status = Blocked,
        !.block_reason = BkIo,
        !.block_target = token,
        !.block_targets = {}]

JoinReadyStates == {JoinReadyOk, JoinReadyErr, JoinReadyCancelled}

\* ── Initial state ───────────────────────────────────────────

Init ==
    /\ procs = 1 :> EmptyProc(0)
    /\ runqueue = <<1>>
    /\ waiters = 1 :> {}
    /\ any_waiters = 1 :> {}
    /\ io_pending = {}
    /\ next_pid = 2
    /\ io_next_token = 1
    /\ io_count = 0
    /\ step = 0

\* ── Helpers ─────────────────────────────────────────────────

Step == step < MaxSteps

KnownPid(pid) == pid \in DOMAIN procs

AtHead(pid) == runqueue # <<>> /\ Head(runqueue) = pid

InRunqueue(pid) == \E i \in 1..Len(runqueue) : runqueue[i] = pid

UnknownPid(pid) == pid \in 1..MaxProcesses /\ pid \notin DOMAIN procs

RunnableChildrenOf(parent) ==
    {child \in DOMAIN procs :
        /\ procs[child].parent = parent
        /\ procs[child].status = Runnable}

BlockedChildrenOf(parent) ==
    {child \in DOMAIN procs :
        /\ procs[child].parent = parent
        /\ procs[child].status = Blocked}

TerminalChildrenOf(parent) ==
    {child \in DOMAIN procs :
        /\ procs[child].parent = parent
        /\ procs[child].status \in {Done, Failed}}

JoinWaiter(child) ==
    IF waiters[child] = {} THEN 0 ELSE CHOOSE parent \in waiters[child] : TRUE

AnyWaiter(child) ==
    IF any_waiters[child] = {} THEN 0 ELSE CHOOSE parent \in any_waiters[child] : TRUE

WakeJoinParent(proc, join_state, child) ==
    IF proc.block_reason = BkAwaitRaw
    THEN
        IF join_state = JoinReadyOk
        THEN WithDelivered(ClearBlock(SetStatus(proc, Runnable)), DelValue, child)
        ELSE ClearBlock(SetStatus(proc, Failed))
    ELSE
        IF join_state = JoinReadyOk
        THEN WithDelivered(ClearBlock(SetStatus(proc, Runnable)), DelJoinOk, child)
        ELSE IF join_state = JoinReadyCancelled
             THEN WithDelivered(
                      ClearBlock(SetStatus(proc, Runnable)),
                      DelJoinErrCancelled,
                      child
                  )
             ELSE WithDelivered(
                      ClearBlock(SetStatus(proc, Runnable)),
                      DelJoinErrRuntime,
                      child
                  )

SeqSet(seq) == {seq[i] : i \in 1..Len(seq)}

LeftmostTerminal(children) ==
    CHOOSE child \in SeqSet(children) :
        /\ IsTerminal(child)
        /\ \E i \in 1..Len(children) :
            /\ children[i] = child
            /\ \A j \in 1..(i - 1) : ~IsTerminal(children[j])

WaitAnyInputs == UNION {[1..n -> 1..MaxProcesses] : n \in 1..MaxProcesses}

WakeAnyParent(proc, child) ==
    LET ignored_child == child IN
    WithDelivered(ClearBlock(SetStatus(proc, Runnable)), DelPid, LeftmostTerminal(proc.block_targets))

CanWakeFromJoinState(child) ==
    /\ procs[child].status \in {Done, Failed}
    /\ procs[child].join_state \in JoinReadyStates

JoinStateForFailure(cancelled) ==
    IF cancelled THEN JoinReadyCancelled ELSE JoinReadyErr

IsTerminal(pid) == procs[pid].status \in {Done, Failed}

TerminalParent(pid) ==
    /\ KnownPid(pid)
    /\ procs[pid].status \in {Done, Failed}

WaitAnyChildrenValid(parent, children) ==
    /\ children \in WaitAnyInputs
    /\ \A i \in 1..Len(children) :
        /\ KnownPid(children[i])
        /\ procs[children[i]].parent = parent

\* ── Spawn / yield ───────────────────────────────────────────

Spawn(pid) ==
    /\ Step
    /\ procs[pid].status = Runnable
    /\ AtHead(pid)
    /\ next_pid <= MaxProcesses
    /\ LET child == next_pid IN
        /\ procs' = procs @@ (child :> EmptyProc(pid))
        /\ runqueue' = Tail(runqueue) \o <<child, pid>>
        /\ waiters' = waiters @@ (child :> {})
        /\ any_waiters' = any_waiters @@ (child :> {})
        /\ next_pid' = next_pid + 1
        /\ step' = step + 1
        /\ UNCHANGED <<io_pending, io_next_token, io_count>>

Yield(pid) ==
    /\ Step
    /\ procs[pid].status = Runnable
    /\ ~procs[pid].cancel_requested
    /\ AtHead(pid)
    /\ runqueue' = Tail(runqueue) \o <<pid>>
    /\ step' = step + 1
    /\ UNCHANGED <<procs, waiters, any_waiters, io_pending, next_pid, io_next_token, io_count>>

\* ── Terminal transitions (atomic waiter wakeup) ─────────────

Complete(pid) ==
    /\ Step
    /\ procs[pid].status = Runnable
    /\ ~procs[pid].cancel_requested
    /\ AtHead(pid)
    /\ LET join_parent == JoinWaiter(pid)
           any_parent == AnyWaiter(pid)
           child_join_state ==
               IF procs[pid].parent = 0
               THEN JoinNone
               ELSE IF join_parent = 0 THEN JoinReadyOk ELSE JoinConsumed
           woken_parent ==
               IF join_parent # 0 /\ procs[join_parent].block_reason \in {BkAwaitRaw, BkAwaitResult}
               THEN join_parent
               ELSE IF any_parent # 0 /\ procs[any_parent].block_reason = BkWaitAny
                    THEN any_parent
                    ELSE 0
       IN
        /\ procs' = [p \in DOMAIN procs |->
            IF p = pid
            THEN SetCancelRequested(
                    SetJoinState(SetStatus(ClearBlock(procs[p]), Done), child_join_state),
                    FALSE
                 )
            ELSE IF p = join_parent
            THEN WakeJoinParent(procs[p], JoinReadyOk, pid)
            ELSE IF p = any_parent
            THEN WakeAnyParent(procs[p], pid)
            ELSE procs[p]]
        /\ runqueue' =
            IF woken_parent = 0
            THEN Tail(runqueue)
            ELSE Tail(runqueue) \o <<woken_parent>>
        /\ waiters' = [waiters EXCEPT ![pid] = {}]
        /\ any_waiters' =
            [child \in DOMAIN any_waiters |->
                any_waiters[child] \ (IF any_parent = 0 THEN {} ELSE {any_parent})]
        /\ step' = step + 1
        /\ UNCHANGED <<io_pending, next_pid, io_next_token, io_count>>

Fail(pid) ==
    /\ Step
    /\ procs[pid].status = Runnable
    /\ ~procs[pid].cancel_requested
    /\ AtHead(pid)
    /\ LET join_parent == JoinWaiter(pid)
           any_parent == AnyWaiter(pid)
           child_join_state ==
               IF procs[pid].parent = 0
               THEN JoinNone
               ELSE IF join_parent = 0 THEN JoinReadyErr ELSE JoinConsumed
           woken_parent ==
               IF join_parent # 0 /\ procs[join_parent].block_reason \in {BkAwaitRaw, BkAwaitResult}
               THEN join_parent
               ELSE IF any_parent # 0 /\ procs[any_parent].block_reason = BkWaitAny
                    THEN any_parent
                    ELSE 0
       IN
        /\ procs' = [p \in DOMAIN procs |->
            IF p = pid
            THEN SetCancelRequested(
                    SetJoinState(SetStatus(ClearBlock(procs[p]), Failed), child_join_state),
                    FALSE
                 )
            ELSE IF p = join_parent
            THEN WakeJoinParent(procs[p], JoinReadyErr, pid)
            ELSE IF p = any_parent
            THEN WakeAnyParent(procs[p], pid)
            ELSE procs[p]]
        /\ runqueue' =
            IF join_parent # 0 /\ procs[join_parent].block_reason = BkAwaitRaw
            THEN Tail(runqueue)
            ELSE IF woken_parent = 0
                 THEN Tail(runqueue)
                 ELSE Tail(runqueue) \o <<woken_parent>>
        /\ waiters' = [waiters EXCEPT ![pid] = {}]
        /\ any_waiters' =
            [child \in DOMAIN any_waiters |->
                any_waiters[child] \ (IF any_parent = 0 THEN {} ELSE {any_parent})]
        /\ step' = step + 1
        /\ UNCHANGED <<io_pending, next_pid, io_next_token, io_count>>

ObserveCancel(pid) ==
    /\ Step
    /\ procs[pid].status = Runnable
    /\ procs[pid].cancel_requested
    /\ AtHead(pid)
    /\ LET join_parent == JoinWaiter(pid)
           any_parent == AnyWaiter(pid)
           child_join_state ==
               IF procs[pid].parent = 0
               THEN JoinNone
               ELSE IF join_parent = 0 THEN JoinReadyCancelled ELSE JoinConsumed
           woken_parent ==
               IF join_parent # 0 /\ procs[join_parent].block_reason \in {BkAwaitRaw, BkAwaitResult}
               THEN join_parent
               ELSE IF any_parent # 0 /\ procs[any_parent].block_reason = BkWaitAny
                    THEN any_parent
                    ELSE 0
       IN
        /\ procs' = [p \in DOMAIN procs |->
            IF p = pid
            THEN SetCancelRequested(
                    SetJoinState(SetStatus(ClearBlock(procs[p]), Failed), child_join_state),
                    FALSE
                 )
            ELSE IF p = join_parent
            THEN WakeJoinParent(procs[p], JoinReadyCancelled, pid)
            ELSE IF p = any_parent
            THEN WakeAnyParent(procs[p], pid)
            ELSE procs[p]]
        /\ runqueue' =
            IF join_parent # 0 /\ procs[join_parent].block_reason = BkAwaitRaw
            THEN Tail(runqueue)
            ELSE IF woken_parent = 0
                 THEN Tail(runqueue)
                 ELSE Tail(runqueue) \o <<woken_parent>>
        /\ waiters' = [waiters EXCEPT ![pid] = {}]
        /\ any_waiters' =
            [child \in DOMAIN any_waiters |->
                any_waiters[child] \ (IF any_parent = 0 THEN {} ELSE {any_parent})]
        /\ step' = step + 1
        /\ UNCHANGED <<io_pending, next_pid, io_next_token, io_count>>

\* ── Await / await_result ────────────────────────────────────

AwaitReadyRaw(parent, child) ==
    /\ Step
    /\ procs[parent].status = Runnable
    /\ AtHead(parent)
    /\ KnownPid(child)
    /\ procs[child].parent = parent
    /\ procs[child].join_state = JoinReadyOk
    /\ procs' = [p \in DOMAIN procs |->
        IF p = parent
        THEN WithDelivered(procs[p], DelValue, child)
        ELSE IF p = child
             THEN SetJoinState(procs[p], JoinConsumed)
             ELSE procs[p]]
    /\ runqueue' = Tail(runqueue) \o <<parent>>
    /\ step' = step + 1
    /\ UNCHANGED <<waiters, any_waiters, io_pending, next_pid, io_next_token, io_count>>

AwaitReadyResult(parent, child) ==
    /\ Step
    /\ procs[parent].status = Runnable
    /\ AtHead(parent)
    /\ KnownPid(child)
    /\ procs[child].parent = parent
    /\ procs[child].join_state \in {JoinReadyOk, JoinReadyErr, JoinReadyCancelled}
    /\ LET delivered ==
           IF procs[child].join_state = JoinReadyOk
           THEN DelJoinOk
           ELSE IF procs[child].join_state = JoinReadyCancelled
                THEN DelJoinErrCancelled
                ELSE DelJoinErrRuntime
       IN
        /\ procs' = [p \in DOMAIN procs |->
            IF p = parent
            THEN WithDelivered(procs[p], delivered, child)
            ELSE IF p = child
                 THEN SetJoinState(procs[p], JoinConsumed)
                 ELSE procs[p]]
        /\ runqueue' = Tail(runqueue) \o <<parent>>
        /\ step' = step + 1
        /\ UNCHANGED <<waiters, any_waiters, io_pending, next_pid, io_next_token, io_count>>

AwaitConsumedRaw(parent, child) ==
    /\ Step
    /\ procs[parent].status = Runnable
    /\ AtHead(parent)
    /\ KnownPid(child)
    /\ procs[child].parent = parent
    /\ procs[child].join_state = JoinConsumed
    /\ procs' = [procs EXCEPT ![parent] = SetStatus(procs[parent], Failed)]
    /\ runqueue' = Tail(runqueue)
    /\ step' = step + 1
    /\ UNCHANGED <<waiters, any_waiters, io_pending, next_pid, io_next_token, io_count>>

AwaitConsumedResult(parent, child) ==
    /\ Step
    /\ procs[parent].status = Runnable
    /\ AtHead(parent)
    /\ KnownPid(child)
    /\ procs[child].parent = parent
    /\ procs[child].join_state = JoinConsumed
    /\ procs' = [procs EXCEPT ![parent] = WithDelivered(procs[parent], DelJoinErrAlreadyJoined, child)]
    /\ runqueue' = Tail(runqueue) \o <<parent>>
    /\ step' = step + 1
    /\ UNCHANGED <<waiters, any_waiters, io_pending, next_pid, io_next_token, io_count>>

AwaitBlock(parent, child, reason) ==
    /\ Step
    /\ reason \in {BkAwaitRaw, BkAwaitResult}
    /\ procs[parent].status = Runnable
    /\ AtHead(parent)
    /\ KnownPid(child)
    /\ procs[child].parent = parent
    /\ procs[child].status \in {Runnable, Blocked}
    /\ procs' = [procs EXCEPT ![parent] = BlockOnAwait(procs[parent], reason, child)]
    /\ runqueue' = Tail(runqueue)
    /\ waiters' = [waiters EXCEPT ![child] = @ \union {parent}]
    /\ step' = step + 1
    /\ UNCHANGED <<any_waiters, io_pending, next_pid, io_next_token, io_count>>

AwaitUnknown(parent, child_pid) ==
    /\ Step
    /\ procs[parent].status = Runnable
    /\ AtHead(parent)
    /\ UnknownPid(child_pid)
    /\ procs' = [procs EXCEPT ![parent] = SetStatus(procs[parent], Failed)]
    /\ runqueue' = Tail(runqueue)
    /\ step' = step + 1
    /\ UNCHANGED <<waiters, any_waiters, io_pending, next_pid, io_next_token, io_count>>

AwaitNotChild(parent, child) ==
    /\ Step
    /\ procs[parent].status = Runnable
    /\ AtHead(parent)
    /\ KnownPid(child)
    /\ procs[child].parent # parent
    /\ procs' = [procs EXCEPT ![parent] = SetStatus(procs[parent], Failed)]
    /\ runqueue' = Tail(runqueue)
    /\ step' = step + 1
    /\ UNCHANGED <<waiters, any_waiters, io_pending, next_pid, io_next_token, io_count>>

\* ── wait_any ────────────────────────────────────────────────

WaitAnyReady(parent, children) ==
    /\ Step
    /\ procs[parent].status = Runnable
    /\ AtHead(parent)
    /\ WaitAnyChildrenValid(parent, children)
    /\ \E i \in 1..Len(children) : procs[children[i]].status \in {Done, Failed}
    /\ procs' = [procs EXCEPT ![parent] = WithDelivered(procs[parent], DelPid, LeftmostTerminal(children))]
    /\ runqueue' = Tail(runqueue) \o <<parent>>
    /\ step' = step + 1
    /\ UNCHANGED <<waiters, any_waiters, io_pending, next_pid, io_next_token, io_count>>

WaitAnyBlock(parent, children) ==
    /\ Step
    /\ procs[parent].status = Runnable
    /\ AtHead(parent)
    /\ WaitAnyChildrenValid(parent, children)
    /\ \A i \in 1..Len(children) : procs[children[i]].status \notin {Done, Failed}
    /\ procs' = [procs EXCEPT ![parent] = BlockOnWaitAny(procs[parent], children)]
    /\ runqueue' = Tail(runqueue)
    /\ any_waiters' =
        [child \in DOMAIN any_waiters |->
            IF child \in SeqSet(children)
            THEN any_waiters[child] \union {parent}
            ELSE any_waiters[child]]
    /\ step' = step + 1
    /\ UNCHANGED <<waiters, io_pending, next_pid, io_next_token, io_count>>

WaitAnyEmpty(parent) ==
    /\ Step
    /\ procs[parent].status = Runnable
    /\ AtHead(parent)
    /\ procs' = [procs EXCEPT ![parent] = SetStatus(procs[parent], Failed)]
    /\ runqueue' = Tail(runqueue)
    /\ step' = step + 1
    /\ UNCHANGED <<waiters, any_waiters, io_pending, next_pid, io_next_token, io_count>>

WaitAnyNotChild(parent, children) ==
    /\ Step
    /\ procs[parent].status = Runnable
    /\ AtHead(parent)
    /\ children \in WaitAnyInputs
    /\ \E i \in 1..Len(children) :
        /\ KnownPid(children[i])
        /\ procs[children[i]].parent # parent
    /\ procs' = [procs EXCEPT ![parent] = SetStatus(procs[parent], Failed)]
    /\ runqueue' = Tail(runqueue)
    /\ step' = step + 1
    /\ UNCHANGED <<waiters, any_waiters, io_pending, next_pid, io_next_token, io_count>>

WaitAnyUnknown(parent, children) ==
    /\ Step
    /\ procs[parent].status = Runnable
    /\ AtHead(parent)
    /\ children \in WaitAnyInputs
    /\ \E i \in 1..Len(children) : UnknownPid(children[i])
    /\ procs' = [procs EXCEPT ![parent] = SetStatus(procs[parent], Failed)]
    /\ runqueue' = Tail(runqueue)
    /\ step' = step + 1
    /\ UNCHANGED <<waiters, any_waiters, io_pending, next_pid, io_next_token, io_count>>

\* ── cancel ──────────────────────────────────────────────────

CancelTerminal(parent, child) ==
    /\ Step
    /\ procs[parent].status = Runnable
    /\ AtHead(parent)
    /\ KnownPid(child)
    /\ procs[child].parent = parent
    /\ procs[child].status \in {Done, Failed}
    /\ procs' = [procs EXCEPT ![parent] = WithDelivered(procs[parent], DelUnit, 0)]
    /\ runqueue' = Tail(runqueue) \o <<parent>>
    /\ step' = step + 1
    /\ UNCHANGED <<waiters, any_waiters, io_pending, next_pid, io_next_token, io_count>>

CancelRunnable(parent, child) ==
    /\ Step
    /\ procs[parent].status = Runnable
    /\ AtHead(parent)
    /\ KnownPid(child)
    /\ procs[child].parent = parent
    /\ procs[child].status = Runnable
    /\ procs' = [p \in DOMAIN procs |->
        IF p = parent
        THEN WithDelivered(procs[p], DelUnit, 0)
        ELSE IF p = child
             THEN SetCancelRequested(procs[p], TRUE)
             ELSE procs[p]]
    /\ runqueue' = Tail(runqueue) \o <<parent>>
    /\ step' = step + 1
    /\ UNCHANGED <<waiters, any_waiters, io_pending, next_pid, io_next_token, io_count>>

CancelBlocked(parent, child) ==
    /\ Step
    /\ procs[parent].status = Runnable
    /\ AtHead(parent)
    /\ KnownPid(child)
    /\ procs[child].parent = parent
    /\ procs[child].status = Blocked
    /\ LET join_parent == JoinWaiter(child)
           any_parent == AnyWaiter(child)
           child_join_state == IF join_parent = 0 THEN JoinReadyCancelled ELSE JoinConsumed
           cleared_waiters ==
               IF procs[child].block_reason = BkAwaitRaw \/ procs[child].block_reason = BkAwaitResult
               THEN [waiters EXCEPT ![procs[child].block_target] = @ \ {child}]
               ELSE waiters
           cleared_any_waiters ==
               IF procs[child].block_reason = BkWaitAny
               THEN [pid \in DOMAIN any_waiters |->
                       IF pid \in SeqSet(procs[child].block_targets)
                       THEN any_waiters[pid] \ {child}
                       ELSE any_waiters[pid]]
               ELSE any_waiters
           cleared_io_pending ==
               IF procs[child].block_reason = BkIo
               THEN io_pending \ {[pid |-> child, token |-> procs[child].block_target]}
               ELSE io_pending
       IN
        /\ procs' = [p \in DOMAIN procs |->
            IF p = parent
            THEN WithDelivered(procs[p], DelUnit, 0)
            ELSE IF p = child
                 THEN SetCancelRequested(
                         SetJoinState(SetStatus(ClearBlock(procs[p]), Failed), child_join_state),
                         FALSE
                      )
            ELSE IF p = join_parent
                 THEN WakeJoinParent(procs[p], JoinReadyCancelled, child)
            ELSE IF p = any_parent
                 THEN WakeAnyParent(procs[p], child)
                 ELSE procs[p]]
        /\ runqueue' =
            IF join_parent # 0 /\ procs[join_parent].block_reason = BkAwaitRaw
            THEN Tail(runqueue) \o <<parent>>
            ELSE IF any_parent = 0 /\ join_parent = 0
                 THEN Tail(runqueue) \o <<parent>>
                 ELSE Tail(runqueue) \o <<parent, IF join_parent # 0 THEN join_parent ELSE any_parent>>
        /\ waiters' = [cleared_waiters EXCEPT ![child] = {}]
        /\ any_waiters' =
            [pid \in DOMAIN cleared_any_waiters |->
                cleared_any_waiters[pid] \ (IF any_parent = 0 THEN {} ELSE {any_parent})]
        /\ io_pending' = cleared_io_pending
        /\ step' = step + 1
        /\ UNCHANGED <<next_pid, io_next_token, io_count>>

CancelUnknown(parent, child_pid) ==
    /\ Step
    /\ procs[parent].status = Runnable
    /\ AtHead(parent)
    /\ UnknownPid(child_pid)
    /\ procs' = [procs EXCEPT ![parent] = SetStatus(procs[parent], Failed)]
    /\ runqueue' = Tail(runqueue)
    /\ step' = step + 1
    /\ UNCHANGED <<waiters, any_waiters, io_pending, next_pid, io_next_token, io_count>>

CancelNotChild(parent, child) ==
    /\ Step
    /\ procs[parent].status = Runnable
    /\ AtHead(parent)
    /\ KnownPid(child)
    /\ procs[child].parent # parent
    /\ procs' = [procs EXCEPT ![parent] = SetStatus(procs[parent], Failed)]
    /\ runqueue' = Tail(runqueue)
    /\ step' = step + 1
    /\ UNCHANGED <<waiters, any_waiters, io_pending, next_pid, io_next_token, io_count>>

\* ── I/O ─────────────────────────────────────────────────────

RequestIo(pid) ==
    /\ Step
    /\ procs[pid].status = Runnable
    /\ ~procs[pid].cancel_requested
    /\ AtHead(pid)
    /\ io_count < MaxIoOps
    /\ LET token == io_next_token IN
        /\ procs' = [procs EXCEPT ![pid] = BlockOnIo(procs[pid], token)]
        /\ runqueue' = Tail(runqueue)
        /\ io_pending' = io_pending \union {[pid |-> pid, token |-> token]}
        /\ io_next_token' = io_next_token + 1
        /\ io_count' = io_count + 1
        /\ step' = step + 1
        /\ UNCHANGED <<waiters, any_waiters, next_pid>>

IoComplete(entry) ==
    /\ Step
    /\ entry \in io_pending
    /\ procs[entry.pid].status = Blocked
    /\ procs[entry.pid].block_reason = BkIo
    /\ procs[entry.pid].block_target = entry.token
    /\ procs' = [procs EXCEPT
        ![entry.pid] = WithDelivered(ClearBlock(SetStatus(procs[entry.pid], Runnable)), DelValue, entry.pid)]
    /\ runqueue' = runqueue \o <<entry.pid>>
    /\ io_pending' = io_pending \ {entry}
    /\ step' = step + 1
    /\ UNCHANGED <<waiters, any_waiters, next_pid, io_next_token, io_count>>

IoFail(entry) ==
    /\ Step
    /\ entry \in io_pending
    /\ procs[entry.pid].status = Blocked
    /\ procs[entry.pid].block_reason = BkIo
    /\ procs[entry.pid].block_target = entry.token
    /\ LET join_parent == JoinWaiter(entry.pid)
           any_parent == AnyWaiter(entry.pid)
           child_join_state ==
               IF procs[entry.pid].parent = 0
               THEN JoinNone
               ELSE IF join_parent = 0 THEN JoinReadyErr ELSE JoinConsumed
           woken_parent ==
               IF join_parent # 0 /\ procs[join_parent].block_reason \in {BkAwaitRaw, BkAwaitResult}
               THEN join_parent
               ELSE IF any_parent # 0 /\ procs[any_parent].block_reason = BkWaitAny
                    THEN any_parent
                    ELSE 0
       IN
        /\ procs' = [p \in DOMAIN procs |->
            IF p = entry.pid
            THEN SetCancelRequested(
                    SetJoinState(SetStatus(ClearBlock(procs[p]), Failed), child_join_state),
                    FALSE
                 )
            ELSE IF p = join_parent
            THEN WakeJoinParent(procs[p], JoinReadyErr, entry.pid)
            ELSE IF p = any_parent
            THEN WakeAnyParent(procs[p], entry.pid)
            ELSE procs[p]]
        /\ runqueue' =
            IF join_parent # 0 /\ procs[join_parent].block_reason = BkAwaitRaw
            THEN runqueue
            ELSE IF woken_parent = 0 THEN runqueue ELSE runqueue \o <<woken_parent>>
        /\ waiters' = [waiters EXCEPT ![entry.pid] = {}]
        /\ any_waiters' =
            [child \in DOMAIN any_waiters |->
                any_waiters[child] \ (IF any_parent = 0 THEN {} ELSE {any_parent})]
        /\ io_pending' = io_pending \ {entry}
        /\ step' = step + 1
        /\ UNCHANGED <<next_pid, io_next_token, io_count>>

\* ── Scope cleanup ───────────────────────────────────────────

ScopeCancelRunnable(parent, child) ==
    /\ Step
    /\ TerminalParent(parent)
    /\ KnownPid(child)
    /\ procs[child].parent = parent
    /\ procs[child].status = Runnable
    /\ ~procs[child].cancel_requested
    /\ procs' = [procs EXCEPT ![child] = SetCancelRequested(procs[child], TRUE)]
    /\ step' = step + 1
    /\ UNCHANGED <<runqueue, waiters, any_waiters, io_pending, next_pid, io_next_token, io_count>>

ScopeCancelBlocked(parent, child) ==
    /\ Step
    /\ TerminalParent(parent)
    /\ KnownPid(child)
    /\ procs[child].parent = parent
    /\ procs[child].status = Blocked
    /\ LET join_parent == JoinWaiter(child)
           any_parent == AnyWaiter(child)
           child_join_state == IF join_parent = 0 THEN JoinReadyCancelled ELSE JoinConsumed
           cleared_waiters ==
               IF procs[child].block_reason = BkAwaitRaw \/ procs[child].block_reason = BkAwaitResult
               THEN [waiters EXCEPT ![procs[child].block_target] = @ \ {child}]
               ELSE waiters
           cleared_any_waiters ==
               IF procs[child].block_reason = BkWaitAny
               THEN [pid \in DOMAIN any_waiters |->
                       IF pid \in SeqSet(procs[child].block_targets)
                       THEN any_waiters[pid] \ {child}
                       ELSE any_waiters[pid]]
               ELSE any_waiters
           cleared_io_pending ==
               IF procs[child].block_reason = BkIo
               THEN io_pending \ {[pid |-> child, token |-> procs[child].block_target]}
               ELSE io_pending
           woken_parent ==
               IF join_parent # 0 /\ procs[join_parent].block_reason \in {BkAwaitRaw, BkAwaitResult}
               THEN join_parent
               ELSE IF any_parent # 0 /\ procs[any_parent].block_reason = BkWaitAny
                    THEN any_parent
                    ELSE 0
       IN
        /\ procs' = [p \in DOMAIN procs |->
            IF p = child
            THEN SetCancelRequested(
                    SetJoinState(SetStatus(ClearBlock(procs[p]), Failed), child_join_state),
                    FALSE
                 )
            ELSE IF p = join_parent
            THEN WakeJoinParent(procs[p], JoinReadyCancelled, child)
            ELSE IF p = any_parent
            THEN WakeAnyParent(procs[p], child)
            ELSE procs[p]]
        /\ runqueue' = IF woken_parent = 0 THEN runqueue ELSE runqueue \o <<woken_parent>>
        /\ waiters' = [cleared_waiters EXCEPT ![child] = {}]
        /\ any_waiters' =
            [pid \in DOMAIN cleared_any_waiters |->
                cleared_any_waiters[pid] \ (IF any_parent = 0 THEN {} ELSE {any_parent})]
        /\ io_pending' = cleared_io_pending
        /\ step' = step + 1
        /\ UNCHANGED <<next_pid, io_next_token, io_count>>

\* ── Deadlock detection ──────────────────────────────────────

DetectDeadlock(pid) ==
    /\ Step
    /\ procs[pid].status = Blocked
    /\ runqueue = <<>>
    /\ io_pending = {}
    /\ \/ /\ procs[pid].block_reason \in {BkAwaitRaw, BkAwaitResult}
          /\ procs[procs[pid].block_target].status = Blocked
       \/ /\ procs[pid].block_reason = BkWaitAny
          /\ \A child \in SeqSet(procs[pid].block_targets) : procs[child].status = Blocked
    /\ procs' = [procs EXCEPT ![pid] = ClearBlock(SetStatus(procs[pid], Failed))]
    /\ waiters' =
        IF procs[pid].block_reason \in {BkAwaitRaw, BkAwaitResult}
        THEN [waiters EXCEPT ![procs[pid].block_target] = @ \ {pid}]
        ELSE waiters
    /\ any_waiters' =
        IF procs[pid].block_reason = BkWaitAny
        THEN [child \in DOMAIN any_waiters |->
                IF child \in SeqSet(procs[pid].block_targets)
                THEN any_waiters[child] \ {pid}
                ELSE any_waiters[child]]
        ELSE any_waiters
    /\ runqueue' = runqueue
    /\ step' = step + 1
    /\ UNCHANGED <<io_pending, next_pid, io_next_token, io_count>>

\* ── Next ────────────────────────────────────────────────────

Next ==
    \/ \E pid \in DOMAIN procs :
        \/ Spawn(pid)
        \/ Complete(pid)
        \/ Fail(pid)
        \/ ObserveCancel(pid)
        \/ Yield(pid)
        \/ RequestIo(pid)
        \/ \E child \in DOMAIN procs :
            \/ AwaitReadyRaw(pid, child)
            \/ AwaitReadyResult(pid, child)
            \/ AwaitConsumedRaw(pid, child)
            \/ AwaitConsumedResult(pid, child)
            \/ AwaitBlock(pid, child, BkAwaitRaw)
            \/ AwaitBlock(pid, child, BkAwaitResult)
            \/ CancelTerminal(pid, child)
            \/ CancelRunnable(pid, child)
            \/ CancelBlocked(pid, child)
            \/ ScopeCancelRunnable(pid, child)
            \/ ScopeCancelBlocked(pid, child)
        \/ \E child_pid \in 1..MaxProcesses :
            \/ AwaitUnknown(pid, child_pid)
            \/ CancelUnknown(pid, child_pid)
        \/ \E child \in DOMAIN procs :
            \/ AwaitNotChild(pid, child)
            \/ CancelNotChild(pid, child)
        \/ WaitAnyEmpty(pid)
        \/ \E children \in WaitAnyInputs :
            \/ WaitAnyReady(pid, children)
            \/ WaitAnyBlock(pid, children)
            \/ WaitAnyNotChild(pid, children)
            \/ WaitAnyUnknown(pid, children)
        \/ DetectDeadlock(pid)
    \/ \E entry \in io_pending :
        \/ IoComplete(entry)
        \/ IoFail(entry)

\* ── Invariants ──────────────────────────────────────────────

TypeOK ==
    /\ next_pid \in Nat
    /\ io_next_token \in Nat
    /\ io_count \in Nat
    /\ step \in Nat
    /\ \A p \in DOMAIN procs :
        /\ procs[p].status \in {Runnable, Blocked, Done, Failed}
        /\ procs[p].parent \in (1..MaxProcesses) \union {0}
        /\ procs[p].join_state \in {JoinNone, JoinReadyOk, JoinReadyErr, JoinReadyCancelled, JoinConsumed}
        /\ procs[p].block_reason \in {BkNone, BkAwaitRaw, BkAwaitResult, BkWaitAny, BkIo}
        /\ procs[p].block_target \in Nat \union {0}
        /\ IF procs[p].block_reason = BkWaitAny
           THEN procs[p].block_targets \in WaitAnyInputs
           ELSE procs[p].block_targets = {}
        /\ procs[p].cancel_requested \in BOOLEAN
        /\ procs[p].delivered_kind \in {
            DelNone, DelValue, DelPid, DelJoinOk,
            DelJoinErrRuntime, DelJoinErrCancelled, DelJoinErrAlreadyJoined, DelUnit
        }
        /\ procs[p].delivered_pid \in Nat \union {0}

AllKnownPidsBelowNext ==
    \A p \in DOMAIN procs : p < next_pid

RunnableInQueue ==
    \A p \in DOMAIN procs :
        procs[p].status = Runnable => InRunqueue(p)

BlockedOrTerminalNotInQueue ==
    \A p \in DOMAIN procs :
        procs[p].status \in {Blocked, Done, Failed} => ~InRunqueue(p)

NoDuplicateRunqueuePids ==
    \A i, j \in 1..Len(runqueue) :
        i # j => runqueue[i] # runqueue[j]

JoinStateMatchesStatus ==
    \A p \in DOMAIN procs :
        /\ procs[p].status \in {Runnable, Blocked} => procs[p].join_state = JoinNone
        /\ procs[p].parent = 0 => procs[p].join_state \in {JoinNone}

TerminalStatesAbsorbing ==
    \A p \in DOMAIN procs :
        procs[p].status \in {Done, Failed} =>
            /\ procs[p].block_reason = BkNone
            /\ procs[p].block_target = 0
            /\ procs[p].block_targets = {}

OnlyParentAwaits ==
    \A p \in DOMAIN procs :
        (procs[p].status = Blocked /\ procs[p].block_reason \in {BkAwaitRaw, BkAwaitResult}) =>
            LET child == procs[p].block_target IN
                /\ child \in DOMAIN procs
                /\ procs[child].parent = p

OnlyParentWaitsAny ==
    \A p \in DOMAIN procs :
        (procs[p].status = Blocked /\ procs[p].block_reason = BkWaitAny) =>
            /\ procs[p].block_targets \in WaitAnyInputs
            /\ \A child \in SeqSet(procs[p].block_targets) :
                /\ child \in DOMAIN procs
                /\ procs[child].parent = p

WaitersConsistent ==
    \A child \in DOMAIN waiters :
        \A parent \in waiters[child] :
            /\ parent \in DOMAIN procs
            /\ procs[parent].status = Blocked
            /\ procs[parent].block_reason \in {BkAwaitRaw, BkAwaitResult}
            /\ procs[parent].block_target = child

BlockedAwaitListed ==
    \A parent \in DOMAIN procs :
        (procs[parent].status = Blocked /\ procs[parent].block_reason \in {BkAwaitRaw, BkAwaitResult}) =>
            parent \in waiters[procs[parent].block_target]

AtMostOneWaiterPerChild ==
    \A child \in DOMAIN waiters :
        Cardinality(waiters[child]) <= 1

AnyWaitersConsistent ==
    \A child \in DOMAIN any_waiters :
        \A parent \in any_waiters[child] :
            /\ parent \in DOMAIN procs
            /\ procs[parent].status = Blocked
            /\ procs[parent].block_reason = BkWaitAny
            /\ child \in SeqSet(procs[parent].block_targets)
            /\ procs[child].parent = parent

AtMostOneAnyWaiterPerChild ==
    \A child \in DOMAIN any_waiters :
        Cardinality(any_waiters[child]) <= 1

IoBlockedHasPending ==
    \A p \in DOMAIN procs :
        (procs[p].status = Blocked /\ procs[p].block_reason = BkIo) =>
            \E entry \in io_pending :
                entry.pid = p /\ entry.token = procs[p].block_target

NoOrphanedIo ==
    \A entry \in io_pending :
        /\ entry.pid \in DOMAIN procs
        /\ procs[entry.pid].status = Blocked
        /\ procs[entry.pid].block_reason = BkIo

NoBlockedParentOnJoinReady ==
    \A p \in DOMAIN procs :
        (procs[p].status = Blocked /\ procs[p].block_reason \in {BkAwaitRaw, BkAwaitResult}) =>
            LET child == procs[p].block_target IN
                procs[child].join_state \notin JoinReadyStates

SafetyInvariant ==
    /\ TypeOK
    /\ AllKnownPidsBelowNext
    /\ RunnableInQueue
    /\ BlockedOrTerminalNotInQueue
    /\ NoDuplicateRunqueuePids
    /\ JoinStateMatchesStatus
    /\ TerminalStatesAbsorbing
    /\ OnlyParentAwaits
    /\ OnlyParentWaitsAny
    /\ WaitersConsistent
    /\ BlockedAwaitListed
    /\ AtMostOneWaiterPerChild
    /\ AnyWaitersConsistent
    /\ AtMostOneAnyWaiterPerChild
    /\ IoBlockedHasPending
    /\ NoOrphanedIo
    /\ NoBlockedParentOnJoinReady

\* ── Liveness ────────────────────────────────────────────────

ResolveIo(entry) == IoComplete(entry) \/ IoFail(entry)

ObserveCancelEnabled(pid) ==
    IF pid \in DOMAIN procs THEN ObserveCancel(pid) ELSE FALSE

ResolveIoEnabled(entry) ==
    IF entry \in io_pending THEN ResolveIo(entry) ELSE FALSE

Fairness ==
    /\ \A pid \in 1..MaxProcesses : WF_vars(ObserveCancelEnabled(pid))
    /\ \A entry \in [pid : 1..MaxProcesses, token : 1..MaxIoOps] :
        WF_vars(ResolveIoEnabled(entry))

Spec == Init /\ [][Next]_vars /\ Fairness

CancelRequestedEventuallySettles ==
    \A pid \in 1..MaxProcesses :
        [](pid \in DOMAIN procs /\ procs[pid].cancel_requested
           ~> procs[pid].status \in {Done, Failed})

IoEventuallyCompletes ==
    \A entry \in [pid : 1..MaxProcesses, token : 1..MaxIoOps] :
        entry \in io_pending ~> entry \notin io_pending

====
