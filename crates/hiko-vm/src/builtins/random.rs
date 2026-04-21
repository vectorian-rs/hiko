use super::*;
use smallvec::smallvec;

pub(super) fn random_bytes(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    match &args[0] {
        Value::Int(n) if *n >= 0 => {
            let buf = dryoc::rng::randombytes_buf(*n as usize);
            heap_alloc(heap, HeapObject::Bytes(buf))
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
    let mut output = Vec::with_capacity(n);
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
    let bytes_val = heap_alloc(heap, HeapObject::Bytes(output))?;
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
    let (word, new_state) = pcg_next(state, inc);
    let value = (word as i64).abs() % bound;
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
