---- MODULE ThreadedSchedulerImpl ----
\* # Documentation
\*
\* ## Why this spec exists
\*
\* ProcessLifecycle.tla defines the user-visible process semantics. The threaded
\* runtime (crates/hiko-vm/src/threaded.rs) implements them with workers that
\* take processes out of a shared table, run them, and race with each other and
\* with the I/O monitor when they register waiters, park processes, publish
\* tombstones, cancel children and wake waiters. This spec models that protocol
\* one table operation at a time, so the checker explores the same races the
\* code has.
\*
\* ## What we model
\*
\* Each step below is one operation on the shared table (one DashMap call in
\* the code), or a change to a process its actor owns. A step that would only
\* move an actor to its next step is part of the step before it: it changes
\* nothing another actor can see, so it adds interleavings but no behaviours.
\* The names in comments are the Rust functions they follow.
\*
\*   - Ownership: a process is in the table, owned by one actor (a worker or
\*     the I/O monitor), or gone (a terminated child, dropped).
\*   - Slice results: yield, spawn, done, fail, cancelled, await (raw and
\*     result), cancel, wait_any, I/O.
\*   - The wake protocol: register, park, recheck (block_on_child,
\*     handle_wait_any), and take_waiter's in-flight and stale cases.
\*   - finish_process: tombstone, parent-gone reaping, `live`, pending
\*     cancels, scope exit (cancel live children, reap tombstones), then
\*     wake wait_any and await waiters.
\*   - cancel_pid / try_cancel_in_table, `pending_cancels` observed by park
\*     and by the worker that dequeues the process, the VM cancel flag.
\*   - I/O: register, park, backend registration, monitor completion,
\*     cancellation of a blocked request, late completions.
\*
\* ## What we intentionally abstract
\*
\* Bytecode, heaps and values are abstracted: a slice result is chosen
\* nondeterministically and outcomes are "ok", "err" or "cancelled". The
\* scheduler queue is a set: duplicate entries name the same process, and a
\* dequeue takes it only if it is runnable in the table. The leftmost-winner
\* rule of wait_any is checked in WaitAnyLeftmost.tla; here any terminated
\* listed child may be delivered. A process that ends up owned by an actor is
\* handled by that actor, in any order (the code uses a stack). Taking a
\* blocked process and removing its registration are one step. Spawn, awaiting
\* a pid that is not a child, and other errors that fail the caller at once
\* are left out; they do not touch shared state.
\*
\* ## What we check
\*
\* - NoLostWakeup: once nothing can run (no actor busy, nothing queued, no I/O
\*   in flight), every process has terminated.
\* - CleanAtQuiescence: then no waiter, tombstone, I/O waiter or cancel
\*   request is left behind.
\* - NoResurrection: a terminated process is never in the table as runnable
\*   or blocked, and never owned.
\* - BoundedTombstones: a tombstone exists only for a child of a live parent,
\*   or in the short windows where a terminating parent or child is about to
\*   reap it.
\* - PendingCancelsConsistent, RegistrationsConsistent.
\*
\* NoLostWakeup is the safety form of "a waiter is eventually woken": the model
\* is finite, so a waiter that is never woken ends in a state where nothing
\* can run. The liveness form needs weak fairness per actor (the I/O monitor
\* must not be starved while a process yields forever), which tla-checker
\* cannot evaluate.
\*
\* The broken configurations under specs/tla/broken/ turn off the recheck
\* after parking (RecheckAfterPark) or the scope-exit reaping of tombstones
\* (ReapTombstones); the checker must reject both.

EXTENDS Naturals, Sequences, FiniteSets, TLC

CONSTANTS
    MaxProcesses,
    MaxIo,
    Workers,
    RecheckAfterPark,
    ReapTombstones

Pids == 1..MaxProcesses
Toks == 1..MaxIo
Mon == "mon"
Actors == Workers \union {Mon}
BlockedStates == {"await", "any", "io"}

VARIABLES
    loc,        \* pid -> "none" (not spawned), "table", "gone", or the owning actor
    st,         \* pid -> "runnable", "await", "any", "io", "done", "failed"
    target,     \* pid -> child awaited
    akind,      \* pid -> "raw" or "result"
    targets,    \* pid -> children in wait_any
    tok,        \* pid -> I/O token blocked on
    par,        \* pid -> parent (0 for the root)
    kids,       \* pid -> children not yet consumed (Process::children)
    live,
    tomb,       \* pid -> "none", "ok", "err", "cancelled"
    waiters,    \* child -> processes registered in await
    anyw,       \* child -> processes registered in wait_any
    iow,        \* token -> pid (0: none)
    backend,    \* tokens with a completion still to come
    pcancel,    \* pending_cancels
    creq,       \* VM cancel flag set (request_cancellation)
    queued,
    next_pid,
    next_tok,
    wk          \* actor -> its program state

vars == <<loc, st, target, akind, targets, tok, par, kids, live, tomb, waiters,
          anyw, iow, backend, pcancel, creq, queued, next_pid, next_tok, wk>>

