use hiko_json::{ErrorKind, Kind, Limits, Reader, Scratch};

const LEGACY: &[(&str, &[u8], bool)] = &[
    ("null", &[0x6e, 0x75, 0x6c, 0x6c], true),
    ("bool", &[0x74, 0x72, 0x75, 0x65], true),
    (
        "array",
        &[
            0x5b, 0x6e, 0x75, 0x6c, 0x6c, 0x2c, 0x66, 0x61, 0x6c, 0x73, 0x65, 0x2c, 0x22, 0x22,
            0x5d,
        ],
        true,
    ),
    ("object", &[0x7b, 0x22, 0x61, 0x22, 0x3a, 0x31, 0x7d], true),
    (
        "whitespace",
        &[
            0x20, 0x09, 0x0d, 0x0a, 0x74, 0x72, 0x75, 0x65, 0x20, 0x09, 0x0d, 0x0a,
        ],
        true,
    ),
    ("zero", &[0x30], true),
    ("minus_zero", &[0x2d, 0x30], true),
    ("float_one", &[0x31, 0x2e, 0x30], true),
    ("float_one_zeros", &[0x31, 0x2e, 0x30, 0x30], true),
    ("exponent", &[0x31, 0x65, 0x33], true),
    (
        "exponent_decimal",
        &[0x31, 0x30, 0x30, 0x30, 0x2e, 0x30],
        true,
    ),
    ("minus_float_zero", &[0x2d, 0x30, 0x2e, 0x30], true),
    (
        "i64_min",
        &[
            0x2d, 0x39, 0x32, 0x32, 0x33, 0x33, 0x37, 0x32, 0x30, 0x33, 0x36, 0x38, 0x35, 0x34,
            0x37, 0x37, 0x35, 0x38, 0x30, 0x38,
        ],
        true,
    ),
    (
        "i64_max",
        &[
            0x39, 0x32, 0x32, 0x33, 0x33, 0x37, 0x32, 0x30, 0x33, 0x36, 0x38, 0x35, 0x34, 0x37,
            0x37, 0x35, 0x38, 0x30, 0x37,
        ],
        true,
    ),
    (
        "uint_above_i64",
        &[
            0x39, 0x32, 0x32, 0x33, 0x33, 0x37, 0x32, 0x30, 0x33, 0x36, 0x38, 0x35, 0x34, 0x37,
            0x37, 0x35, 0x38, 0x30, 0x39,
        ],
        true,
    ),
    (
        "u64_max",
        &[
            0x31, 0x38, 0x34, 0x34, 0x36, 0x37, 0x34, 0x34, 0x30, 0x37, 0x33, 0x37, 0x30, 0x39,
            0x35, 0x35, 0x31, 0x36, 0x31, 0x35,
        ],
        true,
    ),
    (
        "above_u64",
        &[
            0x31, 0x38, 0x34, 0x34, 0x36, 0x37, 0x34, 0x34, 0x30, 0x37, 0x33, 0x37, 0x30, 0x39,
            0x35, 0x35, 0x31, 0x36, 0x31, 0x36,
        ],
        true,
    ),
    (
        "long_integer",
        &[
            0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39,
            0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39,
            0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39,
            0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39,
            0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39,
            0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39,
            0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39,
            0x39, 0x39,
        ],
        true,
    ),
    ("large_exponent", &[0x31, 0x65, 0x34, 0x30, 0x30], true),
    (
        "negative_large_exponent",
        &[0x2d, 0x31, 0x65, 0x34, 0x30, 0x30],
        true,
    ),
    ("underflow", &[0x31, 0x65, 0x2d, 0x34, 0x30, 0x30], true),
    (
        "rounded_decimal",
        &[
            0x39, 0x30, 0x30, 0x37, 0x31, 0x39, 0x39, 0x32, 0x35, 0x34, 0x37, 0x34, 0x30, 0x39,
            0x39, 0x33, 0x2e, 0x30,
        ],
        true,
    ),
    (
        "unicode",
        &[0x22, 0xc3, 0xa9, 0xf0, 0x9f, 0x99, 0x82, 0x22],
        true,
    ),
    (
        "surrogate_pair",
        &[
            0x22, 0x5c, 0x75, 0x44, 0x38, 0x33, 0x44, 0x5c, 0x75, 0x44, 0x45, 0x34, 0x32, 0x22,
        ],
        true,
    ),
    (
        "lone_high",
        &[0x22, 0x5c, 0x75, 0x44, 0x38, 0x30, 0x30, 0x22],
        false,
    ),
    (
        "lone_low",
        &[0x22, 0x5c, 0x75, 0x44, 0x45, 0x41, 0x44, 0x22],
        false,
    ),
    (
        "lone_key",
        &[
            0x7b, 0x22, 0x5c, 0x75, 0x44, 0x45, 0x41, 0x44, 0x22, 0x3a, 0x30, 0x7d,
        ],
        false,
    ),
    (
        "discarded_lone",
        &[
            0x7b, 0x22, 0x61, 0x22, 0x3a, 0x22, 0x5c, 0x75, 0x44, 0x45, 0x41, 0x44, 0x22, 0x2c,
            0x22, 0x61, 0x22, 0x3a, 0x22, 0x6f, 0x6b, 0x22, 0x7d,
        ],
        false,
    ),
    ("invalid_utf8", &[0x22, 0xff, 0x22], false),
    ("bom", &[0xef, 0xbb, 0xbf, 0x74, 0x72, 0x75, 0x65], false),
    ("nbsp", &[0x74, 0x72, 0x75, 0x65, 0xc2, 0xa0], false),
    (
        "duplicate",
        &[
            0x7b, 0x22, 0x61, 0x22, 0x3a, 0x31, 0x2c, 0x22, 0x61, 0x22, 0x3a, 0x32, 0x7d,
        ],
        true,
    ),
    (
        "escaped_duplicate",
        &[
            0x7b, 0x22, 0x61, 0x22, 0x3a, 0x31, 0x2c, 0x22, 0x5c, 0x75, 0x30, 0x30, 0x36, 0x31,
            0x22, 0x3a, 0x32, 0x7d,
        ],
        true,
    ),
    (
        "discarded_bad",
        &[
            0x7b, 0x22, 0x61, 0x22, 0x3a, 0x30, 0x31, 0x2c, 0x22, 0x61, 0x22, 0x3a, 0x32, 0x7d,
        ],
        false,
    ),
    ("empty", &[], false),
    ("whitespace_only", &[0x20, 0x09, 0x0d, 0x0a], false),
    (
        "two_roots",
        &[0x74, 0x72, 0x75, 0x65, 0x20, 0x66, 0x61, 0x6c, 0x73, 0x65],
        false,
    ),
    (
        "suffix",
        &[
            0x7b, 0x22, 0x61, 0x6e, 0x73, 0x77, 0x65, 0x72, 0x22, 0x3a, 0x34, 0x32, 0x7d, 0x20,
            0x67, 0x61, 0x72, 0x62, 0x61, 0x67, 0x65,
        ],
        false,
    ),
    (
        "later_bad",
        &[
            0x7b, 0x22, 0x61, 0x6e, 0x73, 0x77, 0x65, 0x72, 0x22, 0x3a, 0x34, 0x32, 0x2c, 0x22,
            0x62, 0x61, 0x64, 0x22, 0x3a, 0x5b, 0x30, 0x2c, 0x5d, 0x7d,
        ],
        false,
    ),
    (
        "comment",
        &[0x2f, 0x2a, 0x63, 0x2a, 0x2f, 0x74, 0x72, 0x75, 0x65],
        false,
    ),
    ("trailing_comma", &[0x5b, 0x30, 0x2c, 0x5d], false),
    ("nan", &[0x4e, 0x61, 0x4e], false),
    (
        "infinity",
        &[0x49, 0x6e, 0x66, 0x69, 0x6e, 0x69, 0x74, 0x79],
        false,
    ),
    ("plus", &[0x2b, 0x31], false),
    ("leading_zero", &[0x30, 0x31], false),
    ("dot", &[0x31, 0x2e], false),
    ("leading_dot", &[0x2e, 0x31], false),
    ("missing_exponent", &[0x31, 0x65], false),
    ("bad_escape", &[0x22, 0x5c, 0x78, 0x34, 0x31, 0x22], false),
    ("raw_control", &[0x22, 0x01, 0x22], false),
    (
        "mutation",
        &[
            0x7b, 0x22, 0x74, 0x65, 0x78, 0x74, 0x22, 0x3a, 0x22, 0x6c, 0x69, 0x6e, 0x65, 0x5c,
            0x6e, 0x76, 0x61, 0x6c, 0x75, 0x65, 0x5c, 0x75, 0x30, 0x30, 0x32, 0x31, 0x22, 0x7d,
        ],
        true,
    ),
    (
        "views",
        &[
            0x7b, 0x22, 0x61, 0x2e, 0x62, 0x22, 0x3a, 0x7b, 0x22, 0x69, 0x74, 0x65, 0x6d, 0x73,
            0x22, 0x3a, 0x5b, 0x6e, 0x75, 0x6c, 0x6c, 0x2c, 0x22, 0x22, 0x2c, 0x7b, 0x22, 0xc3,
            0xa9, 0x22, 0x3a, 0x22, 0x6c, 0x69, 0x6e, 0x65, 0x5c, 0x6e, 0x76, 0x61, 0x6c, 0x75,
            0x65, 0x22, 0x7d, 0x5d, 0x7d, 0x7d,
        ],
        true,
    ),
];

