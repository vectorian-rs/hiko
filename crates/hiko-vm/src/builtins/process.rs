use super::*;

pub(super) fn spawn_placeholder(_args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    Err("spawn: must be called within a runtime".into())
}

pub(super) fn await_placeholder(_args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    Err("await_process: must be called within a runtime".into())
}
