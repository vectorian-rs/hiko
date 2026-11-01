//! Measure what each host-work-charging builtin costs, in opcode-equivalents,
//! next to what it charges.
//!
//! Usage: cargo run --release -p hiko-vm --example host_work_calibration [filter]
//!
//! For every builtin and input size, the harness runs a generated program
//! that calls the builtin K times, and the same program with K = 0. The
//! difference, minus the time of the opcodes the loop executes, is the
//! builtin's own cost. Dividing by the measured time of one opcode expresses
//! it in the unit `max_fuel` uses, which is the unit host work is calibrated
//! to (see docs/architecture/execution-bounds.md).
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use hiko_compile::chunk::CompiledProgram;
use hiko_compile::compiler::Compiler;
use hiko_syntax::lexer::Lexer;
use hiko_syntax::parser::Parser;
use hiko_vm::builder::VMBuilder;
use hiko_vm::vm::RunResult;

struct CountingAllocator {
    allocs: AtomicUsize,
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator {
    allocs: AtomicUsize::new(0),
};

// SAFETY: forwards every call to `System` and only counts allocations.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.allocs.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        self.allocs.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        self.allocs.fetch_add(1, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

const SIZES: [usize; 4] = [16, 1024, 64 * 1024, 1024 * 1024];

/// Bytes processed by the first measurement; K is chosen so that K * size is
/// about this.
const BYTES_PER_MEASUREMENT: usize = 32 * 1024 * 1024;

/// A measurement whose calls take less than this is repeated with more calls,
/// so that run-to-run variation in building the input does not swamp it.
const MIN_MEASURED: Duration = Duration::from_millis(20);
const TARGET_MEASURED: Duration = Duration::from_millis(50);
const MAX_CALLS: usize = 200_000;

/// Inputs a call can use, each with the inputs it is built from. `N` is the
/// number of 16-byte chunks in the input. A program defines only the inputs
/// its call uses, so building unused inputs does not add noise.
const INPUTS: &[(&str, &[&str], &str)] = &[
    (
        "copies",
        &[],
        "fun copies (s, n, acc) = if n = 0 then acc else copies (s, n - 1, s :: acc)",
    ),
    (
        "chunks",
        &["copies"],
        "val chunks = copies (\"abcdefghijklmnop\", N, [])",
    ),
    ("text", &["chunks"], "val text = string_join (chunks, \"\")"),
    (
        "utext",
        &["copies"],
        // 16 bytes of characters whose case mapping is not ASCII.
        "val utext = string_join (copies (\"ßﬃéüßﬃé\", N, []), \"\")",
    ),
    ("bytes", &["text"], "val bytes = string_to_bytes text"),
    (
        "json_array",
        &["copies"],
        // One-character tokens: the densest JSON, and the most expensive per
        // KiB, since parsing cost follows the number of tokens. 8 * N items
        // of 2 bytes make the text about the nominal size.
        "val json_array = \"[\" ^ string_join (copies (\"1\", 8 * N, []), \",\") ^ \"]\"",
    ),
    (
        "json_object",
        &["json_array"],
        "val json_object = \"{\\\"k\\\":\" ^ json_array ^ \"}\"",
    ),
    (
        "parsed",
        &["json_array"],
        "val parsed = json_parse json_array",
    ),
    (
        "rng",
        &[],
        "val rng = rng_seed (string_to_bytes \"calibration\")",
    ),
];

const LOOP: &str = "fun go (k, acc) = if k = 0 then acc else go (k - 1, let val _ = CALL in acc + 1 end)\nval _ = go (K, 0)";

/// (builtin, call expression). The expression may use the inputs above and
/// `SIZE`.
const CALLS: &[(&str, &str)] = &[
    ("string_length", "string_length text"),
    ("substring", "substring (text, SIZE / 2, 8)"),
    ("string_contains", "string_contains (text, \"zz\")"),
    ("string_contains/needle", "string_contains (\"x\", text)"),
    ("starts_with", "starts_with (text, \"abc\")"),
    ("starts_with/long", "starts_with (text, text)"),
    ("ends_with", "ends_with (text, \"nop\")"),
    ("trim", "trim text"),
    ("to_upper", "to_upper text"),
    ("to_upper/unicode", "to_upper utext"),
    ("to_lower", "to_lower text"),
    ("to_lower/unicode", "to_lower utext"),
    ("split", "split (text, \"p\")"),
    ("string_replace", "string_replace (text, \"a\", \"b\")"),
    (
        "string_replace/grow",
        "string_replace (text, \"a\", \"0123456789abcdef\")",
    ),
    ("string_join", "string_join (chunks, \",\")"),
    ("string_to_bytes", "string_to_bytes text"),
    ("bytes_to_string", "bytes_to_string bytes"),
    ("bytes_length", "bytes_length bytes"),
    ("bytes_get", "bytes_get (bytes, SIZE / 2)"),
    ("bytes_slice", "bytes_slice (bytes, 0, SIZE / 2)"),
    ("blake3", "blake3 bytes"),
    ("regex_match", "regex_match (text, \"z+\")"),
    // No literal to search for, so the matcher runs over every byte.
    ("regex_match/class", "regex_match (text, \"[0-9]+x\")"),
    // A pattern of SIZE bytes: compiling it is the cost.
    ("regex_match/pattern", "regex_match (\"x\", text)"),
    ("regex_replace", "regex_replace (text, \"a\", \"b\")"),
    (
        "regex_replace/grow",
        "regex_replace (text, \"a\", \"0123456789abcdef\")",
    ),
    ("json_length", "json_length json_array"),
    ("json_get", "json_get (json_object, \"k.0\")"),
    ("json_parse", "(json_parse json_array : int list)"),
    ("json_to_string", "json_to_string parsed"),
    ("random_bytes", "random_bytes SIZE"),
    ("rng_bytes", "rng_bytes (rng, SIZE)"),
    ("rng_seed", "rng_seed bytes"),
    ("rng_int", "rng_int (rng, 1000)"),
    ("json_keys", "json_keys json_object"),
];

struct Run {
    time: Duration,
    opcodes: u64,
    host_work: u64,
    allocs: usize,
}

fn compile(source: &str) -> CompiledProgram {
    let tokens = Lexer::new(source, 0).tokenize().expect("lex");
    let program = Parser::new(tokens).parse_program().expect("parse");
    let (compiled, _warnings) = Compiler::compile(program).expect("compile");
    compiled
}

/// Run `program` and return the best of five runs, or the error it fails
/// with (a regex over the compiled-size limit, for example).
fn try_run(program: &CompiledProgram) -> Result<Run, String> {
    let mut best: Option<Run> = None;
    for _ in 0..5 {
        let mut vm = VMBuilder::new(program.clone()).with_all().build();
        vm.set_fuel(u64::MAX / 2);
        let allocs_before = ALLOCATOR.allocs.load(Ordering::Relaxed);
        let start = Instant::now();
        // `run_slice` charges the fuel budget, so the budget counts opcodes.
        match vm.run_slice(u64::MAX / 4) {
            RunResult::Done => {}
            other => return Err(format!("{other:?}")),
        }
        let time = start.elapsed();
        let run = Run {
            time,
            opcodes: vm.fuel_budget().expect("fuel is set").used(),
            host_work: vm.heap().host_work_used(),
            allocs: ALLOCATOR.allocs.load(Ordering::Relaxed) - allocs_before,
        };
        if best.as_ref().is_none_or(|b| run.time < b.time) {
            best = Some(run);
        }
    }
    Ok(best.expect("five runs"))
}

fn run(program: &CompiledProgram) -> Run {
    try_run(program).unwrap_or_else(|e| panic!("program did not finish: {e}"))
}

/// Add `name` and the inputs it is built from to `out`, dependencies first.
fn add_input(name: &str, out: &mut Vec<&'static str>) {
    let (_, deps, def) = INPUTS
        .iter()
        .find(|(n, _, _)| *n == name)
        .expect("known input");
    for dep in *deps {
        add_input(dep, out);
    }
    if !out.contains(def) {
        out.push(def);
    }
}

fn program_for(call: &str, size: usize, k: usize) -> CompiledProgram {
    let mut defs = Vec::new();
    for (name, _, _) in INPUTS {
        let used = call
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .any(|word| word == *name);
        if used {
            add_input(name, &mut defs);
        }
    }
    let mut source = defs.join("\n").replace("N,", &format!("{},", size / 16));
    source.push('\n');
    source.push_str(
        &LOOP
            .replace("CALL", &call.replace("SIZE", &size.to_string()))
            .replace("(K, 0)", &format!("({k}, 0)")),
    );
    compile(&source)
}

/// Time of one opcode, from a loop that does nothing else.
fn nanos_per_opcode() -> f64 {
    let program = compile(
        "fun spin (n, acc) = if n = 0 then acc else spin (n - 1, acc + 1)\n\
         val _ = spin (5000000, 0)",
    );
    let r = run(&program);
    r.time.as_nanos() as f64 / r.opcodes as f64
}

fn main() {
    let filter = std::env::args().nth(1);
    let ns_per_op = nanos_per_opcode();
    println!("one opcode: {ns_per_op:.2} ns");
    println!();
    println!(
        "{:16} {:>8} {:>7} {:>12} {:>10} {:>9} {:>8} {:>11}",
        "builtin", "size", "calls", "cost (ops)", "charged", "ratio", "allocs", "call time"
    );
    for (name, call) in CALLS {
        if filter.as_deref().is_some_and(|f| !name.contains(f)) {
            continue;
        }
        for size in SIZES {
            let base = run(&program_for(call, size, 0));
            let mut k = (BYTES_PER_MEASUREMENT / size).clamp(20, 20_000);
            let mut full = match try_run(&program_for(call, size, 1)) {
                Ok(_) => run(&program_for(call, size, k)),
                Err(e) => {
                    println!("{name:16} {size:>8} fails: {e}");
                    continue;
                }
            };
            let measured = full.time.saturating_sub(base.time);
            if measured < MIN_MEASURED && k < MAX_CALLS {
                let scale = TARGET_MEASURED.as_secs_f64() / measured.as_secs_f64().max(1e-6);
                k = ((k as f64 * scale) as usize).clamp(k + 1, MAX_CALLS);
                full = run(&program_for(call, size, k));
            }
            let per_call = |a: f64, b: f64| (a - b) / k as f64;
            let ns = per_call(full.time.as_nanos() as f64, base.time.as_nanos() as f64);
            let loop_ops = per_call(full.opcodes as f64, base.opcodes as f64);
            let native_ops = (ns / ns_per_op - loop_ops).max(0.0);
            let charged = per_call(full.host_work as f64, base.host_work as f64);
            let allocs = per_call(full.allocs as f64, base.allocs as f64);
            let ratio = if charged > 0.0 {
                format!("{:.1}x", native_ops / charged)
            } else {
                "-".to_string()
            };
            println!(
                "{name:16} {size:>8} {k:>7} {native_ops:>12.0} {charged:>10.0} {ratio:>9} {allocs:>8.1} {:>11}",
                format!("{:.1?}", Duration::from_nanos(ns.max(0.0) as u64))
            );
        }
    }
    println!();
    println!(
        "cost: native time per call in opcode-equivalents, excluding the loop's own opcodes.\n\
         ratio: cost / charged; above 1 means the builtin is undercharged."
    );
}