fn validate(bytes: &[u8]) -> bool {
    let mut scratch = Scratch::default();
    let borrowed = Reader::new(bytes, &mut scratch, Limits::default()).and_then(|mut r| r.finish());
    if let Ok(input) = std::str::from_utf8(bytes) {
        let owned = hiko_json::OwnedReader::new(input.into(), Limits::default())
            .and_then(|mut r| r.finish());
        assert_eq!(borrowed, owned, "frontend parity {bytes:?}");
    }
    borrowed.is_ok()
}

#[test]
fn frozen_literals_and_raw_numeric_profile() {
    for &(name, bytes, accepted) in LEGACY {
        let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        println!("input {name} {hex}");
        assert_eq!(validate(bytes), accepted, "{name}");
        if accepted && (bytes[0].is_ascii_digit() || bytes[0] == b'-') {
            let mut scratch = Scratch::default();
            let mut r = Reader::new(bytes, &mut scratch, Limits::default()).unwrap();
            assert_eq!(r.root().unwrap().number_text().unwrap().as_bytes(), bytes);
            r.finish().unwrap();
        }
    }
}

#[test]
fn nested_fields_and_arrays_without_a_tree() {
    let bytes = br#"{"user":{"name":"Ada","age":37},"events":[10,20,30]}"#;
    let mut scratch = Scratch::default();
    let mut r = Reader::new(bytes, &mut scratch, Limits::default()).unwrap();
    let mut root = r.root().unwrap().object().unwrap();
    {
        let mut f = root.find_next_field("user").unwrap().unwrap();
        let mut user = f.value().unwrap().object().unwrap();
        {
            let mut name = user.find_next_field("name").unwrap().unwrap();
            let mut v = name.value().unwrap();
            assert_eq!(v.kind().unwrap(), Kind::String);
            assert_eq!(v.string().unwrap(), "Ada");
        }
        {
            let mut age = user.find_next_field("age").unwrap().unwrap();
            assert_eq!(age.value().unwrap().u64().unwrap(), 37);
        }
        user.finish().unwrap();
    }
    {
        let mut f = root.find_next_field("events").unwrap().unwrap();
        let mut a = f.value().unwrap().array().unwrap();
        let mut total = 0;
        while let Some(mut v) = a.next_value().unwrap() {
            total += v.i64().unwrap();
        }
        assert_eq!(total, 60);
        a.finish().unwrap();
    }
    root.finish().unwrap();
    r.finish().unwrap();
    assert_eq!(r.stats().advanced_bytes, bytes.len());
}