\* An actor's state. pc is the next step; stack holds return labels. p is the
\* process whose slice it handles, c the awaited child, s the wait_any list, t
\* the I/O token, x the target of a cancel, pk the process to park, q the
\* process being terminated, ktodo and k its scope-exit loop, fin the process
\* whose waiters are woken, wtodo and cur that loop, term the processes this
\* actor has to terminate (with their outcomes). A field goes back to its
\* Fresh value once no later step reads it, so that states that differ only
\* in leftovers are one state.
Fresh ==
    [pc |-> "idle", stack |-> <<>>, p |-> 0, c |-> 0, s |-> {}, t |-> 0,
     x |-> 0, pk |-> 0, q |-> 0, ktodo |-> {}, k |-> 0,
     fin |-> 0, wtodo |-> {}, cur |-> 0, term |-> {}]

Init ==
    /\ loc = [p \in Pids |-> IF p = 1 THEN "table" ELSE "none"]
    /\ st = [p \in Pids |-> "runnable"]
    /\ target = [p \in Pids |-> 0]
    /\ akind = [p \in Pids |-> "raw"]
    /\ targets = [p \in Pids |-> {}]
    /\ tok = [p \in Pids |-> 0]
    /\ par = [p \in Pids |-> 0]
    /\ kids = [p \in Pids |-> {}]
    /\ live = {1}
    /\ tomb = [p \in Pids |-> "none"]
    /\ waiters = [p \in Pids |-> {}]
    /\ anyw = [p \in Pids |-> {}]
    /\ iow = [t \in Toks |-> 0]
    /\ backend = {}
    /\ pcancel = {}
    /\ creq = {}
    /\ queued = {1}
    /\ next_pid = 2
    /\ next_tok = 1
    /\ wk = [a \in Actors |-> Fresh]

\* ---------------------------------------------------------------------------
\* Helpers

Return(r) ==
    IF r.stack = <<>>
    THEN Fresh
    ELSE [r EXCEPT !.pc = Head(r.stack), !.stack = Tail(r.stack)]

Call(r, proc, ret) == [r EXCEPT !.pc = proc, !.stack = <<ret>> \o r.stack]

\* cancel_pid(x): try_cancel_in_table(x), then "cp2" if x was not in the table.
CallCancel(r, x, ret) ==
    [r EXCEPT !.pc = "tc", !.x = x, !.stack = <<"cp2", ret>> \o r.stack]

\* Return from try_cancel_in_table. cancel_pid stops if x was in the table.
TcReturn(r, inTable) ==
    IF r.stack # <<>> /\ Head(r.stack) = "cp2"
    THEN IF inTable THEN Return(Return([r EXCEPT !.x = 0])) ELSE Return(r)
    ELSE Return([r EXCEPT !.x = 0])

\* fin is still needed when wake_join_waiters follows.
EndWakeAny(r) ==
    LET n == Return([r EXCEPT !.cur = 0]) IN
    IF n.pc = "wj0" THEN n ELSE [n EXCEPT !.fin = 0]

EndWakeJoin(r) == Return([r EXCEPT !.fin = 0, !.cur = 0])

RemoveEverywhere(m, pid) == [y \in Pids |-> m[y] \ {pid}]

\* resume(): back in the table as runnable, and queued.
ResumeLoc(p) == [loc EXCEPT ![p] = "table"]
ResumeSt(p) == [st EXCEPT ![p] = "runnable"]

\* ---------------------------------------------------------------------------
\* Workers: dequeue and slice results (worker_loop, handle_slice_result)

Dequeue(a, p) ==
    /\ a \in Workers
    /\ wk[a].pc = "idle"
    /\ p \in queued
    /\ queued' = queued \ {p}
    /\ IF loc[p] = "table" /\ st[p] = "runnable"
       THEN /\ loc' = [loc EXCEPT ![p] = a]
            /\ pcancel' = pcancel \ {p}
            /\ creq' = IF p \in pcancel THEN creq \union {p} ELSE creq
            /\ wk' = [wk EXCEPT ![a] = [Fresh EXCEPT !.pc = "slice", !.p = p]]
       ELSE UNCHANGED <<loc, pcancel, creq, wk>>
    /\ UNCHANGED <<st, target, akind, targets, tok, par, kids, live, tomb,
                   waiters, anyw, iow, backend, next_pid, next_tok>>

InSlice(a) == wk[a].pc = "slice" /\ wk[a].p \notin creq

SliceCancelled(a) ==
    LET p == wk[a].p IN
    /\ wk[a].pc = "slice"
    /\ p \in creq
    /\ creq' = creq \ {p}
    /\ wk' = [wk EXCEPT ![a].pc = "drain", ![a].p = 0,
                        ![a].term = {[pid |-> p, out |-> "cancelled"]}]
    /\ UNCHANGED <<loc, st, target, akind, targets, tok, par, kids, live, tomb,
                   waiters, anyw, iow, backend, pcancel, queued, next_pid, next_tok>>

SliceYield(a) ==
    LET p == wk[a].p IN
    /\ InSlice(a)
    /\ loc' = ResumeLoc(p)
    /\ st' = ResumeSt(p)
    /\ queued' = queued \union {p}
    /\ wk' = [wk EXCEPT ![a] = Fresh]
    /\ UNCHANGED <<target, akind, targets, tok, par, kids, live, tomb, waiters,
                   anyw, iow, backend, pcancel, creq, next_pid, next_tok>>

