use super::{BuiltinFn, Heap, Value};

pub(super) fn entries() -> Vec<(&'static str, BuiltinFn)> {
    [("aws_s3_list_buckets", list_buckets as BuiltinFn)].to_vec()
}

pub(super) fn list_buckets(_args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    Err("aws_s3_list_buckets: requires async I/O runtime".into())
}
