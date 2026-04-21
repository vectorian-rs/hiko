use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};
use hiko_syntax::lexer::Lexer;
use hiko_syntax::parser::Parser;
use hiko_syntax::pretty::pretty_program;

fn generate_program(n_decls: usize) -> String {
    let mut out = String::new();
    for i in 0..n_decls {
        out.push_str(&format!("val x{i} = {i}\n"));
        out.push_str(&format!("fun f{i} a = a + {i}\n"));
    }
    out
}

const SIZES: &[(usize, &str)] = &[(5, "small"), (50, "medium"), (500, "large")];

fn bench_lexer(c: &mut Criterion) {
    let mut group = c.benchmark_group("lexer");
    for &(n, label) in SIZES {
        let src = generate_program(n);
        group.bench_with_input(BenchmarkId::new("tokenize", label), &src, |b, src| {
            b.iter(|| {
                Lexer::new(src, 0).tokenize().unwrap();
            });
        });
    }
    group.finish();
}

fn bench_parser(c: &mut Criterion) {
    let mut group = c.benchmark_group("parser");
    for &(n, label) in SIZES {
        let src = generate_program(n);
        let tokens = Lexer::new(&src, 0).tokenize().unwrap();
        group.bench_function(BenchmarkId::new("parse_program", label), |b| {
            b.iter_batched(
                || tokens.clone(),
                |tokens| Parser::new(tokens).parse_program().unwrap(),
                BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

fn bench_pretty(c: &mut Criterion) {
    let mut group = c.benchmark_group("pretty");
    for &(n, label) in SIZES {
        let src = generate_program(n);
        let tokens = Lexer::new(&src, 0).tokenize().unwrap();
        let ast = Parser::new(tokens).parse_program().unwrap();
        group.bench_with_input(BenchmarkId::new("pretty_program", label), &ast, |b, ast| {
            b.iter(|| {
                pretty_program(ast);
            });
        });
    }
    group.finish();
}

fn bench_roundtrip(c: &mut Criterion) {
    let mut group = c.benchmark_group("roundtrip");
    for &(n, label) in SIZES {
        let src = generate_program(n);
        group.bench_with_input(
            BenchmarkId::new("lex_parse_pretty", label),
            &src,
            |b, src| {
                b.iter(|| {
                    let tokens = Lexer::new(src, 0).tokenize().unwrap();
                    let ast = Parser::new(tokens).parse_program().unwrap();
                    pretty_program(&ast);
                });
            },
        );
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_lexer,
    bench_parser,
    bench_pretty,
    bench_roundtrip
);
criterion_main!(benches);