SliceSpawn(a) ==
    LET p == wk[a].p
        c == next_pid
    IN
    /\ InSlice(a)
    /\ next_pid <= MaxProcesses
    /\ loc' = [loc EXCEPT ![c] = "table", ![p] = "table"]
    /\ st' = [st EXCEPT ![c] = "runnable", ![p] = "runnable"]
    /\ par' = [par EXCEPT ![c] = p]
    /\ kids' = [kids EXCEPT ![p] = @ \union {c}]
    /\ live' = live \union {c}
    /\ queued' = queued \union {c, p}
    /\ next_pid' = next_pid + 1
    /\ wk' = [wk EXCEPT ![a] = Fresh]
    /\ UNCHANGED <<target, akind, targets, tok, tomb, waiters, anyw, iow,
                   backend, pcancel, creq, next_tok>>

SliceFinish(a, o) ==
    LET p == wk[a].p IN
    /\ InSlice(a)
    /\ wk' = [wk EXCEPT ![a].pc = "drain", ![a].p = 0,
                        ![a].term = {[pid |-> p, out |-> o]}]
    /\ UNCHANGED <<loc, st, target, akind, targets, tok, par, kids, live, tomb,
                   waiters, anyw, iow, backend, pcancel, creq, queued, next_pid, next_tok>>

\* handle_await. A live child: register, then park, then recheck
\* (block_on_child). A terminated child: consume its tombstone at once.
SliceAwait(a, c, kind) ==
    LET p == wk[a].p IN
    /\ InSlice(a)
    /\ c \in kids[p]
    /\ IF c \in live
       THEN /\ st' = [st EXCEPT ![p] = "await"]
            /\ target' = [target EXCEPT ![p] = c]
            /\ akind' = [akind EXCEPT ![p] = kind]
            /\ waiters' = [waiters EXCEPT ![c] = @ \union {p}]
            /\ wk' = [wk EXCEPT ![a].pc = "park", ![a].pk = p, ![a].c = c, ![a].p = 0,
                                ![a].stack = <<"await_recheck">>]
            /\ UNCHANGED <<loc, kids, tomb, queued>>
       ELSE \* "not live" implies "tombstone recorded" while the parent lives;
            \* a missing one is reported as already joined.
            LET fails == kind = "raw" /\ tomb[c] # "ok" IN
            /\ tomb' = [tomb EXCEPT ![c] = "none"]
            /\ kids' = [kids EXCEPT ![p] = @ \ {c}]
            /\ IF fails
               THEN /\ wk' = [wk EXCEPT ![a].pc = "drain", ![a].p = 0,
                                        ![a].term = {[pid |-> p, out |-> "err"]}]
                    /\ UNCHANGED <<loc, st, queued>>
               ELSE /\ loc' = ResumeLoc(p)
                    /\ st' = ResumeSt(p)
                    /\ queued' = queued \union {p}
                    /\ wk' = [wk EXCEPT ![a] = Fresh]
            /\ UNCHANGED <<target, akind, waiters>>
    /\ UNCHANGED <<targets, tok, par, live, anyw, iow, backend, pcancel, creq,
                   next_pid, next_tok>>

\* handle_cancel: cancel a live child, finish it, then resume the parent.
SliceCancel(a, c) ==
    LET p == wk[a].p IN
    /\ InSlice(a)
    /\ c \in kids[p]
    /\ IF c \in live
       THEN /\ wk' = [wk EXCEPT ![a] =
                          CallCancel([@ EXCEPT !.stack = <<"cancel_resume">>], c, "drain")]
            /\ UNCHANGED <<loc, st, queued>>
       ELSE /\ loc' = ResumeLoc(p)
            /\ st' = ResumeSt(p)
            /\ queued' = queued \union {p}
            /\ wk' = [wk EXCEPT ![a] = Fresh]
    /\ UNCHANGED <<target, akind, targets, tok, par, kids, live, tomb, waiters,
                   anyw, iow, backend, pcancel, creq, next_pid, next_tok>>

CancelResume(a) ==
    LET p == wk[a].p IN
    /\ wk[a].pc = "cancel_resume"
    /\ loc' = ResumeLoc(p)
    /\ st' = ResumeSt(p)
    /\ queued' = queued \union {p}
    /\ wk' = [wk EXCEPT ![a] = Fresh]
    /\ UNCHANGED <<target, akind, targets, tok, par, kids, live, tomb, waiters,
                   anyw, iow, backend, pcancel, creq, next_pid, next_tok>>

\* handle_wait_any. A terminated listed child: deliver at once. Otherwise
\* register with every listed child, park, then recheck.
SliceWaitAny(a, s) ==
    LET p == wk[a].p IN
    /\ InSlice(a)
    /\ s # {}
    /\ s \subseteq kids[p]
    /\ IF \E y \in s : y \notin live
       THEN /\ loc' = ResumeLoc(p)
            /\ st' = ResumeSt(p)
            /\ queued' = queued \union {p}
            /\ wk' = [wk EXCEPT ![a] = Fresh]
            /\ UNCHANGED <<targets, anyw>>
       ELSE /\ st' = [st EXCEPT ![p] = "any"]
            /\ targets' = [targets EXCEPT ![p] = s]
            /\ anyw' = [y \in Pids |-> IF y \in s THEN anyw[y] \union {p} ELSE anyw[y]]
            /\ wk' = [wk EXCEPT ![a].pc = "park", ![a].pk = p, ![a].s = s, ![a].p = 0,
                                ![a].stack = <<"any_recheck">>]
            /\ UNCHANGED <<loc, queued>>
    /\ UNCHANGED <<target, akind, tok, par, kids, live, tomb, waiters, iow,
                   backend, pcancel, creq, next_pid, next_tok>>

