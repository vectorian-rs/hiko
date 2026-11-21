use hiko_json::{ErrorKind, Limits, Reader, Scratch};

#[test]
fn dropped_unconsumed_children_poison_without_hidden_skip() {
    let mut scratch = Scratch::default();
    for mode in 0..4 {
        let mut r = Reader::new(br#"{"a":[1,2]}"#, &mut scratch, Limits::default()).unwrap();
        match mode {
            0 => drop(r.root().unwrap()),
            1 => drop(r.root().unwrap().object().unwrap()),
            2 => {
                let mut o = r.root().unwrap().object().unwrap();
                drop(o.next_field().unwrap().unwrap());
                assert!(o.next_field().is_err());
            }
            _ => {
                let mut o = r.root().unwrap().object().unwrap();
                let mut f = o.next_field().unwrap().unwrap();
                let mut a = f.value().unwrap().array().unwrap();
                drop(a.next_value().unwrap().unwrap());
                assert!(a.next_value().is_err());
            }
        }
        let position = r.stats().advanced_bytes;
        assert_eq!(r.finish().unwrap_err().kind, ErrorKind::Poisoned);
        assert_eq!(r.stats().advanced_bytes, position);
    }
    Reader::new(b"null", &mut scratch, Limits::default())
        .unwrap()
        .finish()
        .unwrap();
}

#[test]
fn wrong_type_and_double_consumption_poison_but_buffers_are_reusable() {
    let mut scratch = Scratch::default();
    {
        let mut r = Reader::new(b"1", &mut scratch, Limits::default()).unwrap();
        {
            let mut v = r.root().unwrap();
            assert_eq!(v.u64().unwrap(), 1);
            assert_eq!(v.u64().unwrap_err().kind, ErrorKind::Consumed);
        }
        assert_eq!(r.finish().unwrap_err().kind, ErrorKind::Poisoned);
    }
    {
        let mut r = Reader::new(b"false", &mut scratch, Limits::default()).unwrap();
        assert!(r.root().unwrap().string().is_err());
        assert_eq!(r.finish().unwrap_err().kind, ErrorKind::Poisoned);
    }
    Reader::new(b"[]", &mut scratch, Limits::default())
        .unwrap()
        .finish()
        .unwrap();
}

#[test]
fn raw_number_borrows_input_not_reusable_decoder_scratch() {
    let input = b"1.00";
    let mut scratch = Scratch::default();
    let n;
    {
        let mut r = Reader::new(input, &mut scratch, Limits::default()).unwrap();
        n = r.root().unwrap().number_text().unwrap();
        r.finish().unwrap();
    }
    Reader::new(br#""\u0061""#, &mut scratch, Limits::default())
        .unwrap()
        .finish()
        .unwrap();
    assert_eq!(n, "1.00");
}

#[test]
fn forgotten_children_cannot_bypass_completion_checks() {
    let mut scratch = Scratch::default();
    {
        let mut r = Reader::new(b"[1,2]", &mut scratch, Limits::default()).unwrap();
        let mut a = r.root().unwrap().array().unwrap();
        std::mem::forget(a.next_value().unwrap().unwrap());
        assert_eq!(
            a.next_value().err().unwrap().kind,
            ErrorKind::UnconsumedChild
        );
    }
    Reader::new(b"[]", &mut scratch, Limits::default())
        .unwrap()
        .finish()
        .unwrap();
}

#[test]
fn mismatched_numeric_values_never_enter_backend_number_any() {
    for mode in 0..3 {
        let mut scratch = Scratch::default();
        let mut r = Reader::new(b"18446744073709551616", &mut scratch, Limits::default()).unwrap();
        let error = {
            let mut v = r.root().unwrap();
            match mode {
                0 => v.string().map(|_| ()).unwrap_err(),
                1 => v.bool().map(|_| ()).unwrap_err(),
                _ => v.null().unwrap_err(),
            }
        };
        assert_eq!(error.kind, ErrorKind::WrongKind);
        assert_eq!(error.offset, 0);
        assert_eq!(r.stats().backend_calls, 1);
        assert_eq!(r.stats().advanced_bytes, 0);
    }
}
