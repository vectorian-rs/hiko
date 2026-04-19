use super::*;

pub(super) fn exit(args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Int(code) => std::process::exit(*code as i32),
        _ => Err("exit: expected Int".into()),
    }
}