\* RunResult::Io: register and park before handing the request to the backend.
SliceIo(a) ==
    LET p == wk[a].p
        t == next_tok
    IN
    /\ InSlice(a)
    /\ next_tok <= MaxIo
    /\ st' = [st EXCEPT ![p] = "io"]
    /\ tok' = [tok EXCEPT ![p] = t]
    /\ iow' = [iow EXCEPT ![t] = p]
    /\ next_tok' = next_tok + 1
    /\ wk' = [wk EXCEPT ![a].pc = "park", ![a].pk = p, ![a].t = t, ![a].p = 0,
                        ![a].stack = <<"drain", "io_register">>]
    /\ UNCHANGED <<loc, target, akind, targets, par, kids, live, tomb, waiters,
                   anyw, backend, pcancel, creq, queued, next_pid>>

\* Cancelled while parking: never start the request.
IoRegister(a) ==
    LET t == wk[a].t IN
    /\ wk[a].pc = "io_register"
    /\ backend' = IF iow[t] # 0 THEN backend \union {t} ELSE backend
    /\ wk' = [wk EXCEPT ![a] = Fresh]
    /\ UNCHANGED <<loc, st, target, akind, targets, tok, par, kids, live, tomb,
                   waiters, anyw, iow, pcancel, creq, queued, next_pid, next_tok>>

AwaitRecheck(a) ==
    LET c == wk[a].c IN
    /\ wk[a].pc = "await_recheck"
    /\ IF RecheckAfterPark /\ c \notin live
       THEN wk' = [wk EXCEPT ![a] = Call([@ EXCEPT !.fin = c, !.c = 0], "wj0", "drain")]
       ELSE wk' = [wk EXCEPT ![a].pc = "drain", ![a].c = 0]
    /\ UNCHANGED <<loc, st, target, akind, targets, tok, par, kids, live, tomb,
                   waiters, anyw, iow, backend, pcancel, creq, queued, next_pid, next_tok>>

AnyRecheck(a) ==
    /\ wk[a].pc = "any_recheck"
    /\ IF RecheckAfterPark /\ \E y \in wk[a].s : y \notin live
       THEN \E y \in wk[a].s :
                /\ y \notin live
                /\ wk' = [wk EXCEPT ![a] = Call([@ EXCEPT !.fin = y, !.s = {}], "wa0", "drain")]
       ELSE wk' = [wk EXCEPT ![a].pc = "drain", ![a].s = {}]
    /\ UNCHANGED <<loc, st, target, akind, targets, tok, par, kids, live, tomb,
                   waiters, anyw, iow, backend, pcancel, creq, queued, next_pid, next_tok>>

\* ---------------------------------------------------------------------------
\* park: return an owned blocked process to the table, then observe a
\* cancellation requested while it was owned.

Park(a) ==
    LET pk == wk[a].pk IN
    /\ wk[a].pc = "park"
    /\ loc' = [loc EXCEPT ![pk] = "table"]
    /\ wk' = [wk EXCEPT ![a].pc = "park2"]
    /\ UNCHANGED <<st, target, akind, targets, tok, par, kids, live, tomb,
                   waiters, anyw, iow, backend, pcancel, creq, queued, next_pid, next_tok>>

Park2(a) ==
    LET pk == wk[a].pk IN
    /\ wk[a].pc = "park2"
    /\ IF pk \in pcancel
       THEN /\ pcancel' = pcancel \ {pk}
            /\ wk' = [wk EXCEPT ![a].pc = "tc", ![a].x = pk, ![a].pk = 0]
       ELSE /\ wk' = [wk EXCEPT ![a] = Return([@ EXCEPT !.pk = 0])]
            /\ UNCHANGED pcancel
    /\ UNCHANGED <<loc, st, target, akind, targets, tok, par, kids, live, tomb,
                   waiters, anyw, iow, backend, creq, queued, next_pid, next_tok>>

\* ---------------------------------------------------------------------------
\* try_cancel_in_table(x). A blocked process is taken, its registration
\* removed, and its termination queued; a runnable one gets the VM cancel
\* flag; a process out of the table is reported as not found.

TryCancel(a) ==
    LET x == wk[a].x IN
    /\ wk[a].pc = "tc"
    /\ IF loc[x] = "table" /\ st[x] \in BlockedStates
       THEN /\ loc' = [loc EXCEPT ![x] = a]
            /\ waiters' = IF st[x] = "await"
                          THEN [waiters EXCEPT ![target[x]] = @ \ {x}]
                          ELSE waiters
            /\ anyw' = IF st[x] = "any"
                       THEN [y \in Pids |-> IF y \in targets[x] THEN anyw[y] \ {x} ELSE anyw[y]]
                       ELSE anyw
            /\ iow' = IF st[x] = "io" THEN [iow EXCEPT ![tok[x]] = 0] ELSE iow
            /\ wk' = [wk EXCEPT ![a] =
                          TcReturn([@ EXCEPT !.term = @ \union {[pid |-> x, out |-> "cancelled"]}],
                                   TRUE)]
            /\ UNCHANGED creq
       ELSE /\ creq' = IF loc[x] = "table" /\ st[x] = "runnable"
                       THEN creq \union {x} ELSE creq
            /\ wk' = [wk EXCEPT ![a] = TcReturn(@, loc[x] = "table")]
            /\ UNCHANGED <<loc, waiters, anyw, iow>>
    /\ UNCHANGED <<st, target, akind, targets, tok, par, kids, live, tomb,
                   backend, pcancel, queued, next_pid, next_tok>>

