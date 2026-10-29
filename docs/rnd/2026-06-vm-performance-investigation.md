# Performance Investigation: Filesystem I/O overhead and VM opcode density

## Summary

Running `examples/benchmark.hml` (walk `.git`, blake3 hash 3381 files, print results) is about **2.2× slower** than an equivalent Python script (215 ms vs 97 ms). Through microbenchmarks we isolated the bottleneck and identified **two distinct issues**:

1. **I/O policy layer**: per-file `fs::canonicalize()` syscalls and `cap_std` capability checks add **~36 μs/file** of overhead (3× baseline `std::fs::read`).
2. **VM bytecode density**: list pattern matching emits **30–50% more bytecode** than necessary because `GetLocal` and `GetField` carry full operands, and common ML idioms (Cons test, head/tail extract) are not fused.

Both are addressable without changing the language semantics.

---

## Reproduction

```bash
# 1. Build release
cargo build --release

# 2. Original benchmark
hyperfine -N --warmup 3 './target/release/hiko-cli run examples/benchmark.hml'

# 3. Microbenchmarks (see files added in this investigation)
hyperfine -N --warmup 3 './target/release/hiko-cli run examples/benchmark_walk_only.hml'
hyperfine -N --warmup 3 './target/release/hiko-cli run examples/benchmark_read_only.hml'
hyperfine -N --warmup 3 './target/release/hiko-cli run examples/benchmark_print_only.hml'
```

### Results

| Benchmark | Mean (ms) | User | System | Note |
|---|---|---|---|---|
| Original (`benchmark.hml`) | **215** | 51 | 164 | walk + read + hash + print |
| `benchmark_walk_only.hml` | **25.5** | 10.7 | 13.6 | Walk alone |
| `benchmark_read_only.hml` | **211.6** | 43.3 | **167.7** | Walk + read + hash (no print) |
| Python equivalent | **97** | 44 | 53 | walk + read + hash + print |

The system-time gap is the smoking gun: Hiko spends ~168 ms in kernel space vs Python’s ~53 ms.

---

## Issue 1: Filesystem I/O overhead (the 2× factor)

### Finding: `cap_relative_path_for` calls `canonicalize()` on every absolute path

In `crates/hiko-vm/src/heap.rs` (~line 758):

```rust
fn cap_relative_path_for(root: &Path, path: &str) -> Result<Option<PathBuf>, String> {
    let path = Path::new(path);
    if path.is_absolute() {
        let resolved = canonicalize_with_missing_tail(path)?;
        // ... strip prefix
    }
    // ...
}
```

`Filesystem.walk` returns **absolute paths** (built from `canonicalize(".git")`). Every `read_file_bytes` on those paths triggers `canonicalize_with_missing_tail`, which calls `std::fs::canonicalize` — a multi-syscall `realpath()` equivalent.

**Isolated with a Rust microbenchmark** (`io_bench.rs`):

| Approach | μs/file | Equivalent |
|---|---|---|
| `std::fs::read(absolute)` | **12.4** | Baseline: open + read + close |
| `cap_std::Dir::read(relative)` | **34.1** | Capability-bounded read |
| `canonicalize` + `cap_std::read` | **48.6** | **Hiko’s actual hot path** |

**Breakdown of Hiko’s per-file overhead:**
- Pure read cost: 12.4 μs
- `cap_std` direction checks: +21.7 μs
- `canonicalize()` per absolute path: +14.5 μs
- **Total: 48.6 μs vs Python’s ~12 μs baseline**

### Confirmation with Hiko-level benchmark

Created `examples/benchmark_read_relative.hml` which strips the absolute prefix before calling `read_bytes`. Result: **176 ms vs 198 ms** (absolute) — a **22 ms win** from avoiding `canonicalize()` alone.

### Recommendations for I/O

1. **Skip canonicalize for paths already under a known cap root.** If `Filesystem.walk` returns paths under an already-canonicalized root, subsequent `read_file_bytes` on those paths should strip the prefix directly without re-canonicalizing.
2. **Cache cap decisions for walk output.** Walk produces a closed namespace of paths all beneath the same root. The VM should mark these (or cache the relative form) so `cap_candidates_for` does not re-verify every file.
3. **(Evaluative) Replace `cap_std` per-call validation with a fast prefix check for read-only paths.** For read-only workloads under trusted configs, `std::fs::read` after one `path.starts_with(root)` check matches Python speed. Keep `cap_std` for writes and untrusted paths.

---

## Issue 2: VM opcode density (the ML-family bytecode tax)

### Finding: `GetLocal` and pattern matching bytecode is bloated for ML idioms

Hiko currently has **60 opcodes**. That is a defensible number, but the **bytecode emitted for list operations** is verbose compared to OCaml/Lua because common patterns are not fused.

### Concrete example: `case xs of _ :: rest => ...`

From `crates/hiko-compile/src/compiler.rs` (`compile_pattern_test` + `compile_pattern_bind`):

**Test phase for Cons** (current codegen):

```
GetLocal    xs_slot         ; 3 bytes
GetTag                      ; 1 byte
Const       1               ; 3 bytes   (tag for Cons)
Eq                          ; 1 byte
JumpIfFalse → fail          ; 3 bytes

GetLocal    xs_slot         ; 3 bytes   (extract head for sub-test)
GetField    0               ; 2 bytes
... (test head, then Pop) ...

GetLocal    xs_slot         ; 3 bytes   (extract tail for sub-test)
GetField    1               ; 2 bytes
... (test tail, then Pop) ...
```

**Bind phase for Cons** (current codegen):

