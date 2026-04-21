//! Shared process operation logic used by both single-threaded and multi-threaded runtimes.

use smallvec::smallvec;
use std::panic::{self, AssertUnwindSafe};

use crate::process::{FiberJoinError, Pid, ProcessFailure, ProcessStatus};
use crate::sendable::{SendableValue, deserialize, serialize};
use crate::value::{Fields, HeapObject, Value};
use crate::vm::VM;

/// Result of checking a child's state for an await operation.
pub enum ChildState {
    Done(SendableValue),
    Failed(ProcessFailure),
    Running,
    NotFound,
    NotChild,
}

/// Check a child process's state for an await operation.
pub fn check_child_state(
    child_status: Option<(&ProcessStatus, Option<Pid>)>,
    parent_pid: Pid,
    child_stack_top: Option<Value>,
    child_heap: Option<&crate::heap::Heap>,
) -> ChildState {
    match child_status {
        None => ChildState::NotFound,
        Some((_, parent)) if parent != Some(parent_pid) => ChildState::NotChild,
        Some((ProcessStatus::Done, _)) => {
            let val = child_stack_top.unwrap_or(Value::Unit);
            let heap = child_heap.unwrap();
            ChildState::Done(serialize(val, heap).unwrap_or(SendableValue::Unit))
        }
        Some((ProcessStatus::Failed(msg), _)) => ChildState::Failed(msg.clone()),
        Some(_) => ChildState::Running,
    }
}

/// Deliver a child's result to a waiting parent.
pub fn deliver_result_to_parent(
    parent_vm: &mut VM,
    sendable: SendableValue,
) -> Result<(), ProcessFailure> {
    parent_vm.stack.pop(); // remove placeholder
    let val = deserialize_with_heap_limit(sendable, &mut parent_vm.heap)?;
    parent_vm.push_value(val);
    Ok(())
}

const PROCESS_AWAIT_RESULT_OK_TAG: u16 = 0;
const PROCESS_AWAIT_RESULT_ERR_TAG: u16 = 1;

const PROCESS_JOIN_ERROR_RUNTIME_ERROR_TAG: u16 = 0;
const PROCESS_JOIN_ERROR_CANCELLED_TAG: u16 = 1;
const PROCESS_JOIN_ERROR_FUEL_EXHAUSTED_TAG: u16 = 2;
const PROCESS_JOIN_ERROR_ALREADY_JOINED_TAG: u16 = 3;
const PROCESS_JOIN_ERROR_HEAP_LIMIT_TAG: u16 = 4;

pub fn deliver_join_result_to_parent(
    parent_vm: &mut VM,
    result: Result<SendableValue, FiberJoinError>,
) -> Result<(), ProcessFailure> {
    parent_vm.stack.pop(); // remove placeholder
    let wrapped = match result {
        Ok(sendable) => {
            let value = deserialize_with_heap_limit(sendable, &mut parent_vm.heap)?;
            alloc_data_value(
                &mut parent_vm.heap,
                PROCESS_AWAIT_RESULT_OK_TAG,
                smallvec![value],
            )?
        }
        Err(error) => {
            let error_value = encode_join_error(&mut parent_vm.heap, error)?;
            alloc_data_value(
                &mut parent_vm.heap,
                PROCESS_AWAIT_RESULT_ERR_TAG,
                smallvec![error_value],
            )?
        }
    };
    parent_vm.push_value(wrapped);
    Ok(())
}

/// Deliver a runtime-managed pid result to a waiting parent.
pub fn deliver_pid_to_parent(parent_vm: &mut VM, pid: Pid) {
    parent_vm.stack.pop(); // remove placeholder
    parent_vm.push_value(Value::Pid(pid.0));
}

/// Create a child VM that inherits capabilities from the parent VM.
pub fn create_child_vm_from_parent(
    parent_vm: &VM,
    proto_idx: usize,
    captures: Vec<SendableValue>,
) -> Result<VM, ProcessFailure> {
    let mut child_vm = parent_vm.create_child();
    let child_captures: Vec<Value> = captures
        .into_iter()
        .map(|v| deserialize_with_heap_limit(v, &mut child_vm.heap))
        .collect::<Result<_, _>>()?;
    child_vm.setup_closure_call(proto_idx, &child_captures);
    Ok(child_vm)
}

