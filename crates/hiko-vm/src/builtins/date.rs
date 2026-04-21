use super::*;
use jiff::fmt::{strtime, temporal::DateTimeParser};
use jiff::{Timestamp, Zoned, tz::Offset, tz::TimeZone};
use smallvec::smallvec;

static TEMPORAL_PARSER: DateTimeParser = DateTimeParser::new();

fn alloc_string(heap: &mut Heap, text: impl Into<String>) -> Result<Value, String> {
    heap_alloc(heap, HeapObject::String(text.into()))
}

fn make_bool_string_pair(
    heap: &mut Heap,
    ok: bool,
    text: impl Into<String>,
) -> Result<Value, String> {
    let text_value = alloc_string(heap, text)?;
    heap_alloc(
        heap,
        HeapObject::Tuple(smallvec![Value::Bool(ok), text_value,]),
    )
}

fn extract_string_value(value: Value, heap: &Heap, name: &str) -> Result<String, String> {
    match value {
        Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => Ok(s.clone()),
            _ => Err(format!("{name}: expected String")),
        },
        _ => Err(format!("{name}: expected String")),
    }
}

fn extract_int_value(value: Value, name: &str) -> Result<i64, String> {
    match value {
        Value::Int(n) => Ok(n),
        _ => Err(format!("{name}: expected Int")),
    }
}

fn extract_string_pair(
    args: &[Value],
    heap: &Heap,
    name: &str,
) -> Result<(String, String), String> {
    match args.first().copied() {
        Some(Value::Heap(r)) => match heap.get(r).map_err(|e| e.to_string())? {
            HeapObject::Tuple(t) if t.len() >= 2 => Ok((
                extract_string_value(t[0], heap, name)?,
                extract_string_value(t[1], heap, name)?,
            )),
            _ => Err(format!("{name}: expected (String, String)")),
        },
        _ => Err(format!("{name}: expected (String, String)")),
    }
}

fn extract_int_string_pair(
    args: &[Value],
    heap: &Heap,
    name: &str,
) -> Result<(i64, String), String> {
    match args.first().copied() {
        Some(Value::Heap(r)) => match heap.get(r).map_err(|e| e.to_string())? {
            HeapObject::Tuple(t) if t.len() >= 2 => Ok((
                extract_int_value(t[0], name)?,
                extract_string_value(t[1], heap, name)?,
            )),
            _ => Err(format!("{name}: expected (Int, String)")),
        },
        _ => Err(format!("{name}: expected (Int, String)")),
    }
}

fn parse_offset_minutes(text: &str) -> Result<i32, String> {
    if text.eq_ignore_ascii_case("z") {
        return Ok(0);
    }

    let (sign, rest) = match text.as_bytes().first().copied() {
        Some(b'+') => (1, &text[1..]),
        Some(b'-') => (-1, &text[1..]),
        _ => return Err(format!("invalid offset '{text}'")),
    };

    let (hours, minutes) = if let Some((h, m)) = rest.split_once(':') {
        let hours = h
            .parse::<i32>()
            .map_err(|_| format!("invalid offset '{text}'"))?;
        let minutes = m
            .parse::<i32>()
            .map_err(|_| format!("invalid offset '{text}'"))?;
        (hours, minutes)
    } else if rest.len() == 2 {
        let hours = rest
            .parse::<i32>()
            .map_err(|_| format!("invalid offset '{text}'"))?;
        (hours, 0)
    } else if rest.len() == 4 {
        let hours = rest[0..2]
            .parse::<i32>()
            .map_err(|_| format!("invalid offset '{text}'"))?;
        let minutes = rest[2..4]
            .parse::<i32>()
            .map_err(|_| format!("invalid offset '{text}'"))?;
        (hours, minutes)
    } else {
        return Err(format!("invalid offset '{text}'"));
    };

    if !(0..=59).contains(&minutes) {
        return Err(format!("invalid offset '{text}'"));
    }

    Ok(sign * (hours * 60 + minutes))
}

fn format_offset_minutes(minutes: i32) -> String {
    let sign = if minutes < 0 { '-' } else { '+' };
    let abs = minutes.abs();
    let hours = abs / 60;
    let mins = abs % 60;
    format!("{sign}{hours:02}:{mins:02}")
}

