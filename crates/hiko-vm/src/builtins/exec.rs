use super::*;

pub(crate) fn entries() -> &'static [(&'static str, BuiltinFn)] {
    &[("exec", exec as BuiltinFn)]
}

pub(super) fn exec(_args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    Err("exec: builtin should be intercepted by the VM runtime".into())
}
