//! Shared process operation logic used by both single-threaded and multi-threaded runtimes.

use crate::process::{BlockReason, Pid, ProcessStatus};
use crate::sendable::{SendableValue, deserialize, serialize};
use crate::value::Value;
use crate::vm::VM;
use hiko_compile::chunk::CompiledProgram;

/// Result of checking a child's state for an await operation.
pub enum ChildState {
    Done(SendableValue),
    Failed(String),
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
pub fn deliver_result_to_parent(parent_vm: &mut VM, sendable: SendableValue) {
    parent_vm.stack.pop(); // remove placeholder
    let val = deserialize(sendable, &mut parent_vm.heap);
    parent_vm.push_value(val);
}

/// Deliver a message to a process (either from mailbox or direct delivery).
pub fn deliver_message(vm: &mut VM, msg: SendableValue) {
    vm.stack.pop(); // remove placeholder
    let val = deserialize(msg, &mut vm.heap);
    vm.push_value(val);
}

/// Create a child VM from a parent's program with serialized captures.
/// Uses VM::new (all builtins, no restrictions). For capability-aware
/// child creation, use create_child_vm_from_parent.
pub fn create_child_vm(
    program: CompiledProgram,
    proto_idx: usize,
    captures: Vec<SendableValue>,
) -> VM {
    let mut child_vm = VM::new(program);
    let child_captures: Vec<Value> = captures
        .into_iter()
        .map(|v| deserialize(v, &mut child_vm.heap))
        .collect();
    child_vm.setup_closure_call(proto_idx, &child_captures);
    child_vm
}

/// Create a child VM that inherits capabilities from the parent VM.
pub fn create_child_vm_from_parent(
    parent_vm: &VM,
    proto_idx: usize,
    captures: Vec<SendableValue>,
) -> VM {
    let mut child_vm = parent_vm.create_child();
    let child_captures: Vec<Value> = captures
        .into_iter()
        .map(|v| deserialize(v, &mut child_vm.heap))
        .collect();
    child_vm.setup_closure_call(proto_idx, &child_captures);
    child_vm
}

// ── I/O Result construction ──────────────────────────────────────────
// Matches stdlib/io.hml: datatype io_result = IoOk of String | IoErr of String
// IoOk = tag 0, IoErr = tag 1

const TAG_IO_OK: u16 = 0;
const TAG_IO_ERR: u16 = 1;

/// Construct an IoOk(value) in the given VM's heap.
pub fn make_io_ok(vm: &mut VM, value: Value) -> Value {
    use smallvec::smallvec;
    Value::Heap(vm.heap.alloc(crate::value::HeapObject::Data {
        tag: TAG_IO_OK,
        fields: smallvec![value],
    }))
}

/// Construct an IoErr(message) in the given VM's heap.
pub fn make_io_err(vm: &mut VM, message: &str) -> Value {
    use smallvec::smallvec;
    let msg = Value::Heap(
        vm.heap
            .alloc(crate::value::HeapObject::String(message.to_string())),
    );
    Value::Heap(vm.heap.alloc(crate::value::HeapObject::Data {
        tag: TAG_IO_ERR,
        fields: smallvec![msg],
    }))
}

/// Known runtime-handled effect names.
/// Effects with these names are automatically registered as runtime-handled
/// when a VM is created, so `perform` suspends instead of erroring.
pub const RUNTIME_EFFECT_NAMES: &[&str] = &["HttpGet", "Sleep"];

/// Register known runtime effects on a VM based on its compiled effect metadata.
pub fn register_runtime_effects(vm: &mut VM) {
    for meta in vm.effect_metadata.clone() {
        if RUNTIME_EFFECT_NAMES.contains(&meta.name.as_str()) {
            vm.register_runtime_effect(meta.tag);
        }
    }
}

/// Serialize the result value from a finished process.
pub fn serialize_result(vm: &VM) -> SendableValue {
    let val = vm.stack.last().copied().unwrap_or(Value::Unit);
    serialize(val, &vm.heap).unwrap_or(SendableValue::Unit)
}

/// Prepare the delivery payload for waiters of a finished process.
pub fn prepare_delivery(status: &ProcessStatus, vm: &VM) -> Result<SendableValue, String> {
    match status {
        ProcessStatus::Done => Ok(serialize_result(vm)),
        ProcessStatus::Failed(msg) => Err(msg.clone()),
        _ => Err("child not finished".into()),
    }
}
