## Structured Resource Lifetime Management and Cancellation

This section defines two essential additions to the Hiko runtime:

* **Structured resource lifetime management**
* **Cooperative cancellation**

These complete the runtime model for robust async execution.

---

# 1. Structured Resource Lifetime Management

## Problem

Without structured ownership:

* spawned processes may outlive their parents
* I/O operations may continue after their results are no longer needed
* resources (sockets, timers, file descriptors) may leak
* suspended continuations may resume into invalid contexts

## Design

Introduce **scopes** as the unit of ownership.

A scope owns:

* child processes spawned within it
* I/O operations initiated within it
* any runtime resources associated with those operations

## API (conceptual)

```sml
with_scope (fn scope =>
  let
    val t1 = spawn_in scope (fn () => ...)
    val t2 = spawn_in scope (fn () => ...)
  in
    (await t1, await t2)
  end)
```

## Semantics

* `with_scope` creates a new scope
* `spawn_in scope` attaches the child process to that scope
* all resources created during execution are registered to the scope

## Scope exit rule

> No child process or resource may outlive its owning scope unless explicitly detached.

When a scope exits:

1. All child processes must be:

   * completed, or
   * cancelled

2. All pending I/O operations must be:

   * completed, or
   * cancelled

3. All runtime resources must be released

## Runtime model

```rust
struct Scope {
    id: ScopeId,
    children: Vec<Pid>,
    resources: Vec<ResourceId>,
}
```

Each process tracks its current scope:

```rust
struct Process {
    ...
    scope_id: ScopeId,
}
```

---

# 2. Cancellation

## Problem

Without cancellation:

* failed computations leave sibling tasks running
* timeouts cannot stop work
* blocked I/O may never be released
* system resources are wasted

## Design

Cancellation is:

> **cooperative and delivered at suspension points**

## Process state

```rust
struct Process {
    ...
    cancelled: bool,
}
```

## Semantics

* cancellation does not immediately terminate execution
* instead, it is observed at:

  * effect suspension points
  * `await`
  * I/O boundaries

## Cancellation propagation

* if a scope fails → all children in that scope are cancelled
* if a child fails inside a structured join → siblings are cancelled
* if a scope exits early → remaining children are cancelled

## I/O interaction

If a process is blocked on I/O:

* runtime attempts to cancel the backend operation (if supported)
* otherwise:

  * completion is ignored
  * process resumes with cancellation

## Result model

```sml
datatype error =
    IoError of string
  | Cancelled
  | ChildFailed of string

datatype 'a result =
    Ok of 'a
  | Err of error
```

## Behavior

* cancelled processes resume with `Err Cancelled`
* `await` on a cancelled process returns `Err Cancelled`
* cancellation is not a runtime crash — it is a normal outcome

---

# Design Principles

1. **Ownership is explicit** — scopes define lifetime boundaries
2. **No orphan work** — all tasks belong to a scope
3. **Cancellation is cooperative** — no arbitrary interruption
4. **Effects remain the suspension mechanism** — no new control primitives
5. **Runtime stays simple** — cancellation handled at well-defined points

---

# Summary

Structured scopes and cooperative cancellation ensure:

* no resource leaks
* no runaway processes
* predictable shutdown behavior
* safe composition of concurrent tasks

Together, they complete the runtime model for effect-based asynchronous execution.