/// Serialize the result value from a finished process.
pub fn serialize_result(vm: &VM) -> SendableValue {
    let val = vm.stack.last().copied().unwrap_or(Value::Unit);
    serialize(val, &vm.heap).unwrap_or(SendableValue::Unit)
}

/// Prepare the delivery payload for waiters of a finished process.
pub fn prepare_delivery(status: &ProcessStatus, vm: &VM) -> Result<SendableValue, ProcessFailure> {
    match status {
        ProcessStatus::Done => Ok(serialize_result(vm)),
        ProcessStatus::Failed(msg) => Err(msg.clone()),
        _ => Err(ProcessFailure::runtime("child not finished")),
    }
}

fn encode_join_error(
    heap: &mut crate::heap::Heap,
    error: FiberJoinError,
) -> Result<Value, ProcessFailure> {
    match error {
        FiberJoinError::RuntimeError(message) => {
            let message_value = alloc_string_value(heap, message)?;
            alloc_data_value(
                heap,
                PROCESS_JOIN_ERROR_RUNTIME_ERROR_TAG,
                smallvec![message_value],
            )
        }
        FiberJoinError::Cancelled => {
            alloc_data_value(heap, PROCESS_JOIN_ERROR_CANCELLED_TAG, smallvec![])
        }
        FiberJoinError::FuelExhausted => {
            alloc_data_value(heap, PROCESS_JOIN_ERROR_FUEL_EXHAUSTED_TAG, smallvec![])
        }
        FiberJoinError::AlreadyJoined => {
            alloc_data_value(heap, PROCESS_JOIN_ERROR_ALREADY_JOINED_TAG, smallvec![])
        }
        FiberJoinError::HeapObjectLimitExceeded { live, limit } => {
            let pair = alloc_tuple_value(
                heap,
                smallvec![Value::Int(live as i64), Value::Int(limit as i64)],
            )?;
            alloc_data_value(heap, PROCESS_JOIN_ERROR_HEAP_LIMIT_TAG, smallvec![pair])
        }
    }
}

fn alloc_string_value(heap: &mut crate::heap::Heap, text: String) -> Result<Value, ProcessFailure> {
    alloc_heap_value(heap, HeapObject::String(text))
}

fn alloc_tuple_value(
    heap: &mut crate::heap::Heap,
    fields: Fields,
) -> Result<Value, ProcessFailure> {
    alloc_heap_value(heap, HeapObject::Tuple(fields))
}

fn alloc_data_value(
    heap: &mut crate::heap::Heap,
    tag: u16,
    fields: Fields,
) -> Result<Value, ProcessFailure> {
    alloc_heap_value(heap, HeapObject::Data { tag, fields })
}

fn alloc_heap_value(
    heap: &mut crate::heap::Heap,
    object: HeapObject,
) -> Result<Value, ProcessFailure> {
    match panic::catch_unwind(AssertUnwindSafe(|| heap.alloc(object))) {
        Ok(reference) => Ok(Value::Heap(reference)),
        Err(payload) => {
            if let Some(failure) = ProcessFailure::from_heap_limit_panic(payload.as_ref()) {
                Err(failure)
            } else {
                panic::resume_unwind(payload);
            }
        }
    }
}

fn deserialize_with_heap_limit(
    sendable: SendableValue,
    heap: &mut crate::heap::Heap,
) -> Result<Value, ProcessFailure> {
    match panic::catch_unwind(AssertUnwindSafe(|| deserialize(sendable, heap))) {
        Ok(value) => Ok(value),
        Err(payload) => {
            if let Some(failure) = ProcessFailure::from_heap_limit_panic(payload.as_ref()) {
                Err(failure)
            } else {
                panic::resume_unwind(payload);
            }
        }
    }
}

/// Deduplicate a pid list, preserving order.
pub fn dedup_pids(child_pids: Vec<Pid>) -> Vec<Pid> {
    let mut seen = std::collections::HashSet::with_capacity(child_pids.len());
    let mut unique = Vec::with_capacity(child_pids.len());
    for pid in child_pids {
        if seen.insert(pid) {
            unique.push(pid);
        }
    }
    unique
}
