//! Owned bulk-join state. Runtimes own scheduling; this module owns admission,
//! ordered results, logical storage credits and the bounded completion inbox.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use super::ChildState;
use crate::budget::SharedBudget;
use crate::heap::Heap;
use crate::process::{ChildOutcome, FiberJoinError, Pid, ProcessFailure};
use crate::sendable::SendableValue;
use crate::sync::Mutex;
use crate::value::{HeapObject, Value};
use crate::vm::{TAG_CONS, TAG_NIL, VM};

#[cfg(all(test, feature = "builtin-process"))]
mod runtime_tests;
#[cfg(test)]
mod tests;

pub const MAX_JOIN_GROUP_ENTRIES: usize = 10_000;
const GROUP_BYTES: u64 = 512;
const ENTRY_BYTES: u64 = 256;
const NODE_BYTES: u64 = std::mem::size_of::<SendableValue>() as u64;

#[derive(Debug)]
struct Credits {
    budget: Arc<SharedBudget>,
    held: u64,
}

impl Credits {
    fn add(&mut self, bytes: u64) -> Result<(), String> {
        self.budget.try_charge(bytes).map_err(|_| {
            format!(
                "fiber_await_all: max_join_group_bytes ({}) exceeded",
                self.budget.limit()
            )
        })?;
        self.held += bytes;
        Ok(())
    }

    fn release(&mut self, bytes: u64) {
        debug_assert!(bytes <= self.held);
        self.held -= bytes;
        self.budget.refund(bytes);
    }
}

impl Drop for Credits {
    fn drop(&mut self) {
        self.budget.refund(self.held);
    }
}

/// An admitted, bounded pid buffer and its reserved metadata credits. Moving
/// this request through the VM boundary never releases or duplicates credits.
#[derive(Debug)]
pub struct GroupAdmission {
    pids: Vec<Pid>,
    credits: Credits,
}

impl GroupAdmission {
    pub(crate) fn read(vm: &mut VM, list: Value) -> Result<Self, String> {
        // One bounded pass. Reserve each logical entry before growing its
        // buffer; geometric native growth remains amortized O(n). Partial
        // quota/allocation rejection drops the exact lease, consuming no child.
        let mut credits = Credits {
            budget: vm.join_group_budget().clone(),
            held: 0,
        };
        let mut pids = Vec::new();
        let mut cursor = list;
        while let Some((pid, tail)) = next_pid(vm, cursor)? {
            if pids.len() == MAX_JOIN_GROUP_ENTRIES {
                return Err("fiber_await_all: at most 10000 entries allowed".into());
            }
            if pids.is_empty() {
                credits.add(GROUP_BYTES)?;
            }
            credits.add(ENTRY_BYTES)?;
            pids.try_reserve(1)
                .map_err(|e| format!("fiber_await_all: admission allocation: {e}"))?;
            pids.push(pid);
            cursor = tail;
        }
        // Pay classification/registration work before any outcome is consumed.
        vm.heap.charge_host_work(pids.len() as u64 * 4)?;
        Ok(Self { pids, credits })
    }
}

fn next_pid(vm: &mut VM, cursor: Value) -> Result<Option<(Pid, Value)>, String> {
    vm.heap.charge_host_work(1)?;
    let Value::Heap(reference) = cursor else {
        return Err("fiber_await_all: expected pid list".into());
    };
    match vm.heap.get(reference).map_err(|e| e.to_string())? {
        HeapObject::Data { tag, fields } if *tag == TAG_NIL && fields.is_empty() => Ok(None),
        HeapObject::Data { tag, fields } if *tag == TAG_CONS && fields.len() == 2 => {
            match fields[0] {
                Value::Pid(pid) => Ok(Some((Pid(pid), fields[1]))),
                _ => Err("fiber_await_all: expected pid list".into()),
            }
        }
        _ => Err("fiber_await_all: expected pid list".into()),
    }
}

/// One registered child publishes exactly one event into pre-reserved space.
/// Locks cover queue operations only, never runtime/backend callbacks.
#[derive(Debug)]
pub(crate) struct GroupInbox {
    pub parent: Pid,
    pub id: u64,
    events: Mutex<VecDeque<Pid>>,
    pub members: Vec<Pid>,
}

impl GroupInbox {
    pub fn publish(&self, pid: Pid) {
        self.events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(pid);
    }

    pub fn pop(&self) -> Option<Pid> {
        self.events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pop_front()
    }

    pub fn has_events(&self) -> bool {
        !self
            .events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty()
    }
}

#[derive(Debug)]
struct Member {
    pid: Pid,
    done: bool,
    value: Option<SendableValue>,
    bytes: u64,
}