\* cancel_pid(x), x out of the table: leave a request for its owner.
CancelPid2(a) ==
    /\ wk[a].pc = "cp2"
    /\ pcancel' = pcancel \union {wk[a].x}
    /\ wk' = [wk EXCEPT ![a].pc = "cp3"]
    /\ UNCHANGED <<loc, st, target, akind, targets, tok, par, kids, live, tomb,
                   waiters, anyw, iow, backend, creq, queued, next_pid, next_tok>>

\* Terminated meanwhile: drop the request. Otherwise the owner may have
\* parked it before seeing the request: try again.
CancelPid3(a) ==
    LET x == wk[a].x IN
    /\ wk[a].pc = "cp3"
    /\ IF x \notin live
       THEN /\ pcancel' = pcancel \ {x}
            /\ wk' = [wk EXCEPT ![a] = Return([@ EXCEPT !.x = 0])]
       ELSE /\ wk' = [wk EXCEPT ![a].pc = "tc"]
            /\ UNCHANGED pcancel
    /\ UNCHANGED <<loc, st, target, akind, targets, tok, par, kids, live, tomb,
                   waiters, anyw, iow, backend, creq, queued, next_pid, next_tok>>

\* ---------------------------------------------------------------------------
\* drain_terminations and finish_process

Drain(a) ==
    /\ wk[a].pc = "drain"
    /\ wk[a].term = {}
    /\ wk' = [wk EXCEPT ![a] = Return(@)]
    /\ UNCHANGED <<loc, st, target, akind, targets, tok, par, kids, live, tomb,
                   waiters, anyw, iow, backend, pcancel, creq, queued, next_pid, next_tok>>

\* Take the next termination. 1. Record the outcome: a tombstone for a child,
\* a terminal status for the root.
Finish1(a) ==
    /\ wk[a].pc = "drain"
    /\ \E e \in wk[a].term :
        /\ queued' = queued \ {e.pid}
        /\ IF par[e.pid] # 0
           THEN /\ loc' = [loc EXCEPT ![e.pid] = "gone"]
                /\ tomb' = [tomb EXCEPT ![e.pid] = e.out]
                /\ UNCHANGED st
           ELSE /\ loc' = [loc EXCEPT ![e.pid] = "table"]
                /\ st' = [st EXCEPT ![e.pid] = IF e.out = "ok" THEN "done" ELSE "failed"]
                /\ UNCHANGED tomb
        /\ wk' = [wk EXCEPT ![a] = Call([@ EXCEPT !.q = e.pid, !.term = @ \ {e}], "f2", "drain")]
    /\ UNCHANGED <<target, akind, targets, tok, par, kids, live, waiters, anyw,
                   iow, backend, pcancel, creq, next_pid, next_tok>>

\* A child whose parent is gone reaps its own tombstone.
Finish2(a) ==
    LET q == wk[a].q IN
    /\ wk[a].pc = "f2"
    /\ tomb' = IF par[q] # 0 /\ par[q] \notin live
               THEN [tomb EXCEPT ![q] = "none"] ELSE tomb
    /\ wk' = [wk EXCEPT ![a].pc = "f3"]
    /\ UNCHANGED <<loc, st, target, akind, targets, tok, par, kids, live, waiters,
                   anyw, iow, backend, pcancel, creq, queued, next_pid, next_tok>>

\* 2. Stop being live.
Finish3(a) ==
    /\ wk[a].pc = "f3"
    /\ live' = live \ {wk[a].q}
    /\ wk' = [wk EXCEPT ![a].pc = "f4"]
    /\ UNCHANGED <<loc, st, target, akind, targets, tok, par, kids, tomb, waiters,
                   anyw, iow, backend, pcancel, creq, queued, next_pid, next_tok>>

Finish4(a) ==
    LET q == wk[a].q IN
    /\ wk[a].pc = "f4"
    /\ pcancel' = pcancel \ {q}
    /\ kids' = [kids EXCEPT ![q] = {}]
    /\ wk' = [wk EXCEPT ![a].pc = "f5", ![a].ktodo = kids[q]]
    /\ UNCHANGED <<loc, st, target, akind, targets, tok, par, live, tomb, waiters,
                   anyw, iow, backend, creq, queued, next_pid, next_tok>>

\* 3. Scope exit: cancel live children, then remove their tombstones.
Finish5(a) ==
    /\ wk[a].pc = "f5"
    /\ IF wk[a].ktodo = {}
       THEN \* 4. Wake wait_any waiters, then await waiters.
            wk' = [wk EXCEPT ![a] = Call([@ EXCEPT !.fin = wk[a].q, !.q = 0], "wa0", "wj0")]
       ELSE \E k \in wk[a].ktodo :
                LET r == [wk[a] EXCEPT !.ktodo = @ \ {k}, !.k = k] IN
                wk' = [wk EXCEPT ![a] =
                          IF k \in live
                          THEN CallCancel(r, k, "f5b")
                          ELSE [r EXCEPT !.pc = "f5b"]]
    /\ UNCHANGED <<loc, st, target, akind, targets, tok, par, kids, live, tomb,
                   waiters, anyw, iow, backend, pcancel, creq, queued, next_pid, next_tok>>

