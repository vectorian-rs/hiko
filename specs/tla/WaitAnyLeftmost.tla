---- MODULE WaitAnyLeftmost ----
\* # Documentation
\*
\* ## Why this spec exists
\*
\* This is a small, focused model-checking harness for Hiko's deterministic
\* wait_any rule. ProcessLifecycle.tla defines the full process semantics and
\* ThreadedSchedulerImpl.tla models the threaded runtime structure; both are
\* intentionally broader. This file isolates one regression-prone property so it
\* can be explored quickly in normal developer checks.
\*
\* ## What we model
\*
\* We model a parent blocked on the ordered input list <<1, 2>>. The children may
\* complete in either order, and either child may act as the wakeup notifier. The
\* delivered result must always be the leftmost completed child in the original
\* input list at delivery time.
\*
\* ## What we intentionally abstract
\*
\* This model does not include process tables, heaps, bytecode execution,
\* cancellation, I/O, or join-result consumption. Those are covered by the larger
\* lifecycle/runtime specs. Here we deliberately keep only the state needed to
\* catch the specific bug class where the notifying child is incorrectly returned
\* as the wait_any winner.
\*
\* ## Why this is a good idea
\*
\* The original threaded wait_any flake was a tie/race bug. A focused model keeps
\* that behavior executable and cheap to check. If someone changes wait_any to
\* deliver the notifier rather than the leftmost completed child, this spec should
\* produce a short counterexample.

EXTENDS Naturals, Sequences, FiniteSets, TLC

VARIABLES terminal, delivered, done

vars == <<terminal, delivered, done>>

Children == <<1, 2>>
ChildSet == {1, 2}
None == 0

SeqSet(seq) == {seq[i] : i \in 1..Len(seq)}

LeftmostTerminal(children) ==
    CHOOSE child \in SeqSet(children) :
        /\ child \in terminal
        /\ \E i \in 1..Len(children) :
            /\ children[i] = child
            /\ \A j \in 1..(i - 1) : children[j] \notin terminal

Init ==
    /\ terminal = {}
    /\ delivered = None
    /\ done = FALSE

Complete(child) ==
    /\ ~done
    /\ child \in ChildSet
    /\ child \notin terminal
    /\ terminal' = terminal \union {child}
    /\ UNCHANGED <<delivered, done>>

Wake(notifier) ==
    /\ ~done
    /\ notifier \in terminal
    /\ terminal # {}
    /\ delivered' = LeftmostTerminal(Children)
    /\ done' = TRUE
    /\ UNCHANGED terminal

Next ==
    \/ \E child \in ChildSet : Complete(child)
    \/ \E notifier \in ChildSet : Wake(notifier)

TypeOK ==
    /\ terminal \subseteq ChildSet
    /\ delivered \in ChildSet \union {None}
    /\ done \in BOOLEAN

WaitAnyLeftmost ==
    done => delivered = LeftmostTerminal(Children)

SafetyInvariant ==
    /\ TypeOK
    /\ WaitAnyLeftmost

Spec == Init /\ [][Next]_vars

====
