//! #264: untimed simd-json 0.19.0 feasibility, NOT a production JSON backend.
//! The frozen corpus contains valid JSON numbers outside this backend's range.
//! A passing test records limitations; it does not mean the foundation is met.
#![forbid(unsafe_code)]

use simd_json::{Buffers, OwnedValue, StaticNode, prelude::*};
use std::{mem::size_of, rc::Rc};

const CORPUS: &[(&str, &[u8], bool)] = &[
    ("null", "null".as_bytes(), true),
    ("bool", "true".as_bytes(), true),
    ("array", "[null,false,\"\"]".as_bytes(), true),
    ("object", "{\"a\":1}".as_bytes(), true),
    ("whitespace", " \t\r\ntrue \t\r\n".as_bytes(), true),
    ("zero", "0".as_bytes(), true),
    ("minus_zero", "-0".as_bytes(), true),
    ("float_one", "1.0".as_bytes(), true),
    ("float_one_zeros", "1.00".as_bytes(), true),
    ("exponent", "1e3".as_bytes(), true),
    ("exponent_decimal", "1000.0".as_bytes(), true),
    ("minus_float_zero", "-0.0".as_bytes(), true),
    ("i64_min", "-9223372036854775808".as_bytes(), true),
    ("i64_max", "9223372036854775807".as_bytes(), true),
    ("uint_above_i64", "9223372036854775809".as_bytes(), true),
    ("u64_max", "18446744073709551615".as_bytes(), true),
    ("above_u64", "18446744073709551616".as_bytes(), false),
    ("long_integer", "9999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999".as_bytes(), false),
    ("large_exponent", "1e400".as_bytes(), false),
    ("negative_large_exponent", "-1e400".as_bytes(), false),
    ("underflow", "1e-400".as_bytes(), true),
    ("rounded_decimal", "9007199254740993.0".as_bytes(), true),
    ("unicode", "\"é🙂\"".as_bytes(), true),
    ("surrogate_pair", "\"\\uD83D\\uDE42\"".as_bytes(), true),
    ("lone_high", "\"\\uD800\"".as_bytes(), false),
    ("lone_low", "\"\\uDEAD\"".as_bytes(), false),
    ("lone_key", "{\"\\uDEAD\":0}".as_bytes(), false),
    ("discarded_lone", "{\"a\":\"\\uDEAD\",\"a\":\"ok\"}".as_bytes(), false),
    ("invalid_utf8", &[34, 255, 34], false),
    ("bom", "﻿true".as_bytes(), false),
    ("nbsp", "true ".as_bytes(), false),
    ("duplicate", "{\"a\":1,\"a\":2}".as_bytes(), true),
    ("escaped_duplicate", "{\"a\":1,\"\\u0061\":2}".as_bytes(), true),
    ("discarded_bad", "{\"a\":01,\"a\":2}".as_bytes(), false),
    ("empty", "".as_bytes(), false),
    ("whitespace_only", " \t\r\n".as_bytes(), false),
    ("two_roots", "true false".as_bytes(), false),
    ("suffix", "{\"answer\":42} garbage".as_bytes(), false),
    ("later_bad", "{\"answer\":42,\"bad\":[0,]}".as_bytes(), false),
    ("comment", "/*c*/true".as_bytes(), false),
    ("trailing_comma", "[0,]".as_bytes(), false),
    ("nan", "NaN".as_bytes(), false),
    ("infinity", "Infinity".as_bytes(), false),
    ("plus", "+1".as_bytes(), false),
    ("leading_zero", "01".as_bytes(), false),
    ("dot", "1.".as_bytes(), false),
    ("leading_dot", ".1".as_bytes(), false),
    ("missing_exponent", "1e".as_bytes(), false),
    ("bad_escape", "\"\\x41\"".as_bytes(), false),
    ("raw_control", "\"\x01\"".as_bytes(), false),
    ("mutation", "{\"text\":\"line\\nvalue\\u0021\"}".as_bytes(), true),
    ("views", "{\"a.b\":{\"items\":[null,\"\",{\"é\":\"line\\nvalue\"}]}}".as_bytes(), true),
];

fn input(name: &str) -> &'static [u8] {
    CORPUS.iter().find(|c| c.0 == name).unwrap().1
}

fn dump_input(name: &str, bytes: &[u8]) {
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    println!("input {name} {hex}");
}

fn parse(bytes: &[u8]) -> simd_json::Result<OwnedValue> {
    // Required mutable copy: backend unescaping may overwrite the input.
    simd_json::to_owned_value(&mut bytes.to_vec())
}