Finish5b(a) ==
    /\ wk[a].pc = "f5b"
    /\ tomb' = IF ReapTombstones THEN [tomb EXCEPT ![wk[a].k] = "none"] ELSE tomb
    /\ wk' = [wk EXCEPT ![a].pc = "f5", ![a].k = 0]
    /\ UNCHANGED <<loc, st, target, akind, targets, tok, par, kids, live, waiters,
                   anyw, iow, backend, pcancel, creq, queued, next_pid, next_tok>>

\* ---------------------------------------------------------------------------
\* wake_any_waiters(fin)

WakeAny0(a) ==
    LET fin == wk[a].fin IN
    /\ wk[a].pc = "wa0"
    /\ wk' = [wk EXCEPT ![a] =
                 IF anyw[fin] = {} THEN EndWakeAny(@)
                 ELSE [@ EXCEPT !.pc = "wa1", !.wtodo = anyw[fin]]]
    /\ UNCHANGED <<loc, st, target, akind, targets, tok, par, kids, live, tomb,
                   waiters, anyw, iow, backend, pcancel, creq, queued, next_pid, next_tok>>

\* take_waiter: take it if it is parked waiting for fin; skip it if it is in
\* flight (live, out of the table); drop a stale registration otherwise.
WakeAny1(a) ==
    LET fin == wk[a].fin IN
    /\ wk[a].pc = "wa1"
    /\ IF wk[a].wtodo = {}
       THEN /\ wk' = [wk EXCEPT ![a] = EndWakeAny(@)]
            /\ UNCHANGED <<loc, anyw>>
       ELSE \E w2 \in wk[a].wtodo :
            LET r == [wk[a] EXCEPT !.wtodo = @ \ {w2}, !.cur = w2] IN
            IF loc[w2] = "table" /\ st[w2] = "any" /\ fin \in targets[w2]
            THEN /\ loc' = [loc EXCEPT ![w2] = a]
                 /\ wk' = [wk EXCEPT ![a] = [r EXCEPT !.pc = "wa2"]]
                 /\ UNCHANGED anyw
            ELSE /\ anyw' = IF loc[w2] = "table" /\ w2 \in live
                            THEN [anyw EXCEPT ![fin] = @ \ {w2}]
                            ELSE IF w2 \in live
                                 THEN anyw
                                 ELSE RemoveEverywhere(anyw, w2)
                 /\ wk' = [wk EXCEPT ![a] = r]
                 /\ UNCHANGED loc
    /\ UNCHANGED <<st, target, akind, targets, tok, par, kids, live, tomb, waiters,
                   iow, backend, pcancel, creq, queued, next_pid, next_tok>>

\* Deliver a terminated listed child, or park the waiter again.
WakeAny2(a) ==
    LET cur == wk[a].cur IN
    /\ wk[a].pc = "wa2"
    /\ IF \E y \in targets[cur] : y \notin live
       THEN /\ anyw' = [y \in Pids |-> IF y \in targets[cur] THEN anyw[y] \ {cur} ELSE anyw[y]]
            /\ wk' = [wk EXCEPT ![a].pc = "wa3"]
       ELSE /\ wk' = [wk EXCEPT ![a] = Call([@ EXCEPT !.pk = cur], "park", "wa1")]
            /\ UNCHANGED anyw
    /\ UNCHANGED <<loc, st, target, akind, targets, tok, par, kids, live, tomb,
                   waiters, iow, backend, pcancel, creq, queued, next_pid, next_tok>>

WakeAny3(a) ==
    LET cur == wk[a].cur IN
    /\ wk[a].pc = "wa3"
    /\ loc' = ResumeLoc(cur)
    /\ st' = ResumeSt(cur)
    /\ queued' = queued \union {cur}
    /\ wk' = [wk EXCEPT ![a].pc = "wa1"]
    /\ UNCHANGED <<target, akind, targets, tok, par, kids, live, tomb, waiters,
                   anyw, iow, backend, pcancel, creq, next_pid, next_tok>>

\* ---------------------------------------------------------------------------
\* wake_join_waiters(fin)

WakeJoin0(a) ==
    LET fin == wk[a].fin IN
    /\ wk[a].pc = "wj0"
    /\ IF waiters[fin] = {}
       THEN /\ wk' = [wk EXCEPT ![a] = EndWakeJoin(@)]
            /\ UNCHANGED waiters
       ELSE IF tomb[fin] = "none"
            THEN \* Reaped: its parent is gone, so nobody can still be waiting.
                 /\ waiters' = [waiters EXCEPT ![fin] = {}]
                 /\ wk' = [wk EXCEPT ![a] = EndWakeJoin(@)]
            ELSE /\ wk' = [wk EXCEPT ![a].pc = "wj1", ![a].wtodo = waiters[fin]]
                 /\ UNCHANGED waiters
    /\ UNCHANGED <<loc, st, target, akind, targets, tok, par, kids, live, tomb,
                   anyw, iow, backend, pcancel, creq, queued, next_pid, next_tok>>

