module ProcessLifecycle {
  /// Literal Quint port of `specs/tla/ProcessLifecycle.tla`.
  ///
  /// Note: this intentionally preserves the mailbox send/receive actions that
  /// still exist in the TLA+ source, even though Hiko's current runtime surface
  /// has already moved away from public mailbox messaging.

  type ProcStatus = Runnable | Blocked | Done | Failed
  type BlockReason = BkNone | BkAwait | BkIo | BkReceive
  type Proc = {
    status: ProcStatus,
    parent: int,
    result: int,
    delivered_to: int,
    block_reason: BlockReason,
    block_target: int,
  }
  type IoEntry = { pid: int, token: int }

  pure val MaxProcesses = 2
  pure val MaxSteps = 8
  pure val MaxMessages = 1
  pure val MaxIoOps = 1
  pure val pidUniverse = 1.to(MaxProcesses)
  pure val ioEntryUniverse =
    pidUniverse.fold(
      Set(),
      (entries, pid) => entries.union(1.to(MaxIoOps).map(token => { pid: pid, token: token }))
    )

  var procs: int -> Proc
  var runqueue: List[int]
  var mailboxes: int -> List[int]
  var waiters: int -> Set[int]
  var io_pending: Set[IoEntry]
  var next_pid: int
  var io_next_token: int
  var msg_count: int
  var io_count: int
  var step_count: int

  pure def emptyProc(parent): (int) => Proc =
    {
      status: Runnable,
      parent: parent,
      result: -1,
      delivered_to: 0,
      block_reason: BkNone,
      block_target: 0,
    }

  pure def clearBlock(proc): (Proc) => Proc =
    proc.with("block_reason", BkNone).with("block_target", 0)

  pure def blockOn(proc, reason, target): (Proc, BlockReason, int) => Proc =
    proc
      .with("status", Blocked)
      .with("block_reason", reason)
      .with("block_target", target)

  pure def withStatus(proc, status): (Proc, ProcStatus) => Proc =
    proc.with("status", status)

  pure def withResult(proc, result): (Proc, int) => Proc =
    proc.with("result", result)

  pure def withDeliveredTo(proc, parent): (Proc, int) => Proc =
    proc.with("delivered_to", parent)

  val knownPids = procs.keys()
  val unknownPidCandidates = 1.to(MaxProcesses).exclude(knownPids)

  val stepOk = step_count < MaxSteps
  val hasRunnableHead = runqueue.length() > 0

  pure def inRunqueue(queue, pid): (List[int], int) => bool =
    queue.indices().exists(i => queue.nth(i) == pid)

  pure def atHead(queue, pid): (List[int], int) => bool =
    queue.length() > 0 and queue.head() == pid

  pure def mailboxMessage(sender): (int) => int = sender * 100

  action init = all {
    procs' = Set(1).mapBy(_ => emptyProc(0)),
    runqueue' = List(1),
    mailboxes' = Set(1).mapBy(_ => List()),
    waiters' = Set(1).mapBy(_ => Set()),
    io_pending' = Set(),
    next_pid' = 2,
    io_next_token' = 1,
    msg_count' = 0,
    io_count' = 0,
    step_count' = 0,
  }

  action disabled = all {
    procs' = procs,
    runqueue' = runqueue,
    mailboxes' = mailboxes,
    waiters' = waiters,
    io_pending' = io_pending,
    next_pid' = next_pid,
    io_next_token' = io_next_token,
    msg_count' = msg_count,
    io_count' = io_count,
    step_count' = step_count,
    false,
  }

  action spawn(pid) = all {
    stepOk,
    procs.get(pid).status == Runnable,
    atHead(runqueue, pid),
    next_pid <= MaxProcesses,
    procs' = procs.put(next_pid, emptyProc(pid)),
    runqueue' = runqueue.tail().concat(List(next_pid, pid)),
    mailboxes' = mailboxes.put(next_pid, List()),
    waiters' = waiters.put(next_pid, Set()),
    io_pending' = io_pending,
    next_pid' = next_pid + 1,
    io_next_token' = io_next_token,
    msg_count' = msg_count,
    io_count' = io_count,
    step_count' = step_count + 1,
  }

