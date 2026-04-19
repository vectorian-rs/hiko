use super::*;
use std::sync::OnceLock;

pub(super) fn epoch(_args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| format!("epoch: {e}"))?
        .as_secs();
    Ok(Value::Int(secs as i64))
}

pub(super) fn epoch_ms(_args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| format!("epoch_ms: {e}"))?
        .as_millis();
    Ok(Value::Int(ms as i64))
}

static MONO_ORIGIN: OnceLock<std::time::Instant> = OnceLock::new();

pub(super) fn monotonic_ms(_args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    let origin = MONO_ORIGIN.get_or_init(std::time::Instant::now);
    Ok(Value::Int(origin.elapsed().as_millis() as i64))
}

pub(super) fn sleep(args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Int(ms) if *ms >= 0 => {
            std::thread::sleep(std::time::Duration::from_millis(*ms as u64));
            Ok(Value::Unit)
        }
        _ => Err("sleep: expected non-negative Int (milliseconds)".into()),
    }
}
