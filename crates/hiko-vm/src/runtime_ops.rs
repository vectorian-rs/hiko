//! Shared process operation logic used by both single-threaded and multi-threaded runtimes.

use std::panic::{self, AssertUnwindSafe};

use crate::process::{Pid, ProcessFailure, ProcessStatus};
use crate::sendable::{SendableValue, deserialize, serialize};
use crate::value::Value;
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