  action complete(pid) =
    if (waiters.get(pid).size() == 0)
      all {
        stepOk,
        procs.get(pid).status == Runnable,
        atHead(runqueue, pid),
        procs' = procs.set(pid, withResult(withStatus(procs.get(pid), Done), pid * 10)),
        runqueue' = runqueue.tail(),
        mailboxes' = mailboxes,
        waiters' = waiters,
        io_pending' = io_pending,
        next_pid' = next_pid,
        io_next_token' = io_next_token,
        msg_count' = msg_count,
        io_count' = io_count,
        step_count' = step_count + 1,
      }
    else
      {
        nondet parent = waiters.get(pid).oneOf()
        all {
          stepOk,
          procs.get(pid).status == Runnable,
          atHead(runqueue, pid),
          procs' = procs
            .set(pid, withDeliveredTo(withResult(withStatus(procs.get(pid), Done), -1), parent))
            .set(parent, clearBlock(withStatus(procs.get(parent), Runnable))),
          runqueue' = runqueue.tail().append(parent),
          mailboxes' = mailboxes,
          waiters' = waiters.set(pid, Set()),
          io_pending' = io_pending,
          next_pid' = next_pid,
          io_next_token' = io_next_token,
          msg_count' = msg_count,
          io_count' = io_count,
          step_count' = step_count + 1,
        }
      }