#[test]
fn frozen_profile_and_whole_input_validation() {
    println!(
        "backend=simd-json/0.19.0 arch={} default_max_depth={}",
        std::env::consts::ARCH,
        simd_json::DEFAULT_MAX_DEPTH
    );
    for &(name, bytes, accepts) in CORPUS {
        dump_input(name, bytes);
        let result = parse(bytes);
        println!("semantic {name}: {result:?}");
        assert_eq!(result.is_ok(), accepts, "backend observation: {name}");
    }
    // Contrary to the proposed last-wins policy, this pinned owned DOM
    // retains both entries in small objects and lookup returns the first.
    for name in ["duplicate", "escaped_duplicate"] {
        let value = parse(input(name)).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 2);
        assert_eq!(value.get("a").unwrap().as_u64(), Some(1));
    }
    assert_eq!(parse(input("unicode")).unwrap().as_str(), Some("é🙂"));
    assert_eq!(parse(input("surrogate_pair")).unwrap().as_str(), Some("🙂"));
}

#[test]
fn owned_numeric_nodes_do_not_preserve_exact_tokens() {
    let one = parse(input("float_one")).unwrap();
    let zeros = parse(input("float_one_zeros")).unwrap();
    assert_eq!(one.encode(), zeros.encode());
    assert_ne!(zeros.encode().as_bytes(), input("float_one_zeros"));
    assert_eq!(parse(input("minus_zero")).unwrap().encode(), "0");
    assert_eq!(
        parse(input("minus_float_zero"))
            .unwrap()
            .as_f64()
            .unwrap()
            .to_bits(),
        (-0.0_f64).to_bits()
    );
    assert_eq!(
        parse(input("exponent")).unwrap().encode(),
        parse(input("exponent_decimal")).unwrap().encode()
    );
    assert!(matches!(
        parse(input("uint_above_i64")).unwrap(),
        OwnedValue::Static(StaticNode::U64(9_223_372_036_854_775_809))
    ));
    assert_eq!(parse(input("uint_above_i64")).unwrap().as_i64(), None);
    assert_eq!(parse(input("u64_max")).unwrap().as_u64(), Some(u64::MAX));
    assert_eq!(parse(input("i64_min")).unwrap().as_i64(), Some(i64::MIN));
    assert_eq!(parse(input("i64_max")).unwrap().as_i64(), Some(i64::MAX));
    assert_eq!(parse(input("underflow")).unwrap().as_f64(), Some(0.0));
    assert_eq!(
        parse(input("rounded_decimal")).unwrap().as_f64(),
        Some(9_007_199_254_740_992.0)
    );
    for name in [
        "above_u64",
        "long_integer",
        "large_exponent",
        "negative_large_exponent",
    ] {
        assert!(
            parse(input(name)).is_err(),
            "valid JSON, unsupported numeric range: {name}"
        );
    }
}

#[test]
fn custom_depth_is_enforced_before_owned_tree_publication() {
    assert_eq!(simd_json::DEFAULT_MAX_DEPTH, 1024);
    for depth in [127, 128, 129] {
        let mut bytes = format!("{}0{}", "[".repeat(depth), "]".repeat(depth)).into_bytes();
        dump_input(&format!("depth_{depth}"), &bytes);
        let mut buffers = Buffers::with_max_depth(bytes.len(), 128);
        let result = simd_json::to_owned_value_with_buffers(&mut bytes, &mut buffers);
        println!(
            "depth={depth} accepted={} error={:?}",
            result.is_ok(),
            result.as_ref().err()
        );
        assert_eq!(result.is_ok(), depth <= 128);
    }
}

#[test]
fn mutable_copy_and_owned_dom_have_independent_lifetimes() {
    let original = input("mutation");
    let mut copy = original.to_vec();
    let mut buffers = Buffers::new(copy.len());
    let value = simd_json::to_owned_value_with_buffers(&mut copy, &mut buffers).unwrap();
    assert_ne!(
        copy.as_slice(),
        original,
        "escaped strings are decoded in place"
    );
    // Both transient buffers and the mutable input can die before DOM access.
    drop(copy);
    drop(buffers);
    assert_eq!(value.get("text").unwrap().as_str(), Some("line\nvalue!"));
}

// Public capacities only. In particular object bucket bytes, parser scratch,
// allocator overhead, temporary/reallocation overlap and native stack are NOT
// measured here. Never use these post-parse observations for admission.
#[derive(Debug, Default)]
struct VisibleCapacities {
    array_slots: usize,
    object_slots: usize,
    string_bytes: usize,
}

fn capacities(value: &OwnedValue, out: &mut VisibleCapacities) {
    match value {
        OwnedValue::Array(values) => {
            out.array_slots += values.capacity();
            for value in values.iter() {
                capacities(value, out);
            }
        }
        OwnedValue::Object(values) => {
            out.object_slots += values.capacity();
            for (key, value) in values.iter() {
                out.string_bytes += key.capacity();
                capacities(value, out);
            }
        }
        OwnedValue::String(s) => out.string_bytes += s.capacity(),
        OwnedValue::Static(_) => {}
    }
}

