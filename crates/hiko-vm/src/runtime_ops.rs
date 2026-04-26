//! Shared process operation logic used by both single-threaded and multi-threaded runtimes.

use smallvec::smallvec;

use crate::process::{FiberJoinError, Pid, ProcessFailure};
use crate::sendable::{SendableValue, deserialize};
use crate::value::{Fields, HeapObject, Value};
use crate::vm::VM;

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

use hiko_builtin_meta::{
    PROCESS_AWAIT_RESULT_ERR_TAG, PROCESS_AWAIT_RESULT_OK_TAG,
    PROCESS_JOIN_ERROR_ALREADY_JOINED_TAG, PROCESS_JOIN_ERROR_CANCELLED_TAG,
    PROCESS_JOIN_ERROR_FUEL_EXHAUSTED_TAG, PROCESS_JOIN_ERROR_HEAP_LIMIT_TAG,
    PROCESS_JOIN_ERROR_RUNTIME_ERROR_TAG,
};

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
///
/// Process creation currently has four distinct stages:
///
/// 1. `VM::create_child()` clones immutable program metadata and rebuilds the
///    builtin/global tables for a fresh interpreter instance.
/// 2. Spawn captures are deserialized into the child heap.
/// 3. `VM::setup_closure_call()` installs the initial frame for the closure.
/// 4. The runtime wraps the VM in `Process` and inserts it into its table.
///
/// Only stages 1-3 happen here. Table insertion remains runtime-specific.
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
    heap.alloc(object).map(Value::Heap).map_err(|e| match e {
        crate::heap::HeapLimitExceeded::Objects { limit, live } => {
            ProcessFailure::HeapObjectLimitExceeded { limit, live }
        }
        crate::heap::HeapLimitExceeded::Bytes { .. } => ProcessFailure::runtime(e.to_string()),
    })
}

fn deserialize_with_heap_limit(
    sendable: SendableValue,
    heap: &mut crate::heap::Heap,
) -> Result<Value, ProcessFailure> {
    deserialize(sendable, heap).map_err(|e| match e {
        crate::heap::HeapLimitExceeded::Objects { limit, live } => {
            ProcessFailure::HeapObjectLimitExceeded { limit, live }
        }
        crate::heap::HeapLimitExceeded::Bytes { .. } => ProcessFailure::runtime(e.to_string()),
    })
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
