use super::*;
use smallvec::smallvec;

// Host work in opcode-equivalents, from the host_work_calibration example
// (docs/builtins/accounting-matrix.md#calibration).
/// `random_bytes` reads the operating system's generator.
const RANDOM_BYTES_BASE_HOST_WORK: u64 = 105;
const RANDOM_BYTES_PER_KIB_HOST_WORK: u64 = 421;
const RNG_BYTES_BASE_HOST_WORK: u64 = 15;
const RNG_BYTES_PER_KIB_HOST_WORK: u64 = 57;
const RNG_SEED_HOST_WORK: u64 = 5;
const RNG_SEED_PER_KIB_HOST_WORK: u64 = 140;
const RNG_INT_HOST_WORK: u64 = 5;

/// Reserve an output buffer of `n` bytes, failing with an error instead of
/// aborting when `n` exceeds the memory limit or cannot be allocated.
fn reserve_output(heap: &Heap, n: usize, name: &str) -> Result<Vec<u8>, String> {
    heap.ensure_can_allocate_bytes(size_of::<HeapObject>().saturating_add(n))
        .map_err(|e| format!("{name}: {e}"))?;
    let mut buf = Vec::new();
    buf.try_reserve_exact(n)
        .map_err(|_| format!("{name}: cannot allocate {n} bytes"))?;
    Ok(buf)
}

pub(crate) fn entries() -> &'static [(&'static str, BuiltinFn)] {
    &[
        ("random_bytes", random_bytes as BuiltinFn),
        ("rng_seed", rng_seed),
        ("rng_bytes", rng_bytes),
        ("rng_int", rng_int),
    ]
}

pub(super) fn random_bytes(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Int(n) if *n >= 0 => {
            let n = *n as usize;
            charge(
                heap,
                "random_bytes",
                host_work(
                    RANDOM_BYTES_BASE_HOST_WORK,
                    RANDOM_BYTES_PER_KIB_HOST_WORK,
                    n,
                ),
            )?;
            let mut buf = reserve_output(heap, n, "random_bytes")?;
            buf.resize(n, 0);
            dryoc::rng::copy_randombytes(&mut buf);
            heap_alloc(heap, HeapObject::Bytes(buf.into()))
        }
        Value::Int(_) => Err("random_bytes: length must be non-negative".into()),
        _ => Err("random_bytes: expected Int".into()),
    }
}

fn pcg_next(state: u64, inc: u64) -> (u32, u64) {
    let old_state = state;
    let new_state = old_state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(inc);
    let xorshifted = (((old_state >> 18) ^ old_state) >> 27) as u32;
    let rot = (old_state >> 59) as u32;
    (xorshifted.rotate_right(rot), new_state)
}

pub(super) fn rng_seed(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let seed_bytes = match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::Bytes(b) => b.clone(),
            _ => return Err("rng_seed: expected Bytes".into()),
        },
        _ => return Err("rng_seed: expected Bytes".into()),
    };
    charge(
        heap,
        "rng_seed",
        host_work(
            RNG_SEED_HOST_WORK,
            RNG_SEED_PER_KIB_HOST_WORK,
            seed_bytes.len(),
        ),
    )?;
    let mut state: u64 = 0;
    let mut inc: u64 = 1;
    for (i, &b) in seed_bytes.iter().enumerate() {
        if i % 2 == 0 {
            state = state.wrapping_mul(31).wrapping_add(b as u64);
        } else {
            inc = inc.wrapping_mul(37).wrapping_add(b as u64);
        }
    }
    inc |= 1;
    let (_, state) = pcg_next(state, inc);
    let (_, state) = pcg_next(state, inc);
    heap_alloc(heap, HeapObject::Rng { state, inc })
}