WakeJoin1(a) ==
    LET fin == wk[a].fin IN
    /\ wk[a].pc = "wj1"
    /\ IF wk[a].wtodo = {}
       THEN /\ wk' = [wk EXCEPT ![a] = EndWakeJoin(@)]
            /\ UNCHANGED <<loc, waiters>>
       ELSE \E w2 \in wk[a].wtodo :
            LET r == [wk[a] EXCEPT !.wtodo = @ \ {w2}, !.cur = w2] IN
            IF loc[w2] = "table" /\ st[w2] = "await" /\ target[w2] = fin
            THEN /\ loc' = [loc EXCEPT ![w2] = a]
                 /\ wk' = [wk EXCEPT ![a] = [r EXCEPT !.pc = "wj2"]]
                 /\ UNCHANGED waiters
            ELSE /\ waiters' = IF loc[w2] = "table" /\ w2 \in live
                               THEN [waiters EXCEPT ![fin] = @ \ {w2}]
                               ELSE IF w2 \in live
                                    THEN waiters
                                    ELSE RemoveEverywhere(waiters, w2)
                 /\ wk' = [wk EXCEPT ![a] = r]
                 /\ UNCHANGED loc
    /\ UNCHANGED <<st, target, akind, targets, tok, par, kids, live, tomb, anyw,
                   iow, backend, pcancel, creq, queued, next_pid, next_tok>>

\* Deliver: consume the tombstone (only the parent awaits a child). A raw
\* await of a failed child, or of a child already joined, fails the waiter.
WakeJoin2(a) ==
    /\ wk[a].pc = "wj2"
    /\ LET fin == wk[a].fin
           cur == wk[a].cur
           fails == akind[cur] = "raw" /\ tomb[fin] # "ok"
       IN
       /\ waiters' = [waiters EXCEPT ![fin] = @ \ {cur}]
       /\ tomb' = [tomb EXCEPT ![fin] = "none"]
       /\ kids' = [kids EXCEPT ![cur] = @ \ {fin}]
       /\ wk' = [wk EXCEPT ![a] =
                    IF fails
                    THEN [@ EXCEPT !.pc = "wj1", !.term = @ \union {[pid |-> cur, out |-> "err"]}]
                    ELSE [@ EXCEPT !.pc = "wj3"]]
    /\ UNCHANGED <<loc, st, target, akind, targets, tok, par, live, anyw, iow,
                   backend, pcancel, creq, queued, next_pid, next_tok>>

WakeJoin3(a) ==
    LET cur == wk[a].cur IN
    /\ wk[a].pc = "wj3"
    /\ loc' = ResumeLoc(cur)
    /\ st' = ResumeSt(cur)
    /\ queued' = queued \union {cur}
    /\ wk' = [wk EXCEPT ![a].pc = "wj1"]
    /\ UNCHANGED <<target, akind, targets, tok, par, kids, live, tomb, waiters,
                   anyw, iow, backend, pcancel, creq, next_pid, next_tok>>

\* ---------------------------------------------------------------------------
\* I/O monitor: handle_io_completion. The backend may report a completion for
\* a token that was cancelled; the runtime drops it.

MonitorPoll(t) ==
    /\ wk[Mon].pc = "idle"
    /\ t \in backend
    /\ backend' = backend \ {t}
    /\ IF iow[t] = 0
       THEN UNCHANGED <<iow, wk>>
       ELSE /\ iow' = [iow EXCEPT ![t] = 0]
            /\ wk' = [wk EXCEPT ![Mon] = [Fresh EXCEPT !.pc = "m1", !.cur = iow[t], !.t = t]]
    /\ UNCHANGED <<loc, st, target, akind, targets, tok, par, kids, live, tomb,
                   waiters, anyw, pcancel, creq, queued, next_pid, next_tok>>

MonitorTake ==
    LET cur == wk[Mon].cur IN
    /\ wk[Mon].pc = "m1"
    /\ IF loc[cur] = "table" /\ st[cur] = "io" /\ tok[cur] = wk[Mon].t
       THEN /\ loc' = [loc EXCEPT ![cur] = Mon]
            /\ wk' = [wk EXCEPT ![Mon].pc = "m2"]
            /\ UNCHANGED iow
       ELSE \* Defensive: a registration for a process not waiting on it.
            /\ iow' = [t \in Toks |-> IF iow[t] = cur THEN 0 ELSE iow[t]]
            /\ wk' = [wk EXCEPT ![Mon] = Fresh]
            /\ UNCHANGED loc
    /\ UNCHANGED <<st, target, akind, targets, tok, par, kids, live, tomb, waiters,
                   anyw, backend, pcancel, creq, queued, next_pid, next_tok>>

MonitorDeliver(ok) ==
    LET cur == wk[Mon].cur IN
    /\ wk[Mon].pc = "m2"
    /\ IF ok
       THEN /\ loc' = ResumeLoc(cur)
            /\ st' = ResumeSt(cur)
            /\ queued' = queued \union {cur}
            /\ wk' = [wk EXCEPT ![Mon] = Fresh]
       ELSE /\ wk' = [wk EXCEPT ![Mon].pc = "drain", ![Mon].cur = 0, ![Mon].t = 0,
                                ![Mon].term = {[pid |-> cur, out |-> "err"]}]
            /\ UNCHANGED <<loc, st, queued>>
    /\ UNCHANGED <<target, akind, targets, tok, par, kids, live, tomb, waiters,
                   anyw, iow, backend, pcancel, creq, next_pid, next_tok>>

\* ---------------------------------------------------------------------------

