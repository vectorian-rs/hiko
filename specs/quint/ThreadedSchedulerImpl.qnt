module ThreadedSchedulerImpl {
  /// Literal Quint port of `specs/tla/ThreadedSchedulerImpl.tla`.

  type ProcStatus = Runnable | Running | BlockedAwait | BlockedReceive | BlockedIo | Done | Failed
  type Proc = { status: ProcStatus, parent: int, target: int }
  type IoEntry = { pid: int, token: int }
  type WorkerId = W1 | W2

  pure val MaxProcesses = 4
  pure val MaxSteps = 10
  pure val MaxIoOps = 2
  pure val pidUniverse = 1.to(MaxProcesses)
  pure val tokenUniverse = 1.to(MaxIoOps)
  pure val workerUniverse = Set(W1, W2)
  pure val ioEntryUniverse =
    pidUniverse.fold(
      Set(),
      (entries, pid) => entries.union(tokenUniverse.map(token => { pid: pid, token: token }))
    )

  var procs: int -> Proc
  var queue: List[int]
  var workers: WorkerId -> int
  var waiters: int -> Set[int]
  var io_waiters: Set[IoEntry]
  var next_pid: int
  var next_token: int
  var shutdown: bool
  var step_count: int

  pure def emptyProc(parent): (int) => Proc =
    { status: Runnable, parent: parent, target: 0 }

  pure def withStatus(proc, status): (Proc, ProcStatus) => Proc =
    proc.with("status", status)

  pure def withTarget(proc, target): (Proc, int) => Proc =
    proc.with("target", target)

  pure def clearTarget(proc): (Proc) => Proc =
    proc.with("target", 0)

  val knownPids = procs.keys()
  val stepOk = step_count < MaxSteps

  pure def atHead(queue, pid): (List[int], int) => bool =
    queue.length() > 0 and queue.head() == pid

  pure def idle(workers, w): ((WorkerId -> int), WorkerId) => bool =
    workers.get(w) == 0

  val stuckPids =
    knownPids.filter(p =>
      procs.get(p).status == BlockedReceive or
      (
        procs.get(p).status == BlockedAwait and
        procs.get(p).target.in(knownPids) and
        procs.get(procs.get(p).target).status.in(Set(BlockedAwait, BlockedReceive, Failed))
      )
    )

  pure def heldBy(workers, pid): ((WorkerId -> int), int) => Set[WorkerId] =
    workerUniverse.filter(w => workers.get(w) == pid)

  pure def isHeld(workers, pid): ((WorkerId -> int), int) => bool =
    heldBy(workers, pid).size() > 0

  action init = all {
    procs' = Set(1).mapBy(_ => emptyProc(0)),
    queue' = List(1),
    workers' = workerUniverse.mapBy(_ => 0),
    waiters' = Set(1).mapBy(_ => Set()),
    io_waiters' = Set(),
    next_pid' = 2,
    next_token' = 1,
    shutdown' = false,
    step_count' = 0,
  }

  action disabled = all {
    procs' = procs,
    queue' = queue,
    workers' = workers,
    waiters' = waiters,
    io_waiters' = io_waiters,
    next_pid' = next_pid,
    next_token' = next_token,
    shutdown' = shutdown,
    step_count' = step_count,
    false,
  }

  action dequeueRunnable(w, pid) =
    if (pid.in(knownPids))
      all {
        stepOk,
        not(shutdown),
        idle(workers, w),
        atHead(queue, pid),
        procs.get(pid).status == Runnable,
        procs' = procs.set(pid, withStatus(procs.get(pid), Running)),
        queue' = queue.tail(),
        workers' = workers.set(w, pid),
        waiters' = waiters,
        io_waiters' = io_waiters,
        next_pid' = next_pid,
        next_token' = next_token,
        shutdown' = shutdown,
        step_count' = step_count + 1,
      }
    else disabled

  action dequeueStale(w, pid) =
    if (pid.in(knownPids))
      all {
        stepOk,
        not(shutdown),
        idle(workers, w),
        atHead(queue, pid),
        procs.get(pid).status != Runnable,
        procs' = procs,
        queue' = queue.tail(),
        workers' = workers,
        waiters' = waiters,
        io_waiters' = io_waiters,
        next_pid' = next_pid,
        next_token' = next_token,
        shutdown' = shutdown,
        step_count' = step_count + 1,
      }
    else disabled

  action workerYield(w) =
    if (workers.get(w).in(knownPids))
      all {
        stepOk,
        not(shutdown),
        procs.get(workers.get(w)).status == Running,
        procs' = procs.set(workers.get(w), withStatus(procs.get(workers.get(w)), Runnable)),
        queue' = queue.append(workers.get(w)),
        workers' = workers.set(w, 0),
        waiters' = waiters,
        io_waiters' = io_waiters,
        next_pid' = next_pid,
        next_token' = next_token,
        shutdown' = shutdown,
        step_count' = step_count + 1,
      }
    else disabled

  action workerSpawn(w) =
    if (workers.get(w).in(knownPids))
      all {
        stepOk,
        not(shutdown),
        next_pid <= MaxProcesses,
        procs.get(workers.get(w)).status == Running,
        procs' = procs
          .set(workers.get(w), withStatus(procs.get(workers.get(w)), Runnable))
          .put(next_pid, emptyProc(workers.get(w))),
        queue' = queue.concat(List(next_pid, workers.get(w))),
        workers' = workers.set(w, 0),
        waiters' = waiters.put(next_pid, Set()),
        io_waiters' = io_waiters,
        next_pid' = next_pid + 1,
        next_token' = next_token,
        shutdown' = shutdown,
        step_count' = step_count + 1,
      }
    else disabled

  action workerAwaitBlock(w, child) =
    if (workers.get(w).in(knownPids) and child.in(knownPids))
      all {
        stepOk,
        not(shutdown),
        procs.get(workers.get(w)).status == Running,
        procs.get(child).parent == workers.get(w),
        procs.get(child).status.in(Set(Runnable, Running, BlockedAwait, BlockedReceive, BlockedIo)),
        procs' = procs.set(
          workers.get(w),
          withTarget(withStatus(procs.get(workers.get(w)), BlockedAwait), child)
        ),
        queue' = queue,
        workers' = workers.set(w, 0),
        waiters' = waiters.set(child, waiters.get(child).union(Set(workers.get(w)))),
        io_waiters' = io_waiters,
        next_pid' = next_pid,
        next_token' = next_token,
        shutdown' = shutdown,
        step_count' = step_count + 1,
      }
    else disabled

  action workerBlockReceive(w) =
    if (workers.get(w).in(knownPids))
      all {
        stepOk,
        not(shutdown),
        procs.get(workers.get(w)).status == Running,
        procs' = procs.set(workers.get(w), withStatus(procs.get(workers.get(w)), BlockedReceive)),
        queue' = queue,
        workers' = workers.set(w, 0),
        waiters' = waiters,
        io_waiters' = io_waiters,
        next_pid' = next_pid,
        next_token' = next_token,
        shutdown' = shutdown,
        step_count' = step_count + 1,
      }
    else disabled

  action workerRequestIo(w) =
    if (workers.get(w).in(knownPids))
      all {
        stepOk,
        not(shutdown),
        next_token <= MaxIoOps,
        procs.get(workers.get(w)).status == Running,
        procs' = procs.set(
          workers.get(w),
          withTarget(withStatus(procs.get(workers.get(w)), BlockedIo), next_token)
        ),
        queue' = queue,
        workers' = workers.set(w, 0),
        waiters' = waiters,
        io_waiters' = io_waiters.union(Set({ pid: workers.get(w), token: next_token })),
        next_pid' = next_pid,
        next_token' = next_token + 1,
        shutdown' = shutdown,
        step_count' = step_count + 1,
      }
    else disabled

  action workerDone(w) =
    if (workers.get(w).in(knownPids) and waiters.get(workers.get(w)).size() == 0)
      all {
        stepOk,
        not(shutdown),
        procs.get(workers.get(w)).status == Running,
        procs' = procs.set(workers.get(w), withStatus(procs.get(workers.get(w)), Done)),
        queue' = queue,
        workers' = workers.set(w, 0),
        waiters' = waiters,
        io_waiters' = io_waiters,
        next_pid' = next_pid,
        next_token' = next_token,
        shutdown' = shutdown,
        step_count' = step_count + 1,
      }
    else if (workers.get(w).in(knownPids))
      {
        nondet parent = waiters.get(workers.get(w)).oneOf()
        all {
          stepOk,
          not(shutdown),
          procs.get(workers.get(w)).status == Running,
          procs' = procs
            .set(workers.get(w), withStatus(procs.get(workers.get(w)), Done))
            .set(parent, clearTarget(withStatus(procs.get(parent), Runnable))),
          queue' = queue.append(parent),
          workers' = workers.set(w, 0),
          waiters' = waiters.set(workers.get(w), Set()),
          io_waiters' = io_waiters,
          next_pid' = next_pid,
          next_token' = next_token,
          shutdown' = shutdown,
          step_count' = step_count + 1,
        }
      }
    else disabled

  action workerFail(w) =
    if (workers.get(w).in(knownPids) and waiters.get(workers.get(w)).size() == 0)
      all {
        stepOk,
        not(shutdown),
        procs.get(workers.get(w)).status == Running,
        procs' = procs.set(workers.get(w), withStatus(procs.get(workers.get(w)), Failed)),
        queue' = queue,
        workers' = workers.set(w, 0),
        waiters' = waiters,
        io_waiters' = io_waiters,
        next_pid' = next_pid,
        next_token' = next_token,
        shutdown' = shutdown,
        step_count' = step_count + 1,
      }
    else if (workers.get(w).in(knownPids))
      {
        nondet parent = waiters.get(workers.get(w)).oneOf()
        all {
          stepOk,
          not(shutdown),
          procs.get(workers.get(w)).status == Running,
          procs' = procs
            .set(workers.get(w), withStatus(procs.get(workers.get(w)), Failed))
            .set(parent, clearTarget(withStatus(procs.get(parent), Failed))),
          queue' = queue,
          workers' = workers.set(w, 0),
          waiters' = waiters.set(workers.get(w), Set()),
          io_waiters' = io_waiters,
          next_pid' = next_pid,
          next_token' = next_token,
          shutdown' = shutdown,
          step_count' = step_count + 1,
        }
      }
    else disabled

  action monitorIoComplete(entry) =
    if (entry.in(io_waiters) and entry.pid.in(knownPids))
      all {
        stepOk,
        not(shutdown),
        procs.get(entry.pid).status == BlockedIo,
        procs.get(entry.pid).target == entry.token,
        procs' = procs.set(entry.pid, clearTarget(withStatus(procs.get(entry.pid), Runnable))),
        queue' = queue.append(entry.pid),
        workers' = workers,
        waiters' = waiters,
        io_waiters' = io_waiters.exclude(Set(entry)),
        next_pid' = next_pid,
        next_token' = next_token,
        shutdown' = shutdown,
        step_count' = step_count + 1,
      }
    else disabled

  action monitorIoFail(entry) =
    if (entry.in(io_waiters) and entry.pid.in(knownPids) and waiters.get(entry.pid).size() == 0)
      all {
        stepOk,
        not(shutdown),
        procs.get(entry.pid).status == BlockedIo,
        procs.get(entry.pid).target == entry.token,
        procs' = procs.set(entry.pid, clearTarget(withStatus(procs.get(entry.pid), Failed))),
        queue' = queue,
        workers' = workers,
        waiters' = waiters,
        io_waiters' = io_waiters.exclude(Set(entry)),
        next_pid' = next_pid,
        next_token' = next_token,
        shutdown' = shutdown,
        step_count' = step_count + 1,
      }
    else if (entry.in(io_waiters) and entry.pid.in(knownPids))
      {
        nondet parent = waiters.get(entry.pid).oneOf()
        all {
          stepOk,
          not(shutdown),
          procs.get(entry.pid).status == BlockedIo,
          procs.get(entry.pid).target == entry.token,
          procs' = procs
            .set(entry.pid, clearTarget(withStatus(procs.get(entry.pid), Failed)))
            .set(parent, clearTarget(withStatus(procs.get(parent), Failed))),
          queue' = queue,
          workers' = workers,
          waiters' = waiters.set(entry.pid, Set()),
          io_waiters' = io_waiters.exclude(Set(entry)),
          next_pid' = next_pid,
          next_token' = next_token,
          shutdown' = shutdown,
          step_count' = step_count + 1,
        }
      }
    else disabled

  action monitorDetectDeadlock = all {
    stepOk,
    not(shutdown),
    queue.length() == 0,
    io_waiters.size() == 0,
    workerUniverse.forall(w => idle(workers, w)),
    stuckPids.size() > 0,
    procs' = knownPids.mapBy(p =>
      if (p.in(stuckPids))
        clearTarget(withStatus(procs.get(p), Failed))
      else
        procs.get(p)
    ),
    queue' = queue,
    workers' = workers,
    waiters' = knownPids.mapBy(child =>
      knownPids.filter(parent =>
        not(parent.in(stuckPids)) and
        procs.get(parent).status == BlockedAwait and
        procs.get(parent).target == child
      )
    ),
    io_waiters' = io_waiters,
    next_pid' = next_pid,
    next_token' = next_token,
    shutdown' = true,
    step_count' = step_count + 1,
  }

  action dequeueRunnableAny = { nondet w = workerUniverse.oneOf(); nondet pid = pidUniverse.oneOf(); dequeueRunnable(w, pid) }
  action dequeueStaleAny = { nondet w = workerUniverse.oneOf(); nondet pid = pidUniverse.oneOf(); dequeueStale(w, pid) }
  action workerYieldAny = { nondet w = workerUniverse.oneOf(); workerYield(w) }
  action workerSpawnAny = { nondet w = workerUniverse.oneOf(); workerSpawn(w) }
  action workerAwaitBlockAny = { nondet w = workerUniverse.oneOf(); nondet child = pidUniverse.oneOf(); workerAwaitBlock(w, child) }
  action workerBlockReceiveAny = { nondet w = workerUniverse.oneOf(); workerBlockReceive(w) }
  action workerRequestIoAny = { nondet w = workerUniverse.oneOf(); workerRequestIo(w) }
  action workerDoneAny = { nondet w = workerUniverse.oneOf(); workerDone(w) }
  action workerFailAny = { nondet w = workerUniverse.oneOf(); workerFail(w) }
  action monitorIoCompleteAny = { nondet entry = ioEntryUniverse.oneOf(); monitorIoComplete(entry) }
  action monitorIoFailAny = { nondet entry = ioEntryUniverse.oneOf(); monitorIoFail(entry) }

  action step = any {
    dequeueRunnableAny,
    dequeueStaleAny,
    workerYieldAny,
    workerSpawnAny,
    workerAwaitBlockAny,
    workerBlockReceiveAny,
    workerRequestIoAny,
    workerDoneAny,
    workerFailAny,
    monitorIoCompleteAny,
    monitorIoFailAny,
    monitorDetectDeadlock,
  }

  val typeOk =
    next_pid >= 0 and
    next_token >= 0 and
    step_count >= 0 and
    shutdown.in(Bool) and
    knownPids.forall(p =>
      procs.get(p).status.in(Set(Runnable, Running, BlockedAwait, BlockedReceive, BlockedIo, Done, Failed)) and
      (procs.get(p).parent == 0 or procs.get(p).parent.in(knownPids)) and
      procs.get(p).target >= 0
    ) and
    workerUniverse.forall(w => workers.get(w) == 0 or workers.get(w).in(knownPids))

  val queueKnownPids =
    queue.indices().forall(i => queue.nth(i).in(knownPids))

  val runningHeldByExactlyOneWorker =
    knownPids.forall(p =>
      procs.get(p).status != Running or heldBy(workers, p).size() == 1
    )

  val onlyHeldProcessesAreRunning =
    workerUniverse.forall(w =>
      workers.get(w) == 0 or procs.get(workers.get(w)).status == Running
    )

  val blockedNeverHeld =
    knownPids.forall(p =>
      not(procs.get(p).status.in(Set(BlockedAwait, BlockedReceive, BlockedIo, Done, Failed))) or
      not(isHeld(workers, p))
    )

  val atMostOneWaiterPerChild =
    waiters.keys().forall(child => waiters.get(child).size() <= 1)

  val waitersConsistent =
    waiters.keys().forall(child =>
      waiters.get(child).forall(parent =>
        parent.in(knownPids) and
        procs.get(parent).status == BlockedAwait and
        procs.get(parent).target == child
      )
    )

  val ioWaitersConsistent =
    io_waiters.forall(entry =>
      entry.pid.in(knownPids) and
      procs.get(entry.pid).status == BlockedIo and
      procs.get(entry.pid).target == entry.token
    )

  val shutdownImpliesNoRunnableOrRunning =
    not(shutdown) or (
      queue.length() == 0 and
      io_waiters.size() == 0 and
      workerUniverse.forall(w => idle(workers, w)) and
      knownPids.forall(p => not(procs.get(p).status.in(Set(Runnable, Running))))
    )

  val safetyInvariant = all {
    typeOk,
    queueKnownPids,
    runningHeldByExactlyOneWorker,
    onlyHeldProcessesAreRunning,
    blockedNeverHeld,
    atMostOneWaiterPerChild,
    waitersConsistent,
    ioWaitersConsistent,
    shutdownImpliesNoRunnableOrRunning,
  }

  val stateView = {
    procs: procs,
    queue: queue,
    workers: workers,
    waiters: waiters,
    io_waiters: io_waiters,
    next_pid: next_pid,
    next_token: next_token,
    shutdown: shutdown,
    step_count: step_count,
  }

  def implParentWaitingOnTerminalChild(parent, child) =
    if (parent.in(knownPids) and child.in(knownPids))
      procs.get(child).status.in(Set(Done, Failed)) and
      parent.in(waiters.get(child))
    else false

  def implParentUnblockedOrFailed(parent) =
    if (parent.in(knownPids))
      procs.get(parent).status.in(Set(Runnable, Running, Failed))
    else true

  temporal fairness =
    workerUniverse.forall(w =>
      workerDone(w).weakFair(stateView) and
      workerFail(w).weakFair(stateView)
    ) and
    ioEntryUniverse.forall(entry => monitorIoComplete(entry).weakFair(stateView)) and
    monitorDetectDeadlock.weakFair(stateView)

  temporal spec =
    always(step.orKeep(stateView)) and
    fairness

  temporal implParentEventuallyWoken =
    pidUniverse.forall(child =>
      pidUniverse.forall(parent =>
        implParentWaitingOnTerminalChild(parent, child) leadsTo implParentUnblockedOrFailed(parent)
      )
    )

  temporal implIoEventuallyCompletes =
    ioEntryUniverse.forall(entry =>
      entry.in(io_waiters) leadsTo not(entry.in(io_waiters))
    )

  temporal liveImplParentEventuallyWoken = spec.implies(implParentEventuallyWoken)
  temporal liveImplIoEventuallyCompletes = spec.implies(implIoEventuallyCompletes)
}