pub(super) fn rng_bytes(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let (v0, v1) = match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::Tuple(t) if t.len() >= 2 => (t[0], t[1]),
            _ => return Err("rng_bytes: expected (rng, Int)".into()),
        },
        _ => return Err("rng_bytes: expected (rng, Int)".into()),
    };
    let (mut state, inc) = match v0 {
        Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
            HeapObject::Rng { state, inc } => (*state, *inc),
            _ => return Err("rng_bytes: expected rng".into()),
        },
        _ => return Err("rng_bytes: expected rng".into()),
    };
    let n = match v1 {
        Value::Int(n) if n >= 0 => n as usize,
        _ => return Err("rng_bytes: expected non-negative Int".into()),
    };
    charge(
        heap,
        "rng_bytes",
        host_work(RNG_BYTES_BASE_HOST_WORK, RNG_BYTES_PER_KIB_HOST_WORK, n),
    )?;
    let mut output = reserve_output(heap, n, "rng_bytes")?;
    while output.len() < n {
        let (word, new_state) = pcg_next(state, inc);
        state = new_state;
        for &b in &word.to_le_bytes() {
            if output.len() >= n {
                break;
            }
            output.push(b);
        }
    }
    let bytes_val = heap_alloc(heap, HeapObject::Bytes(output.into()))?;
    let rng_val = heap_alloc(heap, HeapObject::Rng { state, inc })?;
    heap_alloc(heap, HeapObject::Tuple(smallvec![bytes_val, rng_val]))
}

pub(super) fn rng_int(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let (v0, v1) = match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::Tuple(t) if t.len() >= 2 => (t[0], t[1]),
            _ => return Err("rng_int: expected (rng, Int)".into()),
        },
        _ => return Err("rng_int: expected (rng, Int)".into()),
    };
    let (state, inc) = match v0 {
        Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
            HeapObject::Rng { state, inc } => (*state, *inc),
            _ => return Err("rng_int: expected rng".into()),
        },
        _ => return Err("rng_int: expected rng".into()),
    };
    let bound = match v1 {
        Value::Int(n) if n > 0 => n,
        _ => return Err("rng_int: bound must be positive".into()),
    };
    charge(heap, "rng_int", RNG_INT_HOST_WORK)?;
    let (word, new_state) = pcg_next(state, inc);
    let value = i64::from(word) % bound;
    let rng_val = heap_alloc(
        heap,
        HeapObject::Rng {
            state: new_state,
            inc,
        },
    )?;
    heap_alloc(
        heap,
        HeapObject::Tuple(smallvec![Value::Int(value), rng_val]),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_bytes_respects_host_work_limit() {
        let mut heap = Heap::new();
        heap.set_max_host_work(RANDOM_BYTES_BASE_HOST_WORK - 1);
        let result = random_bytes(&[Value::Int(1)], &mut heap);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("host work budget exceeded"));
    }

    #[test]
    fn random_bytes_charges_host_work() {
        let mut heap = Heap::new();
        random_bytes(&[Value::Int(1)], &mut heap).unwrap();
        assert!(heap.host_work_used() >= RANDOM_BYTES_BASE_HOST_WORK);
    }

    #[test]
    fn random_bytes_checks_memory_limit_before_allocating() {
        let mut heap = Heap::new();
        heap.set_max_bytes(1024);
        let err = random_bytes(&[Value::Int(1 << 20)], &mut heap).unwrap_err();
        assert!(err.contains("random_bytes"), "{err}");
    }

    #[test]
    fn random_bytes_huge_length_is_an_error_not_an_abort() {
        let mut heap = Heap::new();
        let err = random_bytes(&[Value::Int(i64::MAX)], &mut heap).unwrap_err();
        assert!(err.contains("cannot allocate"), "{err}");
    }

    #[test]
    fn rng_bytes_huge_length_is_an_error_not_an_abort() {
        let mut heap = Heap::new();
        let seed = heap_alloc(&mut heap, HeapObject::Bytes(vec![1, 2, 3].into())).unwrap();
        let rng = rng_seed(&[seed], &mut heap).unwrap();
        let args = heap_alloc(
            &mut heap,
            HeapObject::Tuple(smallvec![rng, Value::Int(i64::MAX)]),
        )
        .unwrap();
        let err = rng_bytes(&[args], &mut heap).unwrap_err();
        assert!(err.contains("cannot allocate"), "{err}");
    }
}
