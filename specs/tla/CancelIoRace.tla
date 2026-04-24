---- MODULE CancelIoRace ----
\* # Documentation
\*
\* ## Why this spec exists
\*
\* This focused model checks the race between cooperative cancellation and
\* asynchronous I/O completion. In the threaded runtime, a process can block on
\* I/O, be cancelled by its parent, and later receive an I/O completion from the
\* monitor/backend. The important safety rule is that a stale I/O completion must
\* not resurrect a cancelled terminal process.
\*
\* ## What we model
\*
\* One child process starts blocked on I/O. The parent may cancel it, the I/O
\* backend may complete/fail, and stale completions may arrive after terminal
\* cancellation. We model only enough state to check terminal absorption and
\* stale I/O cleanup.
\*
\* ## What we intentionally abstract
\*
\* We omit bytecode, heaps, process IDs beyond the parent/child pair, and exact
\* result payloads. The implementation-level details live in
\* ThreadedSchedulerImpl.tla; this file is a fast regression harness for the
\* cancellation/I/O race class.
\*
\* ## Why this is a good idea
\*
\* Cancellation and I/O completions are delivered by different runtime paths.
\* Without an executable model, it is easy to accidentally allow a stale I/O
\* completion to make a cancelled process runnable again.

EXTENDS Naturals, Sequences, FiniteSets, TLC

VARIABLES status, io_waiter, terminal_reason, stale_completion_seen

vars == <<status, io_waiter, terminal_reason, stale_completion_seen>>

BlockedIo == "blocked_io"
Runnable == "runnable"
Failed == "failed"
None == "none"
Cancelled == "cancelled"
IoFailed == "io_failed"
IoOk == "io_ok"

Init ==
    /\ status = BlockedIo
    /\ io_waiter = TRUE
    /\ terminal_reason = None
    /\ stale_completion_seen = FALSE

CancelBlockedIo ==
    /\ status = BlockedIo
    /\ io_waiter = TRUE
    /\ status' = Failed
    /\ terminal_reason' = Cancelled
    /\ io_waiter' = FALSE
    /\ UNCHANGED stale_completion_seen

IoCompleteBeforeCancel ==
    /\ status = BlockedIo
    /\ io_waiter = TRUE
    /\ status' = Runnable
    /\ terminal_reason' = IoOk
    /\ io_waiter' = FALSE
    /\ UNCHANGED stale_completion_seen

IoFailBeforeCancel ==
    /\ status = BlockedIo
    /\ io_waiter = TRUE
    /\ status' = Failed
    /\ terminal_reason' = IoFailed
    /\ io_waiter' = FALSE
    /\ UNCHANGED stale_completion_seen

StaleIoCompletionAfterCancel ==
    /\ status = Failed
    /\ terminal_reason = Cancelled
    /\ ~io_waiter
    /\ stale_completion_seen' = TRUE
    /\ UNCHANGED <<status, terminal_reason, io_waiter>>

Next ==
    \/ CancelBlockedIo
    \/ IoCompleteBeforeCancel
    \/ IoFailBeforeCancel
    \/ StaleIoCompletionAfterCancel

TypeOK ==
    /\ status \in {BlockedIo, Runnable, Failed}
    /\ io_waiter \in BOOLEAN
    /\ terminal_reason \in {None, Cancelled, IoFailed, IoOk}
    /\ stale_completion_seen \in BOOLEAN

NoStaleIoResurrection ==
    stale_completion_seen => /\ status = Failed /\ terminal_reason = Cancelled

TerminalHasNoIoWaiter ==
    status = Failed => ~io_waiter

SafetyInvariant ==
    /\ TypeOK
    /\ NoStaleIoResurrection
    /\ TerminalHasNoIoWaiter

Spec == Init /\ [][Next]_vars

====