  action completeBadResult(pid) = all {
    stepOk,
    procs.get(pid).status == Runnable,
    atHead(runqueue, pid),
    procs' = procs.keys().mapBy(p =>
      if (p == pid)
        withStatus(procs.get(p), Failed)
      else if (p.in(waiters.get(pid)))
        clearBlock(withStatus(procs.get(p), Failed))
      else
        procs.get(p)
    ),
    runqueue' = runqueue.tail(),
    mailboxes' = mailboxes,
    waiters' = waiters.set(pid, Set()),
    io_pending' = io_pending,
    next_pid' = next_pid,
    io_next_token' = io_next_token,
    msg_count' = msg_count,
    io_count' = io_count,
    step_count' = step_count + 1,
  }

  action failProc(pid) = all {
    stepOk,
    procs.get(pid).status == Runnable,
    atHead(runqueue, pid),
    procs' = procs.keys().mapBy(p =>
      if (p == pid)
        withStatus(procs.get(p), Failed)
      else if (p.in(waiters.get(pid)))
        clearBlock(withStatus(procs.get(p), Failed))
      else
        procs.get(p)
    ),
    runqueue' = runqueue.tail(),
    mailboxes' = mailboxes,
    waiters' = waiters.set(pid, Set()),
    io_pending' = io_pending,
    next_pid' = next_pid,
    io_next_token' = io_next_token,
    msg_count' = msg_count,
    io_count' = io_count,
    step_count' = step_count + 1,
  }

  action yield(pid) = all {
    stepOk,
    procs.get(pid).status == Runnable,
    atHead(runqueue, pid),
    procs' = procs,
    runqueue' = runqueue.tail().append(pid),
    mailboxes' = mailboxes,
    waiters' = waiters,
    io_pending' = io_pending,
    next_pid' = next_pid,
    io_next_token' = io_next_token,
    msg_count' = msg_count,
    io_count' = io_count,
    step_count' = step_count + 1,
  }

  action awaitDone(parent, child) = all {
    stepOk,
    procs.get(parent).status == Runnable,
    atHead(runqueue, parent),
    child.in(knownPids),
    procs.get(child).parent == parent,
    procs.get(child).status == Done,
    procs.get(child).result != -1,
    procs.get(child).delivered_to == 0,
    procs' = procs.set(
      child,
      withDeliveredTo(withResult(procs.get(child), -1), parent)
    ),
    runqueue' = runqueue.tail().append(parent),
    mailboxes' = mailboxes,
    waiters' = waiters,
    io_pending' = io_pending,
    next_pid' = next_pid,
    io_next_token' = io_next_token,
    msg_count' = msg_count,
    io_count' = io_count,
    step_count' = step_count + 1,
  }

  action awaitConsumed(parent, child) = all {
    stepOk,
    procs.get(parent).status == Runnable,
    atHead(runqueue, parent),
    child.in(knownPids),
    procs.get(child).parent == parent,
    procs.get(child).status == Done,
    procs.get(child).delivered_to != 0,
    procs' = procs.set(parent, withStatus(procs.get(parent), Failed)),
    runqueue' = runqueue.tail(),
    mailboxes' = mailboxes,
    waiters' = waiters,
    io_pending' = io_pending,
    next_pid' = next_pid,
    io_next_token' = io_next_token,
    msg_count' = msg_count,
    io_count' = io_count,
    step_count' = step_count + 1,
  }

  action awaitFailed(parent, child) = all {
    stepOk,
    procs.get(parent).status == Runnable,
    atHead(runqueue, parent),
    child.in(knownPids),
    procs.get(child).parent == parent,
    procs.get(child).status == Failed,
    procs' = procs.set(parent, withStatus(procs.get(parent), Failed)),
    runqueue' = runqueue.tail(),
    mailboxes' = mailboxes,
    waiters' = waiters,
    io_pending' = io_pending,
    next_pid' = next_pid,
    io_next_token' = io_next_token,
    msg_count' = msg_count,
    io_count' = io_count,
    step_count' = step_count + 1,
  }

  action awaitBlock(parent, child) = all {
    stepOk,
    procs.get(parent).status == Runnable,
    atHead(runqueue, parent),
    child.in(knownPids),
    procs.get(child).parent == parent,
    procs.get(child).status.in(Set(Runnable, Blocked)),
    procs' = procs.set(parent, blockOn(procs.get(parent), BkAwait, child)),
    runqueue' = runqueue.tail(),
    mailboxes' = mailboxes,
    waiters' = waiters.set(child, waiters.get(child).union(Set(parent))),
    io_pending' = io_pending,
    next_pid' = next_pid,
    io_next_token' = io_next_token,
    msg_count' = msg_count,
    io_count' = io_count,
    step_count' = step_count + 1,
  }

  action awaitUnknown(parent, child_pid) = all {
    stepOk,
    procs.get(parent).status == Runnable,
    atHead(runqueue, parent),
    not(child_pid.in(knownPids)),
    child_pid.in(1.to(MaxProcesses)),
    procs' = procs.set(parent, withStatus(procs.get(parent), Failed)),
    runqueue' = runqueue.tail(),
    mailboxes' = mailboxes,
    waiters' = waiters,
    io_pending' = io_pending,
    next_pid' = next_pid,
    io_next_token' = io_next_token,
    msg_count' = msg_count,
    io_count' = io_count,
    step_count' = step_count + 1,
  }

  action awaitNotChild(parent, child) = all {
    stepOk,
    procs.get(parent).status == Runnable,
    atHead(runqueue, parent),
    child.in(knownPids),
    procs.get(child).parent != parent,
    procs' = procs.set(parent, withStatus(procs.get(parent), Failed)),
    runqueue' = runqueue.tail(),
    mailboxes' = mailboxes,
    waiters' = waiters,
    io_pending' = io_pending,
    next_pid' = next_pid,
    io_next_token' = io_next_token,
    msg_count' = msg_count,
    io_count' = io_count,
    step_count' = step_count + 1,
  }

  action requestIo(pid) = all {
    stepOk,
    procs.get(pid).status == Runnable,
    atHead(runqueue, pid),
    io_count < MaxIoOps,
    procs' = procs.set(pid, blockOn(procs.get(pid), BkIo, io_next_token)),
    runqueue' = runqueue.tail(),
    mailboxes' = mailboxes,
    waiters' = waiters,
    io_pending' = io_pending.union(Set({ pid: pid, token: io_next_token })),
    next_pid' = next_pid,
    io_next_token' = io_next_token + 1,
    msg_count' = msg_count,
    io_count' = io_count + 1,
    step_count' = step_count + 1,
  }

  action ioComplete(entry) =
    if (entry.in(io_pending) and entry.pid.in(knownPids))
      all {
        stepOk,
        procs.get(entry.pid).status == Blocked,
        procs.get(entry.pid).block_reason == BkIo,
        procs.get(entry.pid).block_target == entry.token,
        procs' = procs.set(entry.pid, clearBlock(withStatus(procs.get(entry.pid), Runnable))),
        runqueue' = runqueue.append(entry.pid),
        mailboxes' = mailboxes,
        waiters' = waiters,
        io_pending' = io_pending.exclude(Set(entry)),
        next_pid' = next_pid,
        io_next_token' = io_next_token,
        msg_count' = msg_count,
        io_count' = io_count,
        step_count' = step_count + 1,
      }
    else disabled

  action ioFail(entry) =
    if (entry.in(io_pending) and entry.pid.in(knownPids))
      all {
        stepOk,
        procs.get(entry.pid).status == Blocked,
        procs.get(entry.pid).block_reason == BkIo,
        procs.get(entry.pid).block_target == entry.token,
        procs' = procs.keys().mapBy(p =>
          if (p == entry.pid)
            clearBlock(withStatus(procs.get(p), Failed))
          else if (p.in(waiters.get(entry.pid)))
            clearBlock(withStatus(procs.get(p), Failed))
          else
            procs.get(p)
        ),
        runqueue' = runqueue,
        mailboxes' = mailboxes,
        waiters' = waiters.set(entry.pid, Set()),
        io_pending' = io_pending.exclude(Set(entry)),
        next_pid' = next_pid,
        io_next_token' = io_next_token,
        msg_count' = msg_count,
        io_count' = io_count,
        step_count' = step_count + 1,
      }
    else disabled

  action send(sender, target) =
    if (target == sender)
      all {
        stepOk,
        procs.get(sender).status == Runnable,
        atHead(runqueue, sender),
        target.in(knownPids),
        msg_count < MaxMessages,
        procs' = procs,
        runqueue' = runqueue.tail().append(sender),
        mailboxes' = mailboxes.set(sender, mailboxes.get(sender).append(mailboxMessage(sender))),
        waiters' = waiters,
        io_pending' = io_pending,
        next_pid' = next_pid,
        io_next_token' = io_next_token,
        msg_count' = msg_count + 1,
        io_count' = io_count,
        step_count' = step_count + 1,
      }
    else if (procs.get(target).status == Blocked and procs.get(target).block_reason == BkReceive)
      all {
        stepOk,
        procs.get(sender).status == Runnable,
        atHead(runqueue, sender),
        target.in(knownPids),
        msg_count < MaxMessages,
        procs' = procs.set(target, clearBlock(withStatus(procs.get(target), Runnable))),
        runqueue' = runqueue.tail().concat(List(target, sender)),
        mailboxes' = mailboxes,
        waiters' = waiters,
        io_pending' = io_pending,
        next_pid' = next_pid,
        io_next_token' = io_next_token,
        msg_count' = msg_count + 1,
        io_count' = io_count,
        step_count' = step_count + 1,
      }
    else
      all {
        stepOk,
        procs.get(sender).status == Runnable,
        atHead(runqueue, sender),
        target.in(knownPids),
        msg_count < MaxMessages,
        procs' = procs,
        runqueue' = runqueue.tail().append(sender),
        mailboxes' = mailboxes.set(target, mailboxes.get(target).append(mailboxMessage(sender))),
        waiters' = waiters,
        io_pending' = io_pending,
        next_pid' = next_pid,
        io_next_token' = io_next_token,
        msg_count' = msg_count + 1,
        io_count' = io_count,
        step_count' = step_count + 1,
      }

  action sendUnknown(sender, target_pid) = all {
    stepOk,
    procs.get(sender).status == Runnable,
    atHead(runqueue, sender),
    not(target_pid.in(knownPids)),
    target_pid.in(1.to(MaxProcesses)),
    procs' = procs.set(sender, withStatus(procs.get(sender), Failed)),
    runqueue' = runqueue.tail(),
    mailboxes' = mailboxes,
    waiters' = waiters,
    io_pending' = io_pending,
    next_pid' = next_pid,
    io_next_token' = io_next_token,
    msg_count' = msg_count,
    io_count' = io_count,
    step_count' = step_count + 1,
  }

  action receiveReady(pid) = all {
    stepOk,
    procs.get(pid).status == Runnable,
    atHead(runqueue, pid),
    mailboxes.get(pid).length() > 0,
    procs' = procs,
    runqueue' = runqueue.tail().append(pid),
    mailboxes' = mailboxes.set(pid, mailboxes.get(pid).tail()),
    waiters' = waiters,
    io_pending' = io_pending,
    next_pid' = next_pid,
    io_next_token' = io_next_token,
    msg_count' = msg_count,
    io_count' = io_count,
    step_count' = step_count + 1,
  }

  action receiveBlock(pid) = all {
    stepOk,
    procs.get(pid).status == Runnable,
    atHead(runqueue, pid),
    mailboxes.get(pid).length() == 0,
    procs' = procs.set(pid, blockOn(procs.get(pid), BkReceive, 0)),
    runqueue' = runqueue.tail(),
    mailboxes' = mailboxes,
    waiters' = waiters,
    io_pending' = io_pending,
    next_pid' = next_pid,
    io_next_token' = io_next_token,
    msg_count' = msg_count,
    io_count' = io_count,
    step_count' = step_count + 1,
  }

  action detectDeadlock(pid) = all {
    stepOk,
    procs.get(pid).status == Blocked,
    runqueue.length() == 0,
    io_pending.size() == 0,
    (
      procs.get(pid).block_reason == BkReceive or
      (
        procs.get(pid).block_reason == BkAwait and
        procs.get(procs.get(pid).block_target).status.in(Set(Blocked, Failed))
      )
    ),
    procs' = procs.set(pid, clearBlock(withStatus(procs.get(pid), Failed))),
    runqueue' = runqueue,
    mailboxes' = mailboxes,
    waiters' =
      if (procs.get(pid).block_reason == BkAwait)
        waiters.set(
          procs.get(pid).block_target,
          waiters.get(procs.get(pid).block_target).exclude(Set(pid))
        )
      else
        waiters,
    io_pending' = io_pending,
    next_pid' = next_pid,
    io_next_token' = io_next_token,
    msg_count' = msg_count,
    io_count' = io_count,
    step_count' = step_count + 1,
  }

  action spawnAny = { nondet pid = knownPids.oneOf(); spawn(pid) }
  action completeAny = { nondet pid = knownPids.oneOf(); complete(pid) }
  action completeBadResultAny = { nondet pid = knownPids.oneOf(); completeBadResult(pid) }
  action failAny = { nondet pid = knownPids.oneOf(); failProc(pid) }
  action yieldAny = { nondet pid = knownPids.oneOf(); yield(pid) }
  action requestIoAny = { nondet pid = knownPids.oneOf(); requestIo(pid) }
  action receiveReadyAny = { nondet pid = knownPids.oneOf(); receiveReady(pid) }
  action receiveBlockAny = { nondet pid = knownPids.oneOf(); receiveBlock(pid) }
  action detectDeadlockAny = { nondet pid = knownPids.oneOf(); detectDeadlock(pid) }
  action awaitDoneAny = { nondet parent = knownPids.oneOf(); nondet child = knownPids.oneOf(); awaitDone(parent, child) }
  action awaitConsumedAny = { nondet parent = knownPids.oneOf(); nondet child = knownPids.oneOf(); awaitConsumed(parent, child) }
  action awaitFailedAny = { nondet parent = knownPids.oneOf(); nondet child = knownPids.oneOf(); awaitFailed(parent, child) }
  action awaitBlockAny = { nondet parent = knownPids.oneOf(); nondet child = knownPids.oneOf(); awaitBlock(parent, child) }
  action awaitNotChildAny = { nondet parent = knownPids.oneOf(); nondet child = knownPids.oneOf(); awaitNotChild(parent, child) }
  action sendAny = { nondet sender = knownPids.oneOf(); nondet target = knownPids.oneOf(); send(sender, target) }
  action ioCompleteAny = { nondet entry = ioEntryUniverse.oneOf(); ioComplete(entry) }
  action ioFailAny = { nondet entry = ioEntryUniverse.oneOf(); ioFail(entry) }
  action awaitUnknownAny = { nondet parent = knownPids.oneOf(); nondet child_pid = pidUniverse.oneOf(); awaitUnknown(parent, child_pid) }
  action sendUnknownAny = { nondet sender = knownPids.oneOf(); nondet target_pid = pidUniverse.oneOf(); sendUnknown(sender, target_pid) }

  action step = any {
    spawnAny,
    completeAny,
    completeBadResultAny,
    failAny,
    yieldAny,
    requestIoAny,
    receiveReadyAny,
    receiveBlockAny,
    detectDeadlockAny,
    awaitDoneAny,
    awaitConsumedAny,
    awaitFailedAny,
    awaitBlockAny,
    awaitNotChildAny,
    sendAny,
    ioCompleteAny,
    ioFailAny,
    awaitUnknownAny,
    sendUnknownAny,
  }

  val typeOk =
    next_pid >= 0 and
    io_next_token >= 0 and
    msg_count >= 0 and
    io_count >= 0 and
    step_count >= 0 and
    knownPids.forall(p =>
      procs.get(p).status.in(Set(Runnable, Blocked, Done, Failed)) and
      (procs.get(p).parent == 0 or procs.get(p).parent.in(knownPids)) and
      procs.get(p).delivered_to >= 0 and
      (procs.get(p).delivered_to == 0 or procs.get(p).delivered_to.in(knownPids)) and
      procs.get(p).block_reason.in(Set(BkNone, BkAwait, BkIo, BkReceive)) and
      procs.get(p).block_target >= 0
    )

  val resultDeliveredToParentOnly =
    knownPids.forall(p =>
      procs.get(p).delivered_to == 0 or procs.get(p).delivered_to == procs.get(p).parent
    )

  val resultDeliveredAtMostOnce =
    knownPids.forall(p =>
      not(procs.get(p).status == Done and procs.get(p).delivered_to != 0) or procs.get(p).result == -1
    )

  val runnableInQueue =
    knownPids.forall(p =>
      procs.get(p).status != Runnable or inRunqueue(runqueue, p)
    )

  val blockedNotInQueue =
    knownPids.forall(p =>
      not(procs.get(p).status.in(Set(Blocked, Done, Failed))) or not(inRunqueue(runqueue, p))
    )

  val blockedIsConsistent =
    knownPids.forall(p =>
      (
        procs.get(p).status == Blocked implies procs.get(p).block_reason.in(Set(BkAwait, BkIo, BkReceive))
      ) and
      (
        procs.get(p).status != Blocked implies procs.get(p).block_reason == BkNone
      )
    )

  val onlyParentAwaits =
    knownPids.forall(p =>
      not(procs.get(p).status == Blocked and procs.get(p).block_reason == BkAwait) or
      (
        procs.get(p).block_target.in(knownPids) and
        procs.get(procs.get(p).block_target).parent == p
      )
    )

  val noCircularAwait =
    knownPids.forall(p =>
      knownPids.forall(q =>
        not(
          procs.get(p).status == Blocked and
          procs.get(p).block_reason == BkAwait and
          procs.get(p).block_target == q and
          procs.get(q).status == Blocked and
          procs.get(q).block_reason == BkAwait and
          procs.get(q).block_target == p
        )
      )
    )

  val ioBlockedHasPending =
    knownPids.forall(p =>
      not(procs.get(p).status == Blocked and procs.get(p).block_reason == BkIo) or
      io_pending.exists(entry => entry.pid == p and entry.token == procs.get(p).block_target)
    )

  val noOrphanedIo =
    io_pending.forall(entry =>
      entry.pid.in(knownPids) and
      procs.get(entry.pid).status == Blocked and
      procs.get(entry.pid).block_reason == BkIo
    )

  val mailboxExists = mailboxes.keys() == knownPids
  val waitersExists = waiters.keys() == knownPids

  val waitersConsistent =
    waiters.keys().forall(child =>
      waiters.get(child).forall(parent =>
        parent.in(knownPids) and
        procs.get(parent).status == Blocked and
        procs.get(parent).block_reason == BkAwait and
        procs.get(parent).block_target == child
      )
    )

  val noSilentSerializationFailure =
    knownPids.forall(p =>
      procs.get(p).status != Done or procs.get(p).result != -1 or procs.get(p).delivered_to != 0
    )

  val blockedAwaitListed =
    knownPids.forall(parent =>
      not(procs.get(parent).status == Blocked and procs.get(parent).block_reason == BkAwait) or
      (
        procs.get(parent).block_target.in(waiters.keys()) and
        parent.in(waiters.get(procs.get(parent).block_target))
      )
    )

  val atMostOneWaiterPerChild =
    waiters.keys().forall(child => waiters.get(child).size() <= 1)

  val safetyInvariant = all {
    typeOk,
    resultDeliveredToParentOnly,
    resultDeliveredAtMostOnce,
    runnableInQueue,
    blockedNotInQueue,
    blockedIsConsistent,
    onlyParentAwaits,
    noCircularAwait,
    ioBlockedHasPending,
    noOrphanedIo,
    mailboxExists,
    waitersExists,
    waitersConsistent,
    blockedAwaitListed,
    atMostOneWaiterPerChild,
    noSilentSerializationFailure,
  }

  val stateView = {
    procs: procs,
    runqueue: runqueue,
    mailboxes: mailboxes,
    waiters: waiters,
    io_pending: io_pending,
    next_pid: next_pid,
    io_next_token: io_next_token,
    msg_count: msg_count,
    io_count: io_count,
    step_count: step_count,
  }

  def parentWaitingOnTerminalChild(parent, child) =
    if (parent.in(knownPids) and child.in(knownPids))
      procs.get(parent).status == Blocked and
      procs.get(parent).block_reason == BkAwait and
      procs.get(parent).block_target == child and
      procs.get(child).status.in(Set(Done, Failed))
    else false

  def parentUnblockedOrGone(parent) =
    if (parent.in(knownPids))
      procs.get(parent).status != Blocked
    else true

  temporal fairness =
    pidUniverse.forall(pid => detectDeadlock(pid).weakFair(stateView)) and
    ioEntryUniverse.forall(entry => any { ioComplete(entry), ioFail(entry) }.weakFair(stateView))

  temporal spec =
    always(step.orKeep(stateView)) and
    fairness

  temporal parentEventuallyWoken =
    pidUniverse.forall(child =>
      pidUniverse.forall(parent =>
        parentWaitingOnTerminalChild(parent, child) leadsTo parentUnblockedOrGone(parent)
      )
    )

  temporal ioEventuallyCompletes =
    ioEntryUniverse.forall(entry =>
      entry.in(io_pending) leadsTo not(entry.in(io_pending))
    )

  temporal liveParentEventuallyWoken = spec.implies(parentEventuallyWoken)
  temporal liveIoEventuallyCompletes = spec.implies(ioEventuallyCompletes)
}
