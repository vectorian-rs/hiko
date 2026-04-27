#[cfg(any(
    test,
    feature = "builtin-aws-config",
    feature = "builtin-bytes",
    feature = "builtin-convert",
    feature = "builtin-env",
    feature = "builtin-exec",
    feature = "builtin-filesystem",
    feature = "builtin-hash",
    feature = "builtin-http",
    feature = "builtin-json",
    feature = "builtin-math",
    feature = "builtin-path",
    feature = "builtin-random",
    feature = "builtin-regex",
    feature = "builtin-stdio",
    feature = "builtin-string",
    feature = "builtin-system",
    feature = "builtin-testing",
    feature = "builtin-time"
))]
use crate::heap::Heap;
use crate::value::BuiltinFn;
#[cfg(any(
    test,
    feature = "builtin-bytes",
    feature = "builtin-convert",
    feature = "builtin-env",
    feature = "builtin-exec",
    feature = "builtin-filesystem",
    feature = "builtin-hash",
    feature = "builtin-http",
    feature = "builtin-json",
    feature = "builtin-math",
    feature = "builtin-path",
    feature = "builtin-random",
    feature = "builtin-regex",
    feature = "builtin-stdio",
    feature = "builtin-string",
    feature = "builtin-system",
    feature = "builtin-testing",
    feature = "builtin-time"
))]
use crate::value::HeapObject;
#[cfg(any(
    test,
    feature = "builtin-aws-config",
    feature = "builtin-bytes",
    feature = "builtin-convert",
    feature = "builtin-env",
    feature = "builtin-exec",
    feature = "builtin-filesystem",
    feature = "builtin-hash",
    feature = "builtin-http",
    feature = "builtin-json",
    feature = "builtin-math",
    feature = "builtin-path",
    feature = "builtin-random",
    feature = "builtin-regex",
    feature = "builtin-stdio",
    feature = "builtin-string",
    feature = "builtin-system",
    feature = "builtin-testing",
    feature = "builtin-time"
))]
use crate::value::Value;

mod support;

#[cfg(any(feature = "builtin-json", feature = "builtin-http"))]
mod json_value;

mod http_args;

#[cfg(any(
    test,
    feature = "builtin-filesystem",
    feature = "builtin-http",
    feature = "builtin-json",
    feature = "builtin-string"
))]
pub(crate) use support::alloc_list;
#[cfg(any(
    test,
    feature = "builtin-bytes",
    feature = "builtin-convert",
    feature = "builtin-env",
    feature = "builtin-filesystem",
    feature = "builtin-hash",
    feature = "builtin-http",
    feature = "builtin-json",
    feature = "builtin-path",
    feature = "builtin-random",
    feature = "builtin-regex",
    feature = "builtin-stdio",
    feature = "builtin-string",
    feature = "builtin-time"
))]
pub(crate) use support::heap_alloc;
pub(crate) use support::{collect_list, extract_pid_list_arg, extract_string_arg};
#[cfg(feature = "builtin-filesystem")]
pub(crate) use support::{fnv1a_tag_parts, is_valid_fnv1a_tag};

#[cfg(any(feature = "builtin-json", feature = "builtin-http"))]
pub(crate) use json_value::json_to_hiko;

#[cfg(feature = "builtin-json")]
pub(crate) use json_value::hiko_to_json_string;

pub(crate) use http_args::extract_http_args;
#[cfg(feature = "builtin-http")]
pub(crate) use http_args::{HttpArgRefs, extract_http_arg_refs};

#[cfg(feature = "builtin-aws-config")]
mod aws_config;
#[cfg(feature = "builtin-bytes")]
mod bytes;
#[cfg(feature = "builtin-convert")]
mod convert;
#[cfg(feature = "builtin-time")]
mod date;
#[cfg(feature = "builtin-env")]
mod env;
#[cfg(feature = "builtin-exec")]
mod exec;
#[cfg(feature = "builtin-filesystem")]
mod filesystem;
#[cfg(feature = "builtin-hash")]
mod hash;
#[cfg(feature = "builtin-http")]
mod http;
#[cfg(feature = "builtin-json")]
mod json;
#[cfg(feature = "builtin-math")]
mod math;
#[cfg(feature = "builtin-convert")]
mod numeric;
#[cfg(feature = "builtin-path")]
mod path;
#[cfg(feature = "builtin-process")]
mod process;
#[cfg(feature = "builtin-random")]
mod random;
#[cfg(feature = "builtin-regex")]
mod regex;
#[cfg(feature = "builtin-stdio")]
mod stdio;
#[cfg(feature = "builtin-string")]
mod string;
#[cfg(feature = "builtin-system")]
mod system;
#[cfg(feature = "builtin-testing")]
mod testing;
#[cfg(feature = "builtin-time")]
mod time;