fn offset_from_minutes(minutes: i32) -> Result<Offset, String> {
    Offset::from_seconds(minutes.saturating_mul(60))
        .map_err(|e| format!("invalid fixed offset {minutes} minutes: {e}"))
}

fn canonical_offset_string(offset: Offset) -> String {
    let offset_seconds = offset.seconds();
    if offset_seconds % 60 == 0 {
        format_offset_minutes(offset_seconds / 60)
    } else {
        offset.to_string()
    }
}

fn canonical_timezone_from_time_zone(tz: &TimeZone) -> Result<String, String> {
    if tz.is_unknown() {
        return Err("unknown system timezone".into());
    }

    if let Some(name) = tz.iana_name() {
        return Ok(name.to_string());
    }

    Ok(canonical_offset_string(
        Timestamp::now().to_zoned(tz.clone()).offset(),
    ))
}

fn parse_timezone_canonical(text: &str) -> Result<TimeZone, String> {
    if text == "UTC" {
        return Ok(TimeZone::UTC);
    }

    if text.starts_with('+') || text.starts_with('-') {
        return Ok(TimeZone::fixed(offset_from_minutes(parse_offset_minutes(
            text,
        )?)?));
    }

    TimeZone::get(text).map_err(|e| format!("invalid timezone '{text}': {e}"))
}

fn parse_date_canonical(text: &str, name: &str) -> Result<Zoned, String> {
    text.parse::<Zoned>().map_err(|e| format!("{name}: {e}"))
}

fn extract_date_arg(args: &[Value], heap: &Heap, name: &str) -> Result<Zoned, String> {
    let text = extract_string_arg(args, heap, name)?;
    parse_date_canonical(&text, name)
}

fn extract_timezone_arg(args: &[Value], heap: &Heap, name: &str) -> Result<TimeZone, String> {
    let text = extract_string_arg(args, heap, name)?;
    parse_timezone_canonical(&text)
}

fn extract_rfc3339_offset(input: &str) -> Result<TimeZone, String> {
    if input.ends_with('Z') || input.ends_with('z') {
        return Ok(TimeZone::UTC);
    }

    let marker = input
        .rfind(['T', 't', ' '])
        .ok_or_else(|| "missing datetime separator in RFC3339 string".to_string())?;
    let tail = &input[marker + 1..];
    let plus = tail.rfind('+');
    let minus = tail.rfind('-');
    let offset_start = match (plus, minus) {
        (Some(p), Some(m)) => marker + 1 + p.max(m),
        (Some(p), None) => marker + 1 + p,
        (None, Some(m)) => marker + 1 + m,
        (None, None) => {
            return Err("missing UTC offset in RFC3339 string".into());
        }
    };

    let offset_text = &input[offset_start..];
    Ok(TimeZone::fixed(offset_from_minutes(parse_offset_minutes(
        offset_text,
    )?)?))
}

pub(super) fn utc_tz(_args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    alloc_string(heap, "UTC")
}

pub(super) fn local_tz(_args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    match canonical_timezone_from_time_zone(&TimeZone::system()) {
        Ok(text) => make_bool_string_pair(heap, true, text),
        Err(_) => make_bool_string_pair(heap, false, ""),
    }
}

pub(super) fn timezone_of(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let name = extract_string_arg(args, heap, "date_timezone_of")?;
    match TimeZone::get(&name) {
        Ok(tz) => make_bool_string_pair(heap, true, canonical_timezone_from_time_zone(&tz)?),
        Err(_) => make_bool_string_pair(heap, false, ""),
    }
}

pub(super) fn fixed_offset(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let minutes = extract_int_value(args[0], "date_fixed_offset")?;
    let minutes = i32::try_from(minutes)
        .map_err(|_| format!("date_fixed_offset: offset out of range: {minutes}"))?;
    let _ = offset_from_minutes(minutes)?;
    alloc_string(heap, format_offset_minutes(minutes))
}

pub(super) fn utc_now(_args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    alloc_string(heap, Timestamp::now().to_zoned(TimeZone::UTC).to_string())
}

pub(super) fn now_in(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let tz = extract_timezone_arg(args, heap, "date_now_in")?;
    alloc_string(heap, Timestamp::now().to_zoned(tz).to_string())
}

