use hiko_json::{ErrorKind, Kind, Limits, OwnedReader};
use std::sync::Arc;

fn reader(s: &str) -> OwnedReader {
    OwnedReader::new(s.into(), Limits::default()).unwrap()
}

#[test]
fn nested_resume_moves_and_drains_frames_without_replay() {
    let input: Arc<str> =
        r#"{"ignored":[{},[null]],"user":{"age":37,"left":false},"events":[10,20]}"#.into();
    let mut r = OwnedReader::new(input.clone(), Limits::default()).unwrap();
    let caps = r.capacities();
    r.begin_object().unwrap();
    assert!(r.find_next_field("user").unwrap());
    // The whole owned frontend can move while a value is pending.
    let mut r = Box::new(r);
    r.begin_object().unwrap();
    assert!(r.find_next_field("age").unwrap());
    assert_eq!(r.read_int().unwrap(), 37);
    r.finish_object().unwrap();
    assert!(r.find_next_field("events").unwrap());
    r.begin_array().unwrap();
    let mut sum = 0;
    while r.next_item().unwrap() {
        sum += r.read_word().unwrap();
    }
    assert_eq!(sum, 30);
    // End marker does not pop; wrong frame finish is rejected separately below.
    r.finish_array().unwrap();
    r.finish().unwrap();
    assert_eq!(r.stats().advanced_bytes, input.len());
    assert_eq!(r.capacities(), caps);
    assert!(r.estimated_bytes() <= OwnedReader::admission_bytes(input.len()).unwrap());
    assert_eq!(Arc::strong_count(&input), 2);
    drop(r);
    assert_eq!(Arc::strong_count(&input), 1);
}