#[derive(Debug)]
pub(crate) enum GroupCause {
    Typed(FiberJoinError),
    Fatal(ProcessFailure),
}

#[derive(Debug)]
pub(crate) struct JoinGroup {
    pub inbox: Arc<GroupInbox>,
    members: Vec<Member>,
    index: HashMap<Pid, usize>,
    pending: usize,
    credits: Credits,
    pub cause: Option<GroupCause>,
    cleanup_started: bool,
    aggregate: u64,
    max_message_bytes: u64,
    /// Counts actual pending-member visits and completion-index lookups, not
    /// elapsed time or a proxy for all runtime CPU work.
    #[cfg(test)]
    member_work: std::sync::atomic::AtomicU64,
}

impl JoinGroup {
    pub fn admit(
        admission: GroupAdmission,
        parent: Pid,
        id: u64,
        max_message_bytes: u64,
        mut classify: impl FnMut(Pid) -> ChildState,
    ) -> Result<Self, ProcessFailure> {
        let GroupAdmission { pids, credits } = admission;
        let mut members = Vec::new();
        let mut index = HashMap::new();
        let mut events = VecDeque::new();
        let mut named = Vec::new();
        members
            .try_reserve_exact(pids.len())
            .map_err(allocation_failure)?;
        index.try_reserve(pids.len()).map_err(allocation_failure)?;
        events
            .try_reserve_exact(pids.len())
            .map_err(allocation_failure)?;
        named
            .try_reserve_exact(pids.len())
            .map_err(allocation_failure)?;
        let mut first_error = None;
        for pid in pids {
            let state = classify(pid);
            if index.contains_key(&pid) {
                first_error.get_or_insert(FiberJoinError::AlreadyJoined);
                continue;
            }
            match state {
                ChildState::Live | ChildState::Terminated => {
                    index.insert(pid, members.len());
                    named.push(pid);
                    members.push(Member {
                        pid,
                        done: false,
                        value: None,
                        bytes: 0,
                    });
                }
                ChildState::Reaped if first_error.is_none() => {
                    first_error = Some(FiberJoinError::AlreadyJoined);
                }
                ChildState::NotChild if first_error.is_none() => {
                    first_error = Some(FiberJoinError::RuntimeError(format!(
                        "fiber_await_all: process {pid:?} is not a child of {parent:?}"
                    )));
                }
                ChildState::Unknown if first_error.is_none() => {
                    first_error = Some(FiberJoinError::RuntimeError(format!(
                        "fiber_await_all: unknown process {pid:?}"
                    )));
                }
                _ => {}
            }
        }
        let mut group = Self {
            inbox: Arc::new(GroupInbox {
                parent,
                id,
                events: Mutex::new(events),
                members: named,
            }),
            pending: members.len(),
            members,
            index,
            credits,
            cause: None,
            cleanup_started: false,
            aggregate: 2 * NODE_BYTES,
            max_message_bytes,
            #[cfg(test)]
            member_work: std::sync::atomic::AtomicU64::new(0),
        };
        if let Some(error) = first_error {
            group.choose_typed(error);
        }
        // Empty lists still have a list + protected Ok envelope.
        if group.cause.is_none() && group.aggregate > max_message_bytes {
            group.cause = Some(GroupCause::Fatal(message_failure(max_message_bytes)));
        }
        Ok(group)
    }