pub(super) fn from_instant(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let (epoch_ms, tz_text) = extract_int_string_pair(args, heap, "date_from_instant")?;
    let ts =
        Timestamp::from_millisecond(epoch_ms).map_err(|e| format!("date_from_instant: {e}"))?;
    let tz = parse_timezone_canonical(&tz_text)?;
    alloc_string(heap, ts.to_zoned(tz).to_string())
}

pub(super) fn to_epoch_ms(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let zoned = extract_date_arg(args, heap, "date_to_epoch_ms")?;
    Ok(Value::Int(zoned.timestamp().as_millisecond()))
}

pub(super) fn to_timezone(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let zoned = extract_date_arg(args, heap, "date_to_timezone")?;
    alloc_string(
        heap,
        zoned
            .time_zone()
            .iana_name()
            .map(ToString::to_string)
            .unwrap_or_else(|| canonical_offset_string(zoned.offset())),
    )
}

pub(super) fn in_timezone(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let (date_text, tz_text) = extract_string_pair(args, heap, "date_in_timezone")?;
    let zoned = parse_date_canonical(&date_text, "date_in_timezone")?;
    let tz = parse_timezone_canonical(&tz_text)?;
    alloc_string(heap, zoned.with_time_zone(tz).to_string())
}

pub(super) fn year(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    Ok(Value::Int(
        extract_date_arg(args, heap, "date_year")?.year().into(),
    ))
}

pub(super) fn month(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    Ok(Value::Int(
        extract_date_arg(args, heap, "date_month")?.month().into(),
    ))
}

pub(super) fn day(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    Ok(Value::Int(
        extract_date_arg(args, heap, "date_day")?.day().into(),
    ))
}

pub(super) fn hour(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    Ok(Value::Int(
        extract_date_arg(args, heap, "date_hour")?.hour().into(),
    ))
}

pub(super) fn minute(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    Ok(Value::Int(
        extract_date_arg(args, heap, "date_minute")?.minute().into(),
    ))
}

pub(super) fn second(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    Ok(Value::Int(
        extract_date_arg(args, heap, "date_second")?.second().into(),
    ))
}

pub(super) fn millisecond(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    Ok(Value::Int(
        extract_date_arg(args, heap, "date_millisecond")?
            .millisecond()
            .into(),
    ))
}

pub(super) fn weekday(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let weekday = extract_date_arg(args, heap, "date_weekday")?
        .weekday()
        .to_monday_one_offset();
    Ok(Value::Int(i64::from(weekday)))
}

pub(super) fn to_rfc3339(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let zoned = extract_date_arg(args, heap, "date_to_rfc3339")?;
    let text = strtime::format("%Y-%m-%dT%H:%M:%S%.3f%:z", &zoned)
        .map_err(|e| format!("date_to_rfc3339: {e}"))?;
    alloc_string(heap, text)
}

pub(super) fn to_rfc2822(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let zoned = extract_date_arg(args, heap, "date_to_rfc2822")?;
    let text = strtime::format("%a, %d %b %Y %H:%M:%S %z", &zoned)
        .map_err(|e| format!("date_to_rfc2822: {e}"))?;
    alloc_string(heap, text)
}

pub(super) fn format(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let (pattern, date_text) = extract_string_pair(args, heap, "date_format")?;
    let zoned = parse_date_canonical(&date_text, "date_format")?;
    let text = strtime::format(pattern, &zoned).map_err(|e| format!("date_format: {e}"))?;
    alloc_string(heap, text)
}

pub(super) fn parse_rfc3339(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let input = extract_string_arg(args, heap, "date_parse_rfc3339")?;
    match TEMPORAL_PARSER.parse_timestamp(&input) {
        Ok(timestamp) => match extract_rfc3339_offset(&input) {
            Ok(tz) => make_bool_string_pair(heap, true, timestamp.to_zoned(tz).to_string()),
            Err(_) => make_bool_string_pair(heap, false, ""),
        },
        Err(_) => make_bool_string_pair(heap, false, ""),
    }
}