\* Every action is its own disjunct of Next. With tla-checker, grouping actions
\* in an operator made exploration far slower or stall, and a CASE dispatch on
\* pc produced successor states the actions do not allow.
Next ==
    \/ \E a \in Workers, p \in Pids : Dequeue(a, p)
    \/ \E a \in Workers : SliceCancelled(a)
    \/ \E a \in Workers : SliceYield(a)
    \/ \E a \in Workers : SliceSpawn(a)
    \/ \E a \in Workers, o \in {"ok", "err"} : SliceFinish(a, o)
    \/ \E a \in Workers, c \in Pids, kind \in {"raw", "result"} : SliceAwait(a, c, kind)
    \/ \E a \in Workers, c \in Pids : SliceCancel(a, c)
    \/ \E a \in Workers, s \in SUBSET Pids : SliceWaitAny(a, s)
    \/ \E a \in Workers : SliceIo(a)
    \/ \E a \in Actors :
        \/ Park(a) \/ Park2(a)
        \/ TryCancel(a) \/ CancelPid2(a) \/ CancelPid3(a)
        \/ Drain(a)
        \/ Finish1(a) \/ Finish2(a) \/ Finish3(a) \/ Finish4(a) \/ Finish5(a)
        \/ Finish5b(a)
        \/ WakeAny0(a) \/ WakeAny1(a) \/ WakeAny2(a) \/ WakeAny3(a)
        \/ WakeJoin0(a) \/ WakeJoin1(a) \/ WakeJoin2(a) \/ WakeJoin3(a)
        \/ AwaitRecheck(a) \/ AnyRecheck(a) \/ IoRegister(a) \/ CancelResume(a)
    \/ \E t \in Toks : MonitorPoll(t)
    \/ MonitorTake
    \/ \E ok \in BOOLEAN : MonitorDeliver(ok)

Spec == Init /\ [][Next]_vars

\* ---------------------------------------------------------------------------
\* Invariants

TypeOK ==
    /\ \A p \in Pids :
        /\ loc[p] \in {"none", "table", "gone"} \union Actors
        /\ st[p] \in {"runnable", "await", "any", "io", "done", "failed"}
        /\ tomb[p] \in {"none", "ok", "err", "cancelled"}
        /\ par[p] \in Pids \union {0}
        /\ kids[p] \subseteq Pids
        /\ waiters[p] \subseteq Pids
        /\ anyw[p] \subseteq Pids
    /\ live \subseteq Pids
    /\ pcancel \subseteq Pids
    /\ creq \subseteq Pids
    /\ queued \subseteq Pids
    /\ backend \subseteq Toks
    /\ \A t \in Toks : iow[t] \in Pids \union {0}

Quiescent ==
    /\ \A a \in Actors : wk[a].pc = "idle"
    /\ queued = {}
    /\ backend = {}

\* Nothing can run, so every process must have terminated: a live process
\* here is blocked with no waker left.
NoLostWakeup == Quiescent => live = {}

CleanAtQuiescence ==
    Quiescent =>
        /\ \A p \in Pids : waiters[p] = {} /\ anyw[p] = {} /\ tomb[p] = "none"
        /\ \A t \in Toks : iow[t] = 0
        /\ pcancel = {}
        /\ creq = {}

\* A terminated process is never back in the table as runnable or blocked,
\* and nobody owns it.
NoResurrection ==
    \A p \in Pids :
        (loc[p] # "none" /\ p \notin live) =>
            \/ loc[p] = "gone"
            \/ par[p] = 0 /\ loc[p] = "table" /\ st[p] \in {"done", "failed"}

\* Actor a is terminating x and has not yet reaped the tombstone of child p.
ReapingChild(a, x, p) ==
    /\ wk[a].q = x
    /\ \/ wk[a].pc = "f4"
       \/ p \in wk[a].ktodo
       \/ wk[a].k = p /\ (wk[a].pc = "f5b" \/ "f5b" \in {wk[a].stack[i] : i \in DOMAIN wk[a].stack})

\* A tombstone exists only for a child of a live parent, while the terminating
\* parent has not reached it in its scope-exit loop, or between the child
\* recording it and checking whether its parent is gone.
BoundedTombstones ==
    \A p \in Pids :
        tomb[p] # "none" =>
            \/ par[p] \in live
            \/ \E a \in Actors : ReapingChild(a, par[p], p)
            \/ \E a \in Actors : wk[a].q = p /\ wk[a].pc = "f2"

\* A pending cancel names a live process, except between cancel_pid recording
\* it and checking whether the process terminated meanwhile, and between a
\* terminating process stopping being live and clearing its request.
PendingCancelsConsistent ==
    \A p \in pcancel :
        \/ p \in live
        \/ \E a \in Actors : wk[a].x = p /\ wk[a].pc = "cp3"
        \/ \E a \in Actors : wk[a].q = p /\ wk[a].pc = "f4"

\* A registered waiter is never terminated and still waiting for that child
\* once it is back in the table; stale entries name live processes.
RegistrationsConsistent ==
    /\ \A c \in Pids : \A w \in waiters[c] :
        loc[w] = "table" /\ w \in live => st[w] = "await" /\ target[w] = c
    /\ \A t \in Toks : iow[t] # 0 => iow[t] \in live

SafetyInvariant ==
    /\ TypeOK
    /\ NoLostWakeup
    /\ CleanAtQuiescence
    /\ NoResurrection
    /\ BoundedTombstones
    /\ PendingCancelsConsistent
    /\ RegistrationsConsistent

====
