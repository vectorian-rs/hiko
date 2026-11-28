// Fixed-seed differential against serde_json: generated valid documents are
// fully extracted, mutated ones compared for acceptance. Known policy gaps:
// hiko refuses f64 underflow, and validation does not range-check numbers.
use hiko_json::{Kind, Limits, Reader, Result, Scratch, Value};
use serde_json::{Map, Number, Value as J};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.below(xs.len())]
    }
}

const NUMBERS: &[&str] = &[
    "0",
    "-0",
    "1",
    "-1",
    "37",
    "9223372036854775807",
    "-9223372036854775808",
    "9223372036854775808",
    "18446744073709551615",
    "18446744073709551616",
    "-9223372036854775809",
    "0.0",
    "-0.0",
    "1.5",
    "1e3",
    "1E-3",
    "1e+10",
    "2.5e-308",
    "1e-400",
    "1e308",
    "1.7976931348623157e308",
    "4.9e-324",
    "0e100",
    "123456789012345678901234567890",
    "0.1",
    "3.141592653589793",
    "1.00",
];
const STRING_PIECES: &[&str] = &[
    "a",
    "Z",
    " ",
    "é",
    "🙂",
    "日本",
    "\\\"",
    "\\\\",
    "\\/",
    "\\b",
    "\\f",
    "\\n",
    "\\r",
    "\\t",
    "\\u0000",
    "\\u001f",
    "\\u00e9",
    "\\u20AC",
    "\\ud83d\\ude00",
    "\\uFFFF",
    "x\u{7f}",
    "\u{10ffff}",
];
const WS: &[&str] = &["", "", "", " ", "\n", "\t", "\r\n  "];

fn gen_string(r: &mut Rng, out: &mut String) {
    out.push('"');
    for _ in 0..r.below(6) {
        out.push_str(r.pick(STRING_PIECES));
    }
    out.push('"');
}

fn gen_value(r: &mut Rng, depth: usize, out: &mut String) {
    out.push_str(r.pick(WS));
    let choice = if depth == 0 { r.below(4) } else { r.below(7) };
    match choice {
        0 => out.push_str(r.pick(&["null", "true", "false"])),
        1 | 2 => out.push_str(r.pick(NUMBERS)),
        3 => gen_string(r, out),
        4 | 5 => {
            out.push('{');
            let n = r.below(5);
            for i in 0..n {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(r.pick(WS));
                // Unique keys: serde keeps the last duplicate.
                out.push_str(&format!("\"k{i}"));
                let mut s = String::new();
                gen_string(r, &mut s);
                out.push_str(&s[1..]);
                out.push_str(r.pick(WS));
                out.push(':');
                gen_value(r, depth - 1, out);
            }
            out.push_str(r.pick(WS));
            out.push('}');
        }
        _ => {
            out.push('[');
            let n = r.below(5);
            for i in 0..n {
                if i > 0 {
                    out.push(',');
                }
                gen_value(r, depth - 1, out);
            }
            out.push_str(r.pick(WS));
            out.push(']');
        }
    }
    out.push_str(r.pick(WS));
}

/// Fully walk a value with hiko-json, producing a serde_json value. Numbers
/// follow serde's default: i64/u64 for integer tokens in range, else f64.
fn walk(mut v: Value<'_, '_, '_>) -> Result<J> {
    Ok(match v.kind()? {
        Kind::Null => {
            v.null()?;
            J::Null
        }
        Kind::Boolean => J::Bool(v.bool()?),
        Kind::String => J::String(v.string()?.to_owned()),
        Kind::Number => {
            let text = v.number_text()?;
            // Re-read through a fresh reader to exercise checked extraction.
            number(text)?
        }
        Kind::Array => {
            let mut a = v.array()?;
            let mut items = Vec::new();
            while let Some(item) = a.next_value()? {
                items.push(walk(item)?);
            }
            J::Array(items)
        }
        Kind::Object => {
            let mut o = v.object()?;
            let mut map = Map::new();
            while let Some(mut f) = o.next_field()? {
                let k = f.key().to_owned();
                let val = walk(f.value()?)?;
                map.insert(k, val);
            }
            J::Object(map)
        }
    })
}

