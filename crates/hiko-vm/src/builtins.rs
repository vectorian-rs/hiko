#[cfg(any(
    test,
    feature = "builtin-aws-config",
    feature = "builtin-aws-s3",
    feature = "builtin-aws-sqs",
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
    feature = "builtin-time",
    feature = "builtin-github"
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
    feature = "builtin-time",
    feature = "builtin-github"
))]
use crate::value::HeapObject;
#[cfg(any(
    test,
    feature = "builtin-aws-config",
    feature = "builtin-aws-s3",
    feature = "builtin-aws-sqs",
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
    feature = "builtin-time",
    feature = "builtin-github"
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
pub(crate) use support::host_work;
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

#[cfg(any(
    feature = "builtin-aws-config",
    feature = "builtin-aws-s3",
    feature = "builtin-aws-sqs"
))]
mod aws;
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
#[cfg(feature = "builtin-github")]
mod github;
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
    #[cfg(any(
        feature = "builtin-aws-config",
        feature = "builtin-aws-s3",
        feature = "builtin-aws-sqs"
    ))]
    _entries.extend(aws::entries());

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

    #[cfg(feature = "builtin-github")]
    _entries.extend(github::entries());
}

#[cfg(test)]
pub(super) mod test_helpers {
    use super::*;
    use smallvec::smallvec;

    pub fn heap_string(value: Value, heap: &Heap) -> String {
        match value {
            Value::Heap(r) => match heap.get(r).unwrap() {
                HeapObject::String(text) => text.to_string(),
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

    /// The type checker binds every name in `hiko_builtin_meta::builtins()`,
    /// so the VM must register exactly the same set under the same features.
    #[test]
    fn registered_builtins_match_builtin_metadata() {
        use std::collections::BTreeSet;
        let registered: BTreeSet<&str> = builtin_entries().into_iter().map(|(n, _)| n).collect();
        let declared: BTreeSet<&str> = hiko_builtin_meta::builtins().map(|m| m.name).collect();
        let missing_metadata: Vec<_> = registered.difference(&declared).collect();
        let missing_registration: Vec<_> = declared.difference(&registered).collect();
        assert!(
            missing_metadata.is_empty() && missing_registration.is_empty(),
            "registered but not declared: {missing_metadata:?}; declared but not registered: {missing_registration:?}"
        );
    }
}

/// Host work must grow with the input a builtin walks, so a large input
/// cannot cost the same as a small one.
#[cfg(all(
    test,
    feature = "builtin-string",
    feature = "builtin-bytes",
    feature = "builtin-regex",
    feature = "builtin-json",
    feature = "builtin-hash",
    feature = "builtin-random"
))]
mod host_work_tests {
    use super::test_helpers::*;
    use super::*;

    fn call(name: &str, heap: &mut Heap, arg: Value) -> Value {
        let (_, builtin) = builtin_entries()
            .into_iter()
            .find(|(n, _)| *n == name)
            .unwrap_or_else(|| panic!("no builtin {name}"));
        builtin(&[arg], heap).unwrap_or_else(|e| panic!("{name}: {e}"))
    }

    /// The host work one call of `name` charges, for an input built from
    /// `n` bytes by `build`.
    fn charged(name: &str, n: usize, build: impl Fn(&mut Heap, usize) -> Value) -> u64 {
        let mut heap = Heap::new();
        let arg = build(&mut heap, n);
        let before = heap.host_work_used();
        call(name, &mut heap, arg);
        heap.host_work_used() - before
    }

    fn text(heap: &mut Heap, n: usize) -> Value {
        string_arg(heap, &"a".repeat(n))
    }

    fn bytes(heap: &mut Heap, n: usize) -> Value {
        heap_alloc(heap, HeapObject::Bytes(vec![b'a'; n].into())).unwrap()
    }

    fn json_array(heap: &mut Heap, n: usize) -> Value {
        let items = vec!["1"; n / 2].join(",");
        string_arg(heap, &format!("[{items}]"))
    }

    fn with_strings(heap: &mut Heap, first: Value, rest: &[&str]) -> Value {
        let mut fields = vec![first];
        fields.extend(rest.iter().map(|s| string_arg(heap, s)));
        heap_alloc(heap, HeapObject::Tuple(fields.into())).unwrap()
    }

    #[test]
    fn charges_grow_with_input_size() {
        type Build = fn(&mut Heap, usize) -> Value;
        let cases: &[(&str, Build)] = &[
            ("string_length", text),
            ("substring", |h, n| {
                let s = text(h, n);
                tuple3(h, s, Value::Int(n as i64 - 8), Value::Int(8))
            }),
            ("string_contains", |h, n| {
                let s = text(h, n);
                with_strings(h, s, &["zz"])
            }),
            ("trim", text),
            ("to_upper", text),
            ("to_lower", text),
            ("split", |h, n| {
                let s = text(h, n);
                with_strings(h, s, &[","])
            }),
            ("string_replace", |h, n| {
                let s = text(h, n);
                with_strings(h, s, &["a", "b"])
            }),
            ("string_join", |h, n| {
                let parts = (0..n / 16)
                    .map(|_| string_arg(h, "abcdefghijklmnop"))
                    .collect();
                let list = alloc_list(h, parts).unwrap();
                with_strings(h, list, &[""])
            }),
            ("string_to_bytes", text),
            ("bytes_to_string", bytes),
            ("bytes_slice", |h, n| {
                let b = bytes(h, n);
                tuple3(h, b, Value::Int(0), Value::Int(n as i64))
            }),
            ("blake3", bytes),
            ("regex_match", |h, n| {
                let s = text(h, n);
                with_strings(h, s, &["z+"])
            }),
            ("regex_replace", |h, n| {
                let s = text(h, n);
                with_strings(h, s, &["a", "b"])
            }),
            ("json_parse", json_array),
            ("json_length", json_array),
            ("json_get", |h, n| {
                let a = json_array(h, n);
                with_strings(h, a, &["0"])
            }),
            ("json_to_string", |h, n| {
                let a = json_array(h, n);
                call("json_parse", h, a)
            }),
            ("random_bytes", |_, n| Value::Int(n as i64)),
            ("rng_seed", bytes),
        ];
        for (name, build) in cases {
            let small = charged(name, 16, build);
            let large = charged(name, 64 * 1024, build);
            // Every per-KiB rate is at least 1, so 64 KiB adds at least 64.
            assert!(
                large >= small + 64,
                "{name}: charged {small} for 16 B and {large} for 64 KiB"
            );
        }
    }
}