#[test]
fn duplicate_literal_keys_missing_null_empty_and_all_scalar_operations() {
    let mut r =
        reader(r#"{"a":1,"a":2,"a.b":null,"":"","b":true,"f":-0.0,"n":1.00,"s":"\uD83D\uDE42"}"#);
    r.begin_object().unwrap();
    for n in [1, 2] {
        assert!(r.find_next_field("a").unwrap());
        assert_eq!(r.read_int().unwrap(), n);
    }
    assert_eq!(r.next_field().unwrap(), Some("a.b"));
    assert_eq!(r.kind().unwrap(), Kind::Null);
    r.read_null().unwrap();
    assert_eq!(r.next_field().unwrap(), Some(""));
    assert_eq!(r.read_string().unwrap(), "");
    assert_eq!(r.next_field().unwrap(), Some("b"));
    assert!(r.read_bool().unwrap());
    assert_eq!(r.next_field().unwrap(), Some("f"));
    assert_eq!(r.read_float().unwrap().to_bits(), (-0.0f64).to_bits());
    assert_eq!(r.next_field().unwrap(), Some("n"));
    assert_eq!(r.read_number_text().unwrap(), "1.00");
    assert_eq!(r.next_field().unwrap(), Some("s"));
    assert_eq!(r.read_string().unwrap(), "🙂");
    assert!(!r.find_next_field("a").unwrap());
    assert_eq!(r.next_field().unwrap(), None);
    r.finish_object().unwrap();
    r.finish().unwrap();
}

#[test]
fn protocol_errors_and_late_syntax_have_absolute_offsets() {
    let mut r = reader(r#"{"ok":1,"bad":[1,]}"#);
    r.begin_object().unwrap();
    assert!(r.find_next_field("ok").unwrap());
    assert_eq!(r.read_int().unwrap(), 1);
    let e = r.finish().unwrap_err();
    assert!(matches!(e.kind, ErrorKind::Syntax(_)));
    assert_eq!(e.offset, 17);
    let pos = r.stats().advanced_bytes;
    assert_eq!(r.kind().unwrap_err().kind, ErrorKind::Poisoned);
    assert_eq!(r.stats().advanced_bytes, pos);
    let mut r = reader("  true false");
    assert!(r.read_bool().unwrap());
    assert_eq!(r.finish().unwrap_err().offset, 7);
    let mut r = reader("[1,2]");
    r.begin_array().unwrap();
    assert!(r.next_item().unwrap());
    assert_eq!(r.next_item().unwrap_err().kind, ErrorKind::UnconsumedChild);
    let mut r = reader("[]");
    r.begin_array().unwrap();
    assert!(!r.next_item().unwrap());
    assert_eq!(r.finish_object().unwrap_err().kind, ErrorKind::WrongKind);
    let mut r = reader("1");
    r.skip().unwrap();
    assert_eq!(r.skip().unwrap_err().kind, ErrorKind::Consumed);
}

#[test]
fn cached_classification_and_single_float_conversion_have_explicit_counts() {
    let mut r = reader(" 1.25");
    for _ in 0..4 {
        assert_eq!(r.kind().unwrap(), Kind::Number);
    }
    assert_eq!(r.stats().backend_calls, 1);
    assert_eq!(r.stats().classification_peeks, 1);
    assert_eq!(r.read_float().unwrap(), 1.25);
    assert_eq!(r.stats().backend_calls, 3); // peek, raw token, conversion
    assert_eq!(r.stats().float_conversions, 1);
    assert_eq!(r.stats().api_calls, 5);
    r.finish().unwrap();
    assert_eq!(r.stats().charged_work, 32 * 5 + 256 + 6 + 4);
    let mut r = reader("[1.25]");
    r.begin_array().unwrap();
    assert!(r.next_item().unwrap());
    assert_eq!(r.kind().unwrap(), Kind::Number);
    assert_eq!(r.read_float().unwrap(), 1.25);
    // Array advancement supplies the child's Peek: no second classification.
    assert_eq!(r.stats().classification_peeks, 1);
    assert_eq!(r.stats().float_conversions, 1);
    r.finish().unwrap();
}

#[test]
fn numeric_profile_and_wrong_kind_avoid_number_any() {
    assert_eq!(reader("-0").read_word().unwrap(), 0);
    assert_eq!(
        reader("18446744073709551615").read_word().unwrap(),
        u64::MAX
    );
    for (s, kind) in [
        ("1.0", ErrorKind::IntegerTokenRequired),
        ("18446744073709551616", ErrorKind::NumberRange),
        ("-1", ErrorKind::NumberRange),
    ] {
        assert_eq!(reader(s).read_word().unwrap_err().kind, kind);
    }
    for (s, kind) in [
        ("1e-400", ErrorKind::Underflow),
        ("1e400", ErrorKind::NonFinite),
    ] {
        assert_eq!(reader(s).read_float().unwrap_err().kind, kind);
    }
    let mut r = reader("18446744073709551616");
    assert_eq!(r.read_string().unwrap_err().kind, ErrorKind::WrongKind);
    assert_eq!(r.stats().backend_calls, 1);
    assert_eq!(r.stats().advanced_bytes, 0);
}

#[test]
fn owned_depth_and_long_escaped_buffers_remain_bounded() {
    for n in [127, 128, 129] {
        let s = format!("{}0{}", "[".repeat(n), "]".repeat(n));
        assert_eq!(reader(&s).finish().is_ok(), n <= 128);
    }
    for n in [9000, 20000] {
        let s = format!(
            r#"{{"{}":"{}","tail":[0]}}"#,
            "\\u0061".repeat(n),
            "\\u0062".repeat(n)
        );
        let mut r = reader(&s);
        let caps = r.capacities();
        r.begin_object().unwrap();
        assert_eq!(r.next_field().unwrap().unwrap().len(), n);
        assert_eq!(r.read_string().unwrap().len(), n);
        r.finish().unwrap();
        assert_eq!(r.capacities(), caps);
    }
}

#[test]
fn frozen_owned_dense_growth_uses_cached_array_peeks() {
    for n in [64, 128, 256] {
        let s = format!("[{}]", vec!["0"; n].join(","));
        let mut r = reader(&s);
        let caps = r.capacities();
        r.begin_array().unwrap();
        let mut count = 0;
        while r.next_item().unwrap() {
            assert_eq!(r.read_word().unwrap(), 0);
            count += 1;
        }
        r.finish_array().unwrap();
        r.finish().unwrap();
        assert_eq!(count, n);
        assert_eq!(r.stats().backend_calls, 2 * n as u64 + 3);
        assert_eq!(r.stats().advanced_bytes, s.len());
        assert_eq!(r.capacities(), caps);
        println!(
            "owned dense n={n} calls={} input={} estimated={}",
            r.stats().backend_calls,
            s.len(),
            r.estimated_bytes()
        );
    }
}