    pub fn pending_pids(&self) -> impl Iterator<Item = Pid> + '_ {
        self.members
            .iter()
            .filter(|m| {
                #[cfg(test)]
                self.member_work
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                !m.done
            })
            .map(|m| m.pid)
    }

    #[cfg(test)]
    pub fn member_work(&self) -> u64 {
        self.member_work.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn is_pending(&self, pid: Pid) -> bool {
        self.index.get(&pid).is_some_and(|&i| !self.members[i].done)
    }

    pub fn ready(&self) -> bool {
        self.pending == 0
    }

    /// Called once when a cause first requires cleanup. At most one full pass.
    pub fn begin_cleanup(&mut self) -> bool {
        if self.cause.is_none() || self.cleanup_started {
            return false;
        }
        self.cleanup_started = true;
        for member in &mut self.members {
            member.value = None;
            self.credits.release(member.bytes);
            member.bytes = 0;
        }
        true
    }

    fn choose_typed(&mut self, error: FiberJoinError) {
        let bytes = join_error_bytes(&error);
        let failure = if bytes > self.max_message_bytes {
            Some(message_failure(self.max_message_bytes).to_string())
        } else {
            self.credits.add(bytes).err()
        };
        self.cause = Some(match failure {
            // Retention admission failed: do not retain or duplicate the
            // uncharged child string. This is a parent quota/transfer failure,
            // not an adopted typed cause. Once admitted, Typed is preserved.
            Some(message) => GroupCause::Fatal(ProcessFailure::runtime(message)),
            None => GroupCause::Typed(error),
        });
    }

    /// Consume a single notified completion, by index, without scanning peers.
    /// After a cause is chosen, draining performs no fresh budget/host admission.
    pub fn complete(&mut self, pid: Pid, outcome: ChildOutcome, heap: &mut Heap) {
        #[cfg(test)]
        self.member_work
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let Some(&i) = self.index.get(&pid) else {
            return;
        };
        if self.members[i].done {
            return;
        }
        self.members[i].done = true;
        self.pending -= 1;
        if self.cause.is_some() {
            return;
        }
        match outcome {
            ChildOutcome::Err(failure) => {
                self.choose_typed(FiberJoinError::from_process_failure(failure))
            }
            ChildOutcome::Ok(value) => {
                let measured = value.checked_footprint(heap).and_then(|bytes| {
                    let aggregate = self
                        .aggregate
                        .checked_add(bytes)
                        .filter(|n| *n <= self.max_message_bytes)
                        .ok_or_else(|| message_failure(self.max_message_bytes).to_string())?;
                    self.credits.add(bytes)?;
                    self.aggregate = aggregate;
                    Ok(bytes)
                });
                match measured {
                    Ok(bytes) => {
                        self.members[i].bytes = bytes;
                        self.members[i].value = Some(value);
                    }
                    Err(message) => {
                        self.cause = Some(GroupCause::Fatal(ProcessFailure::runtime(message)))
                    }
                }
            }
        }
    }

    /// Credits remain owned by `self` throughout non-collecting transfer.
    pub fn deliver(mut self, vm: &mut VM) -> Result<(), ProcessFailure> {
        match self.cause.take() {
            Some(GroupCause::Fatal(failure)) => Err(failure),
            Some(GroupCause::Typed(error)) => {
                let cause = error.to_string();
                super::deliver_join_result_to_parent(vm, Err(error)).map_err(|failure| {
                    ProcessFailure::runtime(format!("{cause}; transfer failed: {failure}"))
                })
            }
            None => {
                // Each deserialized value is rooted on the parent stack before
                // constructing the list. Heap::alloc does not collect here.
                let base = vm.stack.len();
                let result = (|| {
                    for member in &mut self.members {
                        let value = member.value.take().ok_or_else(|| {
                            ProcessFailure::runtime("fiber_await_all: missing result")
                        })?;
                        let value = super::deserialize_with_heap_limit(&value, &mut vm.heap)?;
                        vm.push_value(value);
                    }
                    let list =
                        super::alloc_data_value(&mut vm.heap, TAG_NIL, smallvec::smallvec![])?;
                    vm.push_value(list);
                    for i in (base..base + self.members.len()).rev() {
                        vm.heap
                            .charge_host_work(1)
                            .map_err(ProcessFailure::runtime)?;
                        let list = super::alloc_data_value(
                            &mut vm.heap,
                            TAG_CONS,
                            smallvec::smallvec![
                                vm.stack[i],
                                *vm.stack.last().ok_or_else(|| ProcessFailure::runtime(
                                    "fiber_await_all: missing list root"
                                ))?
                            ],
                        )?;
                        *vm.stack.last_mut().ok_or_else(|| {
                            ProcessFailure::runtime("fiber_await_all: missing list root")
                        })? = list;
                    }
                    super::alloc_data_value(
                        &mut vm.heap,
                        hiko_builtin_meta::PROCESS_AWAIT_RESULT_OK_TAG,
                        smallvec::smallvec![*vm.stack.last().ok_or_else(|| {
                            ProcessFailure::runtime("fiber_await_all: missing list root")
                        })?],
                    )
                })();
                vm.stack.truncate(base);
                let wrapped = result?;
                super::pop_await_placeholder(vm, "deliver bulk join")?;
                vm.push_value(wrapped);
                Ok(())
            }
        }
    }
}

fn allocation_failure(error: impl std::fmt::Display) -> ProcessFailure {
    ProcessFailure::runtime(format!("fiber_await_all: group allocation: {error}"))
}

fn message_failure(limit: u64) -> ProcessFailure {
    ProcessFailure::runtime(format!(
        "fiber_await_all: aggregate exceeds max_message_bytes ({limit} bytes)"
    ))
}

fn join_error_bytes(error: &FiberJoinError) -> u64 {
    match error {
        FiberJoinError::RuntimeError(message) => 3 * NODE_BYTES + message.len() as u64,
        FiberJoinError::HeapObjectLimitExceeded { .. } => 5 * NODE_BYTES + 16,
        _ => 2 * NODE_BYTES,
    }
}
