use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use hiko_compile::compiler::Compiler;
use hiko_syntax::lexer::Lexer;
use hiko_syntax::parser::Parser;

use hiko_compile::chunk::CompiledProgram;

fn compile_source(src: &str) -> CompiledProgram {
    let tokens = Lexer::new(src, 0).tokenize().expect("lex error");
    let program = Parser::new(tokens).parse_program().expect("parse error");
    let (compiled, _warnings) = Compiler::compile(program).expect("compile error");
    compiled
}

// ── VM creation ─────────────────────────────────────────────────────

fn bench_vm_creation(c: &mut Criterion) {
    let compiled = compile_source("val x = 1");
    c.bench_function("vm_creation", |b| {
        b.iter(|| {
            let _ = hiko_vm::vm::VM::new(compiled.clone());
        });
    });
}

// ── Fibonacci ───────────────────────────────────────────────────────

fn bench_fibonacci(c: &mut Criterion) {
    let mut group = c.benchmark_group("fibonacci");
    for n in [10, 15, 20] {
        let src = format!(
            "fun fib n = if n < 2 then n else fib (n - 1) + fib (n - 2)\n\
             val result = fib {n}"
        );
        let compiled = compile_source(&src);
        group.bench_with_input(BenchmarkId::new("fib", n), &compiled, |b, compiled| {
            b.iter(|| {
                let mut vm = hiko_vm::vm::VM::new(compiled.clone());
                vm.run().unwrap();
            });
        });
    }
    group.finish();
}

// ── List operations ─────────────────────────────────────────────────

fn bench_list_ops(c: &mut Criterion) {
    let mut group = c.benchmark_group("list_ops");
    for n in [100, 500, 1000] {
        let src = format!(
            "fun range n = if n = 0 then [] else n :: range (n - 1)\n\
             fun sum xs = case xs of [] => 0 | x :: rest => x + sum rest\n\
             val result = sum (range {n})"
        );
        let compiled = compile_source(&src);
        group.bench_with_input(
            BenchmarkId::new("sum_range", n),
            &compiled,
            |b, compiled| {
                b.iter(|| {
                    let mut vm = hiko_vm::vm::VM::new(compiled.clone());
                    vm.run().unwrap();
                });
            },
        );
    }
    group.finish();
}

// ── Pattern matching ────────────────────────────────────────────────

fn bench_pattern_matching(c: &mut Criterion) {
    let mut group = c.benchmark_group("pattern_matching");
    for n in [100, 500, 1000] {
        let src = format!(
            "datatype 'a option = None | Some of 'a\n\
             fun map_option f opt = case opt of\n\
                 None => None\n\
               | Some x => Some (f x)\n\
             fun count n = if n = 0 then None else map_option (fn x => x + 1) (count (n - 1))\n\
             val result = count {n}"
        );
        let compiled = compile_source(&src);
        group.bench_with_input(
            BenchmarkId::new("adt_dispatch", n),
            &compiled,
            |b, compiled| {
                b.iter(|| {
                    let mut vm = hiko_vm::vm::VM::new(compiled.clone());
                    vm.run().unwrap();
                });
            },
        );
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_vm_creation,
    bench_fibonacci,
    bench_list_ops,
    bench_pattern_matching
);
criterion_main!(benches);
