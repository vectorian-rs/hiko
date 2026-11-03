//! Type checking must stay close to linear in the size of the program: input
//! is untrusted, and a quadratic pass turns a large program into a denial of
//! service (or, under `max_type_work`, into a rejected program).
//!
//! Each case type-checks generated programs of size N and 2N and requires
//! the type work, which is deterministic, to grow by at most `MAX_RATIO`.
//! Linear growth gives 2; quadratic gives 4.

use hiko_syntax::lexer::Lexer;
use hiko_syntax::parser::Parser;
use hiko_types::infer::InferCtx;

const N: usize = 800;
const MAX_RATIO: f64 = 2.5;

fn type_work(source: &str) -> u64 {
    let tokens = Lexer::new(source, 0).tokenize().expect("lex");
    let program = Parser::new(tokens).parse_program().expect("parse");
    let program = hiko_syntax::desugar::desugar_program(program);
    let mut ctx = InferCtx::new();
    ctx.interner = program.interner.clone();
    ctx.set_max_type_work(u64::MAX);
    ctx.infer_program(&program).expect("type checks");
    ctx.type_work()
}

fn assert_linear(name: &str, generate: impl Fn(usize) -> String) {
    let small = type_work(&generate(N));
    let large = type_work(&generate(2 * N));
    let ratio = large as f64 / small as f64;
    assert!(
        ratio <= MAX_RATIO,
        "{name}: type work grew {ratio:.2}x from N={N} ({small}) to N={} ({large})",
        2 * N
    );
}

#[test]
fn many_top_level_functions() {
    assert_linear("top-level functions", |n| {
        (0..n).map(|i| format!("fun f{i} x = x\n")).collect()
    });
}

#[test]
fn many_top_level_values_using_earlier_ones() {
    assert_linear("top-level values", |n| {
        let mut s = String::from("val v0 = 0\n");
        for i in 1..n {
            s.push_str(&format!("val v{i} = v{} + 1\n", i - 1));
        }
        s
    });
}

#[test]
fn many_local_functions_in_one_declaration() {
    assert_linear("local functions", |n| {
        let mut s = String::from("val r =\n let\n");
        for i in 0..n {
            s.push_str(&format!("  fun a{i} x = x + x\n"));
        }
        for i in 0..n {
            s.push_str(&format!("  val u{i} = a{i} 1\n"));
        }
        s.push_str(" in\n  u0\n end\n");
        s
    });
}

#[test]
fn many_constructors_and_matches() {
    assert_linear("datatype and matches", |n| {
        let ctors: Vec<String> = (0..n).map(|i| format!("C{i} of int")).collect();
        let mut s = format!("datatype t = {}\n", ctors.join(" | "));
        for i in 0..n {
            s.push_str(&format!("fun g{i} v = case v of C{i} x => x | _ => 0\n"));
        }
        s
    });
}