#[test]
fn forward_fields_duplicates_null_empty_and_literal_keys() {
    let mut scratch = Scratch::default();
    let mut r = Reader::new(
        br#"{"a":1,"a":2,"a.b":null,"":""}"#,
        &mut scratch,
        Limits::default(),
    )
    .unwrap();
    let mut root = r.root().unwrap().object().unwrap();
    for expected in [1, 2] {
        let mut f = root.find_next_field("a").unwrap().unwrap();
        assert_eq!(f.value().unwrap().i64().unwrap(), expected);
    }
    {
        let mut f = root.find_next_field("a.b").unwrap().unwrap();
        let mut v = f.value().unwrap();
        assert_eq!(v.kind().unwrap(), Kind::Null);
        v.null().unwrap();
    }
    {
        let mut f = root.find_next_field("").unwrap().unwrap();
        assert_eq!(f.value().unwrap().string().unwrap(), "");
    }
    assert!(root.find_next_field("a").unwrap().is_none()); // not a rewind
    assert!(root.find_next_field("absent").unwrap().is_none());
    root.finish().unwrap();
    r.finish().unwrap();
}

#[test]
fn parent_key_is_preserved_while_nested_keys_reuse_scratch() {
    let mut scratch = Scratch::default();
    let mut r = Reader::new(
        br#"{"a\u002eb":{"nested\u0021":"\uD83D\uDE42"}}"#,
        &mut scratch,
        Limits::default(),
    )
    .unwrap();
    let mut o = r.root().unwrap().object().unwrap();
    {
        let mut f = o.next_field().unwrap().unwrap();
        assert_eq!(f.key(), "a.b");
        {
            let mut inner = f.value().unwrap().object().unwrap();
            {
                let mut child = inner.next_field().unwrap().unwrap();
                assert_eq!(child.key(), "nested!");
                assert_eq!(child.value().unwrap().string().unwrap(), "🙂");
            }
            inner.finish().unwrap();
        }
        assert_eq!(f.key(), "a.b");
    }
    o.finish().unwrap();
    r.finish().unwrap();
}

