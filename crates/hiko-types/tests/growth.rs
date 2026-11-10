//! Type checking must stay close to linear in the size of the program: input
//! is untrusted, and a quadratic pass turns a large program into a denial of
//! service (or, under `max_type_work`, into a rejected program).
//!
//! Each case type-checks generated programs of size N and 2N and requires
//! the type work, which is deterministic, to grow by at most `MAX_RATIO`.
//! Linear growth gives 2; quadratic gives 4. The work of an empty program
//! (builtins, prelude) is subtracted first, so a fixed cost cannot hide a
//! quadratic term.
//!
//! `type_work` counts inference walks, pattern usefulness/matrix work and
//! expanded alias copying. Other passes (for example scope lookups) remain
//! outside this counter; this is not proof that every compiler pass is linear.

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
    assert_linear_at(name, N, generate);
}

fn assert_linear_at(name: &str, n: usize, generate: impl Fn(usize) -> String) {
    let base = type_work("");
    let small = type_work(&generate(n)) - base;
    let large = type_work(&generate(2 * n)) - base;
    let ratio = large as f64 / small as f64;
    assert!(
        ratio <= MAX_RATIO,
        "{name}: type work grew {ratio:.2}x from N={n} ({small}) to N={} ({large})",
        2 * n
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

#[test]
fn many_nullary_alias_copies() {
    assert_linear("nullary alias copies", |n| {
        let mut source = String::from("type atom = int * int\n");
        for i in 0..n {
            source.push_str(&format!("type a{i} = atom\n"));
        }
        source
    });
}

#[test]
fn many_parameterized_alias_copies() {
    assert_linear("parameterized alias copies", |n| {
        let mut source = String::from("type 'a dup = 'a * 'a\n");
        for i in 0..n {
            source.push_str(&format!("type a{i} = int dup\n"));
        }
        source
    });
}

/// `fun f params = let <prelude> <binding 1> ... <binding n> in result end`.
fn let_chain(
    params: &str,
    prelude: &str,
    result: &str,
    n: usize,
    binding: impl Fn(usize) -> String,
) -> String {
    let mut s = format!("fun id x = x\nfun f {params} =\n let\n{prelude}");
    for i in 1..=n {
        s.push_str(&binding(i));
        s.push('\n');
    }
    s.push_str(&format!(" in {result} end\n"));
    s
}

/// Many local values whose types share one free type variable. Walking the
/// environment at each generalization made this cubic; binding the older of
/// two unified variables to the newer one made it quadratic, through a chain
/// of substitutions every lookup walked.
#[test]
fn many_local_values_sharing_a_type_variable() {
    assert_linear("local values sharing a type variable", |n| {
        let_chain("y", "", "y", n, |i| format!("  val u{i} = y"))
    });
}

#[test]
fn many_local_values_each_copying_the_previous() {
    assert_linear("local values chained", |n| {
        let_chain("y", "  val a0 = y\n", "a0", n, |i| {
            format!("  val a{i} = a{}", i - 1)
        })
    });
}

#[test]
fn many_local_values_passed_through_a_function() {
    assert_linear("local values through calls", |n| {
        let_chain("y", "  val b0 = y\n", "b0", n, |i| {
            format!("  val b{i} = id b{}", i - 1)
        })
    });
}

#[test]
fn many_local_tuples_swapping_their_components() {
    assert_linear("local tuples swapped", |n| {
        let_chain(
            "(y, z)",
            "  val c0 = y\n  val d0 = z\n",
            "(c0, d0)",
            n,
            |i| format!("  val (c{i}, d{i}) = (d{}, c{})", i - 1, i - 1),
        )
    });
}
