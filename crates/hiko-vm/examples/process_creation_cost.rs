use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use hiko_compile::chunk::CompiledProgram;
use hiko_compile::compiler::Compiler;
use hiko_syntax::lexer::Lexer;
use hiko_syntax::parser::Parser;
use hiko_vm::runtime_ops::create_child_vm_from_parent;
use hiko_vm::sendable::{SendableValue, serialize};
use hiko_vm::value::{HeapObject, Value};
use hiko_vm::vm::VM;

struct CountingAllocator {
    alloc_calls: AtomicUsize,
    realloc_calls: AtomicUsize,
    dealloc_calls: AtomicUsize,
    allocated_bytes: AtomicUsize,
}

impl CountingAllocator {
    const fn new() -> Self {
        Self {
            alloc_calls: AtomicUsize::new(0),
            realloc_calls: AtomicUsize::new(0),
            dealloc_calls: AtomicUsize::new(0),
            allocated_bytes: AtomicUsize::new(0),
        }
    }

    fn snapshot(&self) -> AllocationSnapshot {
        AllocationSnapshot {
            alloc_calls: self.alloc_calls.load(Ordering::Relaxed),
            realloc_calls: self.realloc_calls.load(Ordering::Relaxed),
            dealloc_calls: self.dealloc_calls.load(Ordering::Relaxed),
            allocated_bytes: self.allocated_bytes.load(Ordering::Relaxed),
        }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator::new();

// SAFETY: This forwards allocation operations to `System` while keeping best-
// effort counters for a single-process benchmark binary.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.alloc_calls.fetch_add(1, Ordering::Relaxed);
        self.allocated_bytes
            .fetch_add(layout.size(), Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        self.alloc_calls.fetch_add(1, Ordering::Relaxed);
        self.allocated_bytes
            .fetch_add(layout.size(), Ordering::Relaxed);
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        self.realloc_calls.fetch_add(1, Ordering::Relaxed);
        let additional = new_size.saturating_sub(layout.size());
        self.allocated_bytes
            .fetch_add(additional, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        self.dealloc_calls.fetch_add(1, Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[derive(Clone, Copy)]
struct AllocationSnapshot {
    alloc_calls: usize,
    realloc_calls: usize,
    dealloc_calls: usize,
    allocated_bytes: usize,
}

impl std::ops::Sub for AllocationSnapshot {
    type Output = AllocationSnapshot;

    fn sub(self, rhs: Self) -> Self::Output {
        Self::Output {
            alloc_calls: self.alloc_calls.saturating_sub(rhs.alloc_calls),
            realloc_calls: self.realloc_calls.saturating_sub(rhs.realloc_calls),
            dealloc_calls: self.dealloc_calls.saturating_sub(rhs.dealloc_calls),
            allocated_bytes: self.allocated_bytes.saturating_sub(rhs.allocated_bytes),
        }
    }
}

fn compile_program(source: &str) -> CompiledProgram {
    let tokens = Lexer::new(source, 0).tokenize().expect("lex");
    let program = Parser::new(tokens).parse_program().expect("parse");
    let (compiled, _warnings) = Compiler::compile(program).expect("compile");
    compiled
}

fn resolve_global_closure(program: &CompiledProgram, name: &str) -> (usize, Vec<SendableValue>) {
    let mut vm = VM::new(program.clone());
    vm.run()
        .unwrap_or_else(|err| panic!("failed to initialize benchmark globals: {}", err.message));

    let closure = *vm
        .get_global(name)
        .unwrap_or_else(|| panic!("benchmark program is missing global closure '{name}'"));
    let closure_ref = match closure {
        Value::Heap(r) => r,
        other => panic!("global '{name}' is not a closure: {other:?}"),
    };

    match vm
        .heap()
        .get(closure_ref)
        .unwrap_or_else(|err| panic!("global '{name}' points to invalid heap object: {err}"))
    {
        HeapObject::Closure {
            proto_idx,
            captures,
        } => (
            *proto_idx,
            captures
                .iter()
                .copied()
                .map(|value| {
                    serialize(value, vm.heap())
                        .unwrap_or_else(|err| panic!("capture for '{name}' is not sendable: {err}"))
                })
                .collect(),
        ),
        other => panic!("global '{name}' is not a closure heap object: {other:?}"),
    }
}

fn benchmark<T>(name: &str, iterations: usize, mut op: impl FnMut() -> T) {
    for _ in 0..128 {
        black_box(op());
    }

    let before = ALLOCATOR.snapshot();
    let start = Instant::now();
    for _ in 0..iterations {
        black_box(op());
    }
    let elapsed = start.elapsed();
    let delta = ALLOCATOR.snapshot() - before;

    let ns_per_op = elapsed.as_nanos() / iterations as u128;
    let allocs_per_op = delta.alloc_calls as f64 / iterations as f64;
    let reallocs_per_op = delta.realloc_calls as f64 / iterations as f64;
    let deallocs_per_op = delta.dealloc_calls as f64 / iterations as f64;
    let bytes_per_op = delta.allocated_bytes as f64 / iterations as f64;

    println!(
        "{name:30} {ns_per_op:>8} ns/op  {allocs_per_op:>6.2} alloc  {reallocs_per_op:>6.2} realloc  {deallocs_per_op:>6.2} free  {bytes_per_op:>8.1} B/op"
    );
}

fn main() {
    let program = compile_program(
        "val entry0 = fn () => 41
         val a = 1
         val b = 2
         val c = true
         val d = \"cap\"
         val entry4 = fn () => if c then if d = \"cap\" then a + b else 0 else 0",
    );
    let (entry0_proto_idx, empty_captures) = resolve_global_closure(&program, "entry0");
    let (entry4_proto_idx, small_captures) = resolve_global_closure(&program, "entry4");
    assert!(
        empty_captures.is_empty(),
        "entry0 benchmark closure should not capture values"
    );
    assert_eq!(
        small_captures.len(),
        4,
        "entry4 benchmark closure should capture exactly four values"
    );

    let parent_vm = VM::new(program);

    let iterations = 20_000;
    println!(
        "Measuring hiko process creation costs over {iterations} iterations.\n\
         This excludes scheduler/table insertion and focuses on VM creation."
    );
    println!();

    benchmark("VM::create_child", iterations, || parent_vm.create_child());
    benchmark("spawn path (0 captures)", iterations, || {
        create_child_vm_from_parent(&parent_vm, entry0_proto_idx, empty_captures.clone())
            .expect("child")
    });
    benchmark("spawn path (4 captures)", iterations, || {
        create_child_vm_from_parent(&parent_vm, entry4_proto_idx, small_captures.clone())
            .expect("child")
    });
}