#[test]
fn extraction_is_not_whole_input_validation() {
    let mut scratch = Scratch::default();
    let mut r = Reader::new(br#"{"ok":1,"bad":[1,]}"#, &mut scratch, Limits::default()).unwrap();
    let mut o = r.root().unwrap().object().unwrap();
    {
        let mut f = o.find_next_field("ok").unwrap().unwrap();
        assert_eq!(f.value().unwrap().u64().unwrap(), 1);
    }
    let e = o.finish().unwrap_err();
    assert!(matches!(e.kind, ErrorKind::Syntax(_)));
    assert!(e.offset > 10);
    assert_eq!(r.finish().unwrap_err().kind, ErrorKind::Poisoned);
    drop(r);
    let mut r = Reader::new(b"  true false", &mut scratch, Limits::default()).unwrap();
    assert!(r.root().unwrap().bool().unwrap());
    assert_eq!(r.finish().unwrap_err().offset, 7);
}

#[test]
fn checked_numeric_extraction() {
    fn number<T>(
        bytes: &[u8],
        f: impl FnOnce(&mut hiko_json::Value<'_, '_, '_>) -> hiko_json::Result<T>,
    ) -> hiko_json::Result<T> {
        let mut scratch = Scratch::default();
        let mut r = Reader::new(bytes, &mut scratch, Limits::default())?;
        let value = f(&mut r.root()?)?;
        r.finish()?;
        Ok(value)
    }
    assert_eq!(
        number(b"9223372036854775807", |v| v.i64()).unwrap(),
        i64::MAX
    );
    assert_eq!(
        number(b"-9223372036854775808", |v| v.i64()).unwrap(),
        i64::MIN
    );
    assert_eq!(
        number(b"18446744073709551615", |v| v.u64()).unwrap(),
        u64::MAX
    );
    assert_eq!(
        number(b"9223372036854775809", |v| v.i64())
            .unwrap_err()
            .kind,
        ErrorKind::NumberRange
    );
    assert_eq!(
        number(b"18446744073709551616", |v| v.u64())
            .unwrap_err()
            .kind,
        ErrorKind::NumberRange
    );
    assert_eq!(
        number(b"1.0", |v| v.i64()).unwrap_err().kind,
        ErrorKind::IntegerTokenRequired
    );
    assert_eq!(
        number(b"1e-400", |v| v.f64()).unwrap_err().kind,
        ErrorKind::Underflow
    );
    assert!(number(b"1e400", |v| v.f64()).is_err());
    assert_eq!(
        number(b"-0.0", |v| v.f64()).unwrap().to_bits(),
        (-0.0f64).to_bits()
    );
    assert_eq!(number(b"0e100", |v| v.f64()).unwrap(), 0.0);
}

#[test]
fn fixed_count_shared_profile_differential_and_generated_bytes() {
    let mut state = 0x264u64;
    let alphabet = b"{}[],:\"\\0129truefalsn \t\n\rxyz";
    for _ in 0..4096 {
        let mut bytes = Vec::new();
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let len = (state as usize) % 129;
        for _ in 0..len {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            bytes.push(alphabet[state as usize % alphabet.len()]);
        }
        assert_eq!(
            validate(&bytes),
            serde_json::from_slice::<serde_json::Value>(&bytes).is_ok(),
            "{bytes:?}"
        );
    }
    // Fixed valid nested/common-numeric inputs, not merely mostly-invalid bytes.
    for n in 0..128 {
        let source = format!(r#"{{"items":[{n},false,null,"é🙂"],"obj":{{"a":1,"a":2}}}}"#);
        assert!(validate(source.as_bytes()));
        assert!(serde_json::from_str::<serde_json::Value>(&source).is_ok());
    }
}

#[test]
fn frozen_extra_reader_inputs() {
    let cases: &[(&str, &[u8], bool)] = &[
        (
            "extra_nested",
            &[
                0x7b, 0x22, 0x75, 0x73, 0x65, 0x72, 0x22, 0x3a, 0x7b, 0x22, 0x6e, 0x61, 0x6d, 0x65,
                0x22, 0x3a, 0x22, 0x41, 0x64, 0x61, 0x22, 0x2c, 0x22, 0x61, 0x67, 0x65, 0x22, 0x3a,
                0x33, 0x37, 0x7d, 0x2c, 0x22, 0x65, 0x76, 0x65, 0x6e, 0x74, 0x73, 0x22, 0x3a, 0x5b,
                0x31, 0x30, 0x2c, 0x32, 0x30, 0x2c, 0x33, 0x30, 0x5d, 0x7d,
            ],
            true,
        ),
        (
            "extra_duplicates",
            &[
                0x7b, 0x22, 0x61, 0x22, 0x3a, 0x31, 0x2c, 0x22, 0x61, 0x22, 0x3a, 0x32, 0x2c, 0x22,
                0x61, 0x2e, 0x62, 0x22, 0x3a, 0x6e, 0x75, 0x6c, 0x6c, 0x2c, 0x22, 0x22, 0x3a, 0x22,
                0x22, 0x7d,
            ],
            true,
        ),
        (
            "extra_late_error",
            &[
                0x7b, 0x22, 0x6f, 0x6b, 0x22, 0x3a, 0x31, 0x2c, 0x22, 0x62, 0x61, 0x64, 0x22, 0x3a,
                0x5b, 0x31, 0x2c, 0x5d, 0x7d,
            ],
            false,
        ),
        (
            "extra_suffix",
            &[0x5b, 0x31, 0x5d, 0x20, 0x66, 0x61, 0x6c, 0x73, 0x65],
            false,
        ),
        (
            "extra_escaped",
            &[
                0x7b, 0x22, 0x61, 0x5c, 0x75, 0x30, 0x30, 0x32, 0x65, 0x62, 0x22, 0x3a, 0x22, 0x5c,
                0x75, 0x44, 0x38, 0x33, 0x44, 0x5c, 0x75, 0x44, 0x45, 0x34, 0x32, 0x22, 0x2c, 0x22,
                0x63, 0x68, 0x69, 0x6c, 0x64, 0x22, 0x3a, 0x7b, 0x22, 0x6f, 0x74, 0x68, 0x65, 0x72,
                0x22, 0x3a, 0x30, 0x7d, 0x7d,
            ],
            true,
        ),
    ];
    for &(name, bytes, accepted) in cases {
        println!(
            "input {name} {}",
            bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
        );
        assert_eq!(validate(bytes), accepted, "{name}");
    }
}

#[test]
fn invalid_value_byte_is_syntax_and_minus_zero_is_u64() {
    use jiter::{JiterErrorType, JsonErrorType};
    for bytes in [&b"[@]"[..], b"{\"a\":x}", b"@"] {
        let mut scratch = Scratch::default();
        let mut r = Reader::new(bytes, &mut scratch, Limits::default()).unwrap();
        assert_eq!(
            r.finish().unwrap_err().kind,
            ErrorKind::Syntax(JiterErrorType::JsonError(JsonErrorType::ExpectedSomeValue)),
            "{bytes:?}"
        );
    }
    let mut scratch = Scratch::default();
    let mut r = Reader::new(b"-0", &mut scratch, Limits::default()).unwrap();
    assert_eq!(r.root().unwrap().u64().unwrap(), 0);
    r.finish().unwrap();
}