fn dense(n: usize, object: bool) -> String {
    if object {
        format!(
            "{{{}}}",
            (0..n)
                .map(|i| format!(r#""k{i}":"v""#))
                .collect::<Vec<_>>()
                .join(",")
        )
    } else {
        format!("[{}]", vec!["0"; n].join(","))
    }
}

#[test]
fn scratch_hints_and_dom_output_capacity_are_not_hard_limits() {
    for n in [64, 128, 256] {
        for object in [false, true] {
            let original = dense(n, object);
            let kind = if object { "object" } else { "array" };
            dump_input(&format!("dense_{kind}_{n}"), original.as_bytes());
            // Start with zero hint, then full-size hint. Neither is a cap.
            // Reuse each buffer twice, but do not mistake reuse for admission.
            for hint in [0, original.len()] {
                let mut buffers = Buffers::with_max_depth(hint, 128);
                for pass in 0..2 {
                    let mut copy = original.as_bytes().to_vec();
                    let value =
                        simd_json::to_owned_value_with_buffers(&mut copy, &mut buffers).unwrap();
                    let count = if object {
                        value.as_object().unwrap().len()
                    } else {
                        value.as_array().unwrap().len()
                    };
                    assert_eq!(count, n);
                    let indexes = buffers.structural_indexes().len();
                    // Source-pinned initial structural capacity is hint/128;
                    // the populated length demonstrates subsequent growth.
                    assert!(indexes > hint / 128);
                    let mut visible = VisibleCapacities::default();
                    capacities(&value, &mut visible);
                    if object {
                        assert!(visible.object_slots >= n);
                        assert!(visible.string_bytes >= n);
                    } else {
                        assert!(visible.array_slots >= n);
                    }
                    let output = value.encode();
                    assert!(output.capacity() >= output.len());
                    assert_eq!(parse(output.as_bytes()).unwrap(), value);
                    println!(
                        "capacity n={n} object={object} hint={hint} pass={pass} input_len={} input_cap={} structural_len={indexes} visible={visible:?} output_len={} output_cap={}",
                        original.len(),
                        copy.capacity(),
                        output.len(),
                        output.capacity()
                    );
                }
            }
            // The owned API also builds a transient tape. This separate parse
            // exposes its public capacity, NOT the private owned-path peak.
            let mut copy = original.into_bytes();
            let mut buffers = Buffers::new(copy.len());
            let tape = simd_json::to_tape_with_buffers(&mut copy, &mut buffers).unwrap();
            println!(
                "tape n={n} object={object} nodes={} slots={} node_bytes={} owned_value_bytes={}",
                tape.0.len(),
                tape.0.capacity(),
                size_of::<simd_json::Node<'_>>(),
                size_of::<OwnedValue>()
            );
            assert!(tape.0.capacity() >= tape.0.len());
        }
    }
}

// A deliberately small safe ownership demonstration, not the proposed arena or
// constant-time node-ID representation. Rc makes this prototype non-Send.
// Owned typed paths avoid self-references; replay costs O(depth) lookups and
// copying a view costs O(path bytes). No borrowed reference escapes its view.
#[derive(Clone)]
enum Segment {
    Field(String),
    Item(usize),
}

#[derive(Clone)]
struct View {
    owner: Rc<OwnedValue>,
    path: Vec<Segment>,
}

impl View {
    fn value(&self) -> Option<&OwnedValue> {
        let mut value = self.owner.as_ref();
        for segment in &self.path {
            value = match segment {
                Segment::Field(key) => value.get(key.as_str())?,
                Segment::Item(i) => value.as_array()?.get(*i)?,
            };
        }
        Some(value)
    }

    fn child(&self, segment: Segment) -> Option<Self> {
        let mut view = self.clone();
        view.path.push(segment);
        view.value()?;
        Some(view)
    }
}

#[test]
fn immutable_child_view_retains_and_releases_whole_owner() {
    let root = View {
        owner: Rc::new(parse(input("views")).unwrap()),
        path: vec![],
    };
    let weak = Rc::downgrade(&root.owner);
    let nested = root.child(Segment::Field("a.b".into())).unwrap();
    let items = nested.child(Segment::Field("items".into())).unwrap();
    let child = items
        .child(Segment::Item(2))
        .unwrap()
        .child(Segment::Field("é".into()))
        .unwrap();
    assert!(root.child(Segment::Field("a".into())).is_none());
    assert!(root.child(Segment::Item(0)).is_none());
    assert!(items.child(Segment::Item(3)).is_none());
    assert!(items.child(Segment::Field("0".into())).is_none());
    assert!(
        items
            .child(Segment::Item(0))
            .unwrap()
            .value()
            .unwrap()
            .is_null()
    );
    assert_eq!(
        items
            .child(Segment::Item(1))
            .unwrap()
            .value()
            .unwrap()
            .as_str(),
        Some("")
    );
    drop((root, nested, items));
    assert!(weak.upgrade().is_some());
    assert_eq!(child.value().unwrap().as_str(), Some("line\nvalue"));
    let extracted = child.value().unwrap().as_str().unwrap().to_owned();
    drop(child);
    assert!(weak.upgrade().is_none());
    assert_eq!(extracted, "line\nvalue");
    // Not a Hiko GC test. A production owner/view needs traced ownership,
    // charge-once retained native capacity, metered path/index storage and
    // release on last collected view, failed publication and cancellation.
}