pub(super) fn parse_rfc9557(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let input = extract_string_arg(args, heap, "date_parse_rfc9557")?;
    match input.parse::<Zoned>() {
        Ok(zoned) => make_bool_string_pair(heap, true, zoned.to_string()),
        Err(_) => make_bool_string_pair(heap, false, ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heap_string(value: Value, heap: &Heap) -> String {
        match value {
            Value::Heap(r) => match heap.get(r).unwrap() {
                HeapObject::String(text) => text.clone(),
                other => panic!("expected string, got {other:?}"),
            },
            other => panic!("expected heap string, got {other:?}"),
        }
    }

    fn heap_bool_string_pair(value: Value, heap: &Heap) -> (bool, String) {
        match value {
            Value::Heap(r) => match heap.get(r).unwrap() {
                HeapObject::Tuple(items) if items.len() == 2 => {
                    let ok = match items[0] {
                        Value::Bool(ok) => ok,
                        other => panic!("expected bool, got {other:?}"),
                    };
                    (ok, heap_string(items[1], heap))
                }
                other => panic!("expected tuple pair, got {other:?}"),
            },
            other => panic!("expected tuple pair, got {other:?}"),
        }
    }

    fn string_arg(heap: &mut Heap, text: &str) -> Value {
        alloc_string(heap, text).unwrap()
    }

    fn tuple2(heap: &mut Heap, left: Value, right: Value) -> Value {
        heap_alloc(heap, HeapObject::Tuple(smallvec![left, right])).unwrap()
    }

    #[test]
    fn timezone_lookup_accepts_budapest() {
        let mut heap = Heap::new();
        let result = timezone_of(&[string_arg(&mut heap, "Europe/Budapest")], &mut heap).unwrap();
        let (ok, value) = heap_bool_string_pair(result, &heap);
        assert!(ok);
        assert_eq!(value, "Europe/Budapest");
    }

    #[test]
    fn timezone_lookup_rejects_unknown_name() {
        let mut heap = Heap::new();
        let result = timezone_of(&[string_arg(&mut heap, "Mars/Olympus")], &mut heap).unwrap();
        let (ok, value) = heap_bool_string_pair(result, &heap);
        assert!(!ok);
        assert!(value.is_empty());
    }

    #[test]
    fn rfc9557_round_trip_preserves_zone_name() {
        let mut heap = Heap::new();
        let parsed = parse_rfc9557(
            &[string_arg(
                &mut heap,
                "2026-04-19T14:23:07+02:00[Europe/Budapest]",
            )],
            &mut heap,
        )
        .unwrap();
        let (ok, canonical) = heap_bool_string_pair(parsed, &heap);
        assert!(ok);
        assert!(canonical.contains("[Europe/Budapest]"));
        let tz = to_timezone(&[string_arg(&mut heap, &canonical)], &mut heap).unwrap();
        assert_eq!(heap_string(tz, &heap), "Europe/Budapest");
    }

    #[test]
    fn rfc3339_round_trip_preserves_fixed_offset() {
        let mut heap = Heap::new();
        let parsed = parse_rfc3339(
            &[string_arg(&mut heap, "2026-04-19T14:23:07+02:00")],
            &mut heap,
        )
        .unwrap();
        let (ok, canonical) = heap_bool_string_pair(parsed, &heap);
        assert!(ok);
        assert!(canonical.contains("[+02:00]"));
        let tz = to_timezone(&[string_arg(&mut heap, &canonical)], &mut heap).unwrap();
        assert_eq!(heap_string(tz, &heap), "+02:00");
    }

    #[test]
    fn timezone_conversion_preserves_epoch() {
        let mut heap = Heap::new();
        let source = "2026-04-19T14:23:07+02:00[Europe/Budapest]";
        let source_arg = string_arg(&mut heap, source);
        let utc_arg = string_arg(&mut heap, "UTC");
        let source_and_tz = tuple2(&mut heap, source_arg, utc_arg);
        let converted = in_timezone(&[source_and_tz], &mut heap).unwrap();
        let source_ms = to_epoch_ms(&[string_arg(&mut heap, source)], &mut heap).unwrap();
        let converted_text = heap_string(converted, &heap);
        let converted_arg = string_arg(&mut heap, &converted_text);
        let converted_ms = to_epoch_ms(&[converted_arg], &mut heap).unwrap();
        match (source_ms, converted_ms) {
            (Value::Int(source_ms), Value::Int(converted_ms)) => {
                assert_eq!(source_ms, converted_ms)
            }
            other => panic!("expected Ints, got {other:?}"),
        }
    }

    #[test]
    fn fixed_offset_formats_canonically() {
        let mut heap = Heap::new();
        let text = fixed_offset(&[Value::Int(90)], &mut heap).unwrap();
        assert_eq!(heap_string(text, &heap), "+01:30");
    }
}