pub(crate) fn builtin_entries() -> Vec<(&'static str, BuiltinFn)> {
    let mut entries = Vec::new();
    append_builtin_entries(&mut entries);
    entries
}

fn append_builtin_entries(_entries: &mut Vec<(&'static str, BuiltinFn)>) {
    #[cfg(feature = "builtin-aws-config")]
    _entries.extend(aws_config::entries());

    #[cfg(feature = "builtin-stdio")]
    _entries.extend(stdio::entries());

    #[cfg(feature = "builtin-convert")]
    {
        _entries.extend(convert::entries());
        _entries.extend(numeric::entries());
    }

    #[cfg(feature = "builtin-string")]
    _entries.extend(string::entries());

    #[cfg(feature = "builtin-regex")]
    _entries.extend(regex::entries());

    #[cfg(feature = "builtin-json")]
    _entries.extend(json::entries());

    #[cfg(feature = "builtin-math")]
    _entries.extend(math::entries());

    #[cfg(feature = "builtin-bytes")]
    _entries.extend(bytes::entries());

    #[cfg(feature = "builtin-hash")]
    _entries.extend(hash::entries());

    #[cfg(feature = "builtin-random")]
    _entries.extend(random::entries());

    #[cfg(feature = "builtin-env")]
    _entries.extend(env::entries());

    #[cfg(feature = "builtin-time")]
    {
        _entries.extend(time::entries());
        _entries.extend(date::entries());
    }

    #[cfg(feature = "builtin-process")]
    _entries.extend(process::entries());

    #[cfg(feature = "builtin-path")]
    _entries.extend(path::entries());

    #[cfg(feature = "builtin-filesystem")]
    _entries.extend(filesystem::entries());

    #[cfg(feature = "builtin-http")]
    _entries.extend(http::entries());

    #[cfg(feature = "builtin-exec")]
    _entries.extend(exec::entries());

    #[cfg(feature = "builtin-system")]
    _entries.extend(system::entries());

    #[cfg(feature = "builtin-testing")]
    _entries.extend(testing::entries());
}

#[cfg(test)]
pub(super) mod test_helpers {
    use super::*;
    use smallvec::smallvec;

    pub fn heap_string(value: Value, heap: &Heap) -> String {
        match value {
            Value::Heap(r) => match heap.get(r).unwrap() {
                HeapObject::String(text) => text.clone(),
                other => panic!("expected string, got {other:?}"),
            },
            other => panic!("expected heap string, got {other:?}"),
        }
    }

    pub fn string_arg(heap: &mut Heap, text: &str) -> Value {
        heap_alloc(heap, HeapObject::String(text.into())).unwrap()
    }

    pub fn tuple2(heap: &mut Heap, left: Value, right: Value) -> Value {
        heap_alloc(heap, HeapObject::Tuple(smallvec![left, right])).unwrap()
    }

    pub fn tuple3(heap: &mut Heap, a: Value, b: Value, c: Value) -> Value {
        heap_alloc(heap, HeapObject::Tuple(smallvec![a, b, c])).unwrap()
    }

    pub fn assert_int(value: Value, expected: i64) {
        match value {
            Value::Int(n) => assert_eq!(n, expected),
            other => panic!("expected Int({expected}), got {other:?}"),
        }
    }

    pub fn assert_bool(value: Value, expected: bool) {
        match value {
            Value::Bool(b) => assert_eq!(b, expected),
            other => panic!("expected Bool({expected}), got {other:?}"),
        }
    }

    pub fn assert_char(value: Value, expected: char) {
        match value {
            Value::Char(c) => assert_eq!(c, expected),
            other => panic!("expected Char({expected}), got {other:?}"),
        }
    }

    pub fn assert_float_approx(value: Value, expected: f64, epsilon: f64) {
        match value {
            Value::Float(f) => {
                assert!(
                    (f - expected).abs() < epsilon,
                    "expected ~{expected}, got {f}"
                );
            }
            other => panic!("expected Float(~{expected}), got {other:?}"),
        }
    }

    pub fn collect_string_list(value: Value, heap: &Heap) -> Vec<String> {
        let elems = collect_list(heap, value).unwrap();
        elems.iter().map(|v| heap_string(*v, heap)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn builtin_names_are_unique() {
        let entries = builtin_entries();
        let mut seen = HashSet::new();
        let mut duplicates = Vec::new();
        for (name, _) in &entries {
            if !seen.insert(name) {
                duplicates.push(*name);
            }
        }
        assert!(
            duplicates.is_empty(),
            "duplicate builtin names: {:?}",
            duplicates
        );
        assert_eq!(seen.len(), entries.len());
    }
}