fn number(text: &str) -> Result<J> {
    if text == "-0" {
        return Ok(J::Number(Number::from_f64(-0.0).unwrap()));
    }
    let read = |f: &dyn Fn(&mut Value<'_, '_, '_>) -> Result<J>| -> Result<J> {
        let mut scratch = Scratch::default();
        let mut r = Reader::new(text.as_bytes(), &mut scratch, Limits::default())?;
        let j = f(&mut r.root()?)?;
        r.finish()?;
        Ok(j)
    };
    if let Ok(j) = read(&|v| Ok(J::Number(v.i64()?.into()))) {
        return Ok(j);
    }
    if let Ok(j) = read(&|v| Ok(J::Number(v.u64()?.into()))) {
        return Ok(j);
    }
    let f = read(&|v| Ok(J::Number(Number::from_f64(v.f64()?).unwrap())))?;
    let std: f64 = text.parse().unwrap();
    assert_eq!(
        f.as_f64().unwrap().to_bits(),
        std.to_bits(),
        "inexact float {text}"
    );
    Ok(f)
}

fn walk_owned(r: &mut hiko_json::OwnedReader) -> Result<J> {
    Ok(match r.kind()? {
        Kind::Null => {
            r.read_null()?;
            J::Null
        }
        Kind::Boolean => J::Bool(r.read_bool()?),
        Kind::String => J::String(r.read_string()?.to_owned()),
        Kind::Number => number(r.read_number_text()?)?,
        Kind::Array => {
            r.begin_array()?;
            let mut items = Vec::new();
            while r.next_item()? {
                items.push(walk_owned(r)?);
            }
            r.finish_array()?;
            J::Array(items)
        }
        Kind::Object => {
            r.begin_object()?;
            let mut map = Map::new();
            while let Some(key) = r.next_field()? {
                let key = key.to_owned();
                map.insert(key, walk_owned(r)?);
            }
            r.finish_object()?;
            J::Object(map)
        }
    })
}

fn hiko(bytes: &[u8]) -> Result<J> {
    let borrowed = (|| {
        let mut scratch = Scratch::default();
        let mut r = Reader::new(bytes, &mut scratch, Limits::default())?;
        let j = walk(r.root()?)?;
        r.finish()?;
        Ok(j)
    })();
    if let Ok(input) = std::str::from_utf8(bytes) {
        let owned = (|| {
            let mut r = hiko_json::OwnedReader::new(input.into(), Limits::default())?;
            let j = walk_owned(&mut r)?;
            r.finish()?;
            Ok(j)
        })();
        assert_eq!(borrowed, owned, "frontend extraction parity {bytes:?}");
    }
    borrowed
}

fn validate(bytes: &[u8]) -> bool {
    let mut scratch = Scratch::default();
    let borrowed = Reader::new(bytes, &mut scratch, Limits::default()).and_then(|mut r| r.finish());
    if let Ok(input) = std::str::from_utf8(bytes) {
        let owned = hiko_json::OwnedReader::new(input.into(), Limits::default())
            .and_then(|mut r| r.finish());
        assert_eq!(borrowed, owned, "frontend validation parity {bytes:?}");
    }
    borrowed.is_ok()
}

/// Structural equality; floats within 1 ULP (serde's default float parse is
/// not correctly rounded; hiko floats are checked exactly in `number`).
fn same(a: &J, b: &J) -> bool {
    match (a, b) {
        (J::Number(x), J::Number(y)) if x.is_f64() || y.is_f64() => {
            let (x, y) = (x.as_f64().unwrap(), y.as_f64().unwrap());
            x == y || (x.to_bits() as i64 - y.to_bits() as i64).abs() <= 1
        }
        (J::Array(x), J::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(a, b)| same(a, b))
        }
        (J::Object(x), J::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w)))
        }
        _ => a == b,
    }
}