```
GetLocal    xs_slot         ; 3 bytes   (extract head AGAIN)
GetField    0               ; 2 bytes
add_local   hd

GetLocal    xs_slot         ; 3 bytes   (extract tail AGAIN)
GetField    1               ; 2 bytes
add_local   tl
```

**Total: ~37 bytes and ~18 dispatches** per recursive Cons step.

### What optimized VMs do

| VM | Technique | Opcode size |
|---|---|---|
| OCaml | `ACC0`–`ACC7` (load local 0–7 in **1 byte**) | No operand |
| Lua 5.4 | Register machine; `ADDI A B sC` in 4 bytes | 1 dispatch |
| OCaml | `GETFIELD0`, `GETFIELD1` (load field in **1 byte**) | No operand |

Hiko has none of these. Every local access is `GetLocal + u16(3 bytes)`, every field access is `GetField + u8(2 bytes)`.

### Impact on real code

For `List.length` (a 4-line function), `xs` (slot 0) is accessed **7 times**:
- Tag check (Nil)
- Tag check (Cons)
- Extract head (test)
- Extract tail (test)
- Extract head (bind)
- Extract tail (bind)
- Load `rest` for recursive call

At 3 bytes per `GetLocal` → **21 bytes of `GetLocal` traffic alone** for a trivial function.

With quick-local opcodes (`GetLocal0`–`GetLocal3` as 1-byte opcodes), this becomes **7 bytes**.

### Proposed opcode additions (minimal set, maximum impact)

| New Opcode | Size | Replaces | Savings |
|---|---|---|---|
| `GetLocal0`–`GetLocal3` | 1 byte | `GetLocal` + 2-byte operand | 2 bytes/op |
| `SetLocal0`–`SetLocal3` | 1 byte | `SetLocal` + 2-byte operand | 2 bytes/op |
| `GetField0`–`GetField1` | 1 byte | `GetField` + 1-byte operand | 1 byte/op |
| `MakeCons` | 3 bytes | `MakeData(tag=1, arity=2)` | 1 byte + constant-pool hit |

Total: **~12 new opcodes**.

### Compiler changes required

1. **`emit_get_var`** — fast path for slots 0–3:
   ```rust
   match slot {
       0 => emit(Op::GetLocal0),
       1 => emit(Op::GetLocal1),
       // ...
       _ => { emit(Op::GetLocal); emit_u16(slot); }
   }
   ```

2. **`emit_field_extract`** — use `GetField0`/`GetField1` when index is 0 or 1:
   ```rust
   if idx == 0 { emit(Op::GetField0); }
   else if idx == 1 { emit(Op::GetField1); }
   else { emit(Op::GetField); emit_u8(idx); }
   ```

3. **`compile_expr` for `Cons`** — emit `MakeCons` instead of `MakeData`:
   ```rust
   ExprKind::Cons(head, tail) => {
       compile_expr(head)?;
       compile_expr(tail)?;
       emit(Op::MakeCons);   // 1 byte, implicit tag=1, arity=2
   }
   ```

### VM dispatch changes required

In `crates/hiko-vm/src/vm/dispatch.rs`, each new opcode is one match arm:

```rust
Op::GetLocal0 => self.push(self.stack[self.frames[fi].base])?,
Op::GetLocal1 => self.push(self.stack[self.frames[fi].base + 1])?,
// ...
Op::GetField0 => {
    let val = self.pop()?;
    match val {
        Value::Heap(r) => {
            match self.heap_get(r)? {
                HeapObject::Tuple(t) => self.push(t[0])?,
                HeapObject::Data { fields, .. } => self.push(fields[0])?,
                _ => // error...
            }
        }
        _ => // error...
    }
}
// ...
```

These are **simpler than the generic versions** because there is no operand to decode.

---

## Expected impact

| Change | Where it helps | Estimated impact |
|---|---|---|
| Fix `canonicalize()` in `cap_relative_path_for` | Every `read_file_bytes` with absolute path | **~22 ms** on 3381 files |
| Bypass `cap_std` for pre-validated read paths | Same + `walk_dir` output | **~50–70 ms** |
| `GetLocal0`–`GetLocal3` + `GetField0`–`GetField1` | Every list recursive function, pattern match | **30–50% smaller bytecode**, fewer dispatches |
| `MakeCons` | Every `::` constructor, list literals | **1 byte + constant pool skip** per cons |

**Combined**: Hiko filesystem benchmark should drop from ~215 ms to **~120–140 ms**, closing most of the gap with Python’s 97 ms.

---

## New files added during investigation

| File | Purpose |
|---|---|
| `examples/benchmark_walk_only.hml` | Isolate `Filesystem.walk` cost |
| `examples/benchmark_read_only.hml` | Isolate read + hash (no print) |
| `examples/benchmark_print_only.hml` | Isolate `println` loop |
| `examples/benchmark_read_relative.hml` | Verify canonicalize hypothesis |

---

## Related code

- `crates/hiko-vm/src/heap.rs` — `cap_relative_path_for`, `canonicalize_with_missing_tail`
- `crates/hiko-vm/src/builtins/filesystem.rs` — `read_file_bytes`, `walk_dir`
- `crates/hiko-vm/src/vm/dispatch.rs` — opcode interpreter
- `crates/hiko-compile/src/compiler.rs` — `emit_get_var`, `emit_field_extract`, `compile_pattern_test`, `compile_pattern_bind`
- `crates/hiko-compile/src/op.rs` — opcode enum

---

## Labels

`performance`, `vm`, `filesystem`, `good first issue` (for the opcode additions), `security` (for cap-std evaluation)
