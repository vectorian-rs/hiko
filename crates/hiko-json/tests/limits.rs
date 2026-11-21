use hiko_json::{ErrorKind, Limits, Reader, Scratch};

#[test]
fn admission_refuses_before_allocating_and_scratch_remains_reusable() {
    let mut scratch = Scratch::default();
    for (limits, kind) in [
        (
            Limits {
                max_input_bytes: 0,
                ..Limits::default()
            },
            ErrorKind::InputLimit,
        ),
        (
            Limits {
                max_scratch_bytes: 0,
                ..Limits::default()
            },
            ErrorKind::ScratchLimit,
        ),
        (
            Limits {
                max_work: 0,
                ..Limits::default()
            },
            ErrorKind::WorkLimit,
        ),
        (
            Limits {
                max_depth: 129,
                ..Limits::default()
            },
            ErrorKind::DepthLimit,
        ),
    ] {
        let e = Reader::new(b"[1]", &mut scratch, limits).err().unwrap();
        assert_eq!(e.kind, kind);
        assert_eq!(scratch.capacities(), (0, 0));
    }
    assert_eq!(
        Reader::new(b"\"\xff\"", &mut scratch, Limits::default())
            .err()
            .unwrap()
            .kind,
        ErrorKind::InvalidUtf8
    );
    assert_eq!(scratch.capacities(), (0, 0));
    Reader::new(b"[1]", &mut scratch, Limits::default())
        .unwrap()
        .finish()
        .unwrap();
    let old = scratch.capacities();
    let e = Reader::new(
        &vec![b' '; 20000],
        &mut scratch,
        Limits {
            max_scratch_bytes: 1,
            ..Limits::default()
        },
    )
    .err()
    .unwrap();
    assert_eq!(e.kind, ErrorKind::ScratchLimit);
    assert_eq!(scratch.capacities(), old);
    Reader::new(b"true", &mut scratch, Limits::default())
        .unwrap()
        .finish()
        .unwrap();
}

#[test]
fn work_is_charged_before_each_backend_invocation() {
    let mut scratch = Scratch::default();
    let bytes = b"[1]";
    let mut r = Reader::new(
        bytes,
        &mut scratch,
        Limits {
            max_work: 32 * bytes.len() as u64 + 256 + 1,
            ..Limits::default()
        },
    )
    .unwrap();
    assert_eq!(r.finish().unwrap_err().kind, ErrorKind::WorkLimit);
    assert_eq!(r.stats().backend_calls, 1);
    assert!(r.stats().advanced_bytes <= 1);
}

#[test]
fn global_depth_boundary_is_checked_for_skip_and_lending_traversal() {
    for depth in [127, 128, 129] {
        let source = format!("{}0{}", "[".repeat(depth), "]".repeat(depth));
        println!(
            "input depth_{depth} {}",
            source
                .as_bytes()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        );
        for opened in [false, true] {
            let mut scratch = Scratch::default();
            let mut r = Reader::new(source.as_bytes(), &mut scratch, Limits::default()).unwrap();
            let result = if opened {
                r.root().unwrap().array().unwrap().finish()
            } else {
                r.finish()
            };
            assert_eq!(result.is_ok(), depth <= 128, "{depth} opened={opened}");
            if let Err(e) = result {
                assert_eq!(e.kind, ErrorKind::DepthLimit);
                assert_eq!(e.offset, 128);
            }
        }
    }
}

#[test]
fn decoded_scratch_never_grows_behind_the_reservation() {
    let mut scratch = Scratch::default();
    for n in [9000, 20000, 9000] {
        let source = format!(r#"{{"{}":"{}"}}"#, "\\u0061".repeat(n), "\\u0062".repeat(n));
        let before = scratch.capacities();
        let mut r = Reader::new(source.as_bytes(), &mut scratch, Limits::default()).unwrap();
        {
            let mut o = r.root().unwrap().object().unwrap();
            {
                let mut f = o.next_field().unwrap().unwrap();
                assert_eq!(f.key().len(), n);
                assert_eq!(f.value().unwrap().string().unwrap().len(), n);
            }
            o.finish().unwrap();
        }
        r.finish().unwrap();
        let peak = r.stats().peak_scratch_bytes;
        drop(r);
        let after = scratch.capacities();
        assert_eq!(
            after,
            (before.0.max(source.len()), before.1.max(source.len()))
        );
        assert!(peak >= scratch.retained_bytes());
        println!(
            "scratch n={n} input={} before={before:?} after={after:?} estimated_peak={peak}",
            source.len()
        );
    }
}

#[test]
fn frozen_dense_growth_is_one_forward_traversal() {
    for n in [64, 128, 256] {
        for object in [false, true] {
            let source = if object {
                format!(
                    "{{{}}}",
                    (0..n)
                        .map(|i| format!(r#""k{i}":"v""#))
                        .collect::<Vec<_>>()
                        .join(",")
                )
            } else {
                format!("[{}]", vec!["0"; n].join(","))
            };
            let name = if object { "object" } else { "array" };
            println!(
                "input dense_{name}_{n} {}",
                source
                    .as_bytes()
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
            );
            let mut scratch = Scratch::default();
            for _ in 0..2 {
                let mut r =
                    Reader::new(source.as_bytes(), &mut scratch, Limits::default()).unwrap();
                let mut count = 0;
                if object {
                    let mut o = r.root().unwrap().object().unwrap();
                    while let Some(f) = o.next_field().unwrap() {
                        f.finish().unwrap();
                        count += 1;
                    }
                    o.finish().unwrap();
                } else {
                    let mut a = r.root().unwrap().array().unwrap();
                    while let Some(mut v) = a.next_value().unwrap() {
                        assert_eq!(v.u64().unwrap(), 0);
                        count += 1;
                    }
                    a.finish().unwrap();
                }
                assert_eq!(count, n);
                r.finish().unwrap();
                let stats = r.stats();
                assert_eq!(stats.advanced_bytes, source.len());
                assert!(stats.backend_calls <= 16 * n as u64 + 16);
                println!(
                    "growth n={n} object={object} calls={} advanced={} work={} estimated_peak={}",
                    stats.backend_calls,
                    stats.advanced_bytes,
                    stats.charged_work,
                    stats.peak_scratch_bytes
                );
            }
        }
    }
}

#[test]
fn replacement_peak_includes_old_and_new_buffers() {
    let mut scratch = Scratch::default();
    let fixed;
    {
        let mut r = Reader::new(b"[0]", &mut scratch, Limits::default()).unwrap();
        fixed = r.stats().peak_scratch_bytes - 6;
        r.finish().unwrap();
    }
    let old = scratch.capacities();
    let input = vec![b' '; 64];
    // Enough for steady state, NOT for old key buffer + new buffers overlap.
    let limits = Limits {
        max_scratch_bytes: fixed + 128,
        ..Limits::default()
    };
    assert_eq!(
        Reader::new(&input, &mut scratch, limits)
            .err()
            .unwrap()
            .kind,
        ErrorKind::ScratchLimit
    );
    assert_eq!(scratch.capacities(), old);
}

#[test]
fn float_conversion_has_its_own_pre_call_work_charge() {
    let mut scratch = Scratch::default();
    let mut r = Reader::new(
        b"1.0",
        &mut scratch,
        Limits {
            max_work: 32 * 3 + 256 + 2,
            ..Limits::default()
        },
    )
    .unwrap();
    assert_eq!(
        r.root().unwrap().f64().unwrap_err().kind,
        ErrorKind::WorkLimit
    );
    assert_eq!(r.stats().backend_calls, 2);
}
