use super::*;

pub(super) fn spawn_placeholder(_args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    Err("spawn: must be called within a runtime".into())
}

pub(super) fn await_placeholder(_args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    Err("await_process: must be called within a runtime".into())
}

pub(super) fn cancel_placeholder(_args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    Err("cancel: must be called within a runtime".into())
}

pub(super) fn wait_any_placeholder(_args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    Err("wait_any: must be called within a runtime".into())
}
