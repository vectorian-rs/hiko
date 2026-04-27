use super::*;

pub(crate) fn entries() -> &'static [(&'static str, BuiltinFn)] {
    &[("aws_config_sso_profile", sso_profile as BuiltinFn)]
}

pub(super) fn sso_profile(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let _ = (args, heap);
    Err("aws_config_sso_profile: requires async I/O runtime".into())
}