#[test]
fn generated_documents_extract_like_serde() {
    let mut r = Rng(0x267);
    let (mut agree, mut both_err, mut policy, mut diffs) = (0, 0, 0, Vec::new());
    for _ in 0..2_000 {
        let mut doc = String::new();
        let depth = r.below(6);
        gen_value(&mut r, depth, &mut doc);
        let serde = serde_json::from_str::<J>(&doc);
        let ours = hiko(doc.as_bytes());
        let valid = validate(doc.as_bytes());
        match (&serde, &ours) {
            (Ok(a), Ok(b)) if same(a, b) && valid => agree += 1,
            (Err(_), Err(_)) if !valid => both_err += 1,
            // Documented policy: hiko refuses underflow instead of returning 0.
            (Ok(_), Err(e)) if e.kind == hiko_json::ErrorKind::Underflow && valid => policy += 1,
            _ => diffs.push(format!(
                "{doc:?}\n  serde={serde:?}\n  hiko ={ours:?} validate={valid}"
            )),
        }
    }
    assert!(
        diffs.is_empty(),
        "{}",
        diffs[..diffs.len().min(5)].join("\n")
    );
    assert!(
        agree > 1_500 && policy > 0 && both_err == 0,
        "{agree} {policy} {both_err}"
    );
}

#[test]
fn mutated_documents_accept_like_serde() {
    let mut r = Rng(0x268);
    let alphabet: &[u8] =
        b"{}[],:\"\\/0123456789.eE+-tfn ulsa\t\n\r\x00\x01\x7f\xff\xc3\xa9\xed\xa0\x80";
    let mut diffs = Vec::new();
    let (mut accepted, mut range_policy) = (0, 0);
    for _ in 0..2_000 {
        let mut doc = String::new();
        let depth = 1 + r.below(5);
        gen_value(&mut r, depth, &mut doc);
        let mut bytes = doc.into_bytes();
        for _ in 0..1 + r.below(2) {
            let i = r.below(bytes.len() + 1);
            match r.below(3) {
                0 if i < bytes.len() => bytes[i] = *r.pick(alphabet),
                1 if i < bytes.len() => {
                    bytes.remove(i);
                }
                _ => bytes.insert(i, *r.pick(alphabet)),
            }
        }
        let serde_result = serde_json::from_slice::<J>(&bytes);
        let range = serde_result
            .as_ref()
            .is_err_and(|e| e.to_string().starts_with("number out of range"));
        let serde = serde_result.is_ok();
        let ours = validate(&bytes);
        accepted += serde as usize;
        if range && ours {
            range_policy += 1;
        } else if serde != ours {
            diffs.push(format!(
                "{:?} serde={serde} hiko={ours}",
                String::from_utf8_lossy(&bytes)
            ));
        }
    }
    assert!(
        diffs.is_empty(),
        "{}",
        diffs[..diffs.len().min(5)].join("\n")
    );
    assert!(
        accepted > 200 && range_policy > 0,
        "{accepted} {range_policy}"
    );
}

#[test]
fn edge_cases_match_serde() {
    let cases: &[&[u8]] = &[
        b"\"\\\x75d800\"",
        b"\"\\\x75dc00\"",
        b"\"\\\x75d800\\\x750041\"",
        b"\"\\\x75d83d\\\x75de00\"",
        b"{\"\\\x75d800\":1}",
        br#""\x""#,
        b"\"\\\x7512\"",
        b"\"a\x01\"",
        b"01",
        b"1.",
        b"-",
        b"+1",
        b".5",
        b"[1,]",
        b"{\"a\":1,}",
        b"[1 2]",
        b"\xef\xbb\xbf1",
        b"",
        b"  ",
        b"nul",
        b"truee",
        b"\"\xc3\"",
        b"\"\xed\xa0\x80\"",
        b"1 /* c */",
    ];
    for &bytes in cases {
        let serde = serde_json::from_slice::<J>(bytes).ok();
        let name = String::from_utf8_lossy(bytes);
        assert_eq!(validate(bytes), serde.is_some(), "{name:?}");
        assert_eq!(hiko(bytes).ok(), serde, "{name:?}");
    }
}
