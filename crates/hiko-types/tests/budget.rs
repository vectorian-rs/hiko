//! Budget regressions run in a kill-and-reap subprocess: a broken checker must
//! not hang the test runner or turn an in-process timer into false evidence.
use hiko_syntax::{lexer::Lexer, parser::Parser};
use hiko_types::infer::{InferCtx, TypeError};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn check(source: &str, limit: u64) -> (u64, Result<(), TypeError>, usize) {
    let tokens = Lexer::new(source, 17).tokenize().expect("lex");
    let program = Parser::new(tokens).parse_program().expect("parse");
    let program = hiko_syntax::desugar::desugar_program(program);
    let mut ctx = InferCtx::new();
    ctx.interner = program.interner.clone();
    ctx.set_max_type_work(limit);
    let result = ctx.infer_program(&program);
    (ctx.type_work(), result, ctx.warnings.len())
}

fn aliases(parameterized: bool, depth: usize) -> String {
    let mut source = String::from("structure Outer = struct\nval value = let\n");
    if parameterized {
        source.push_str("type 'a dup = 'a * 'a\n");
    }
    source.push_str("type t0 = int\n");
    for i in 1..=depth {
        if parameterized {
            source.push_str(&format!("type t{i} = t{} dup\n", i - 1));
        } else {
            source.push_str(&format!("type t{i} = t{} * t{}\n", i - 1, i - 1));
        }
    }
    source.push_str("in () end\nend\n");
    source
}

fn wide_booleans(width: usize) -> String {
    let mut source = String::from("fun wide x = case x of\n");
    for col in 0..width {
        for value in ["true", "false"] {
            let fields: Vec<_> = (0..width)
                .map(|i| if i == col { value } else { "_" })
                .collect();
            if col != 0 || value == "false" {
                source.push_str(" | ");
            }
            source.push_str(&format!("({}) => 0\n", fields.join(",")));
        }
    }
    source
}

#[test]
fn budget_child() {
    let Ok(case) = std::env::var("HIKO_TYPE_BUDGET_CASE") else {
        return;
    };
    match case.as_str() {
        "wide" => {
            let (used, result, _) = check(&wide_booleans(72), 400_000);
            assert!(used <= 400_000);
            if let Err(error) = result {
                assert!(error.message.contains("work limit"), "{error:?}");
                assert!(error.message.contains("pattern matching"), "{error:?}");
                assert_eq!(error.span.file_id, 17);
            }
        }
        "nullary" | "parameterized" => {
            let source = aliases(case == "parameterized", 50);
            let (used, result, _) = check(&source, 20_000);
            let error = result.expect_err("exponential alias must fail");
            assert!(used <= 20_000);
            assert!(error.message.contains("type alias expansion"), "{error:?}");
            assert_eq!(error.span.file_id, 17);
            let failed = &source[error.span.start as usize..error.span.end as usize];
            assert!(
                failed.starts_with('t') || failed.contains("dup"),
                "wrong enclosing span: {failed:?}"
            );
            assert!(!failed.contains("structure"));
        }
        "boundaries" => {
            let source = "type 'a dup = 'a * 'a\ntype t = int dup\nval x : t = (1, 2)";
            let (exact, result, _) = check(source, u64::MAX);
            result.unwrap();
            let (used, result, _) = check(source, exact);
            result.unwrap();
            assert_eq!(used, exact);
            assert!(
                check(source, exact - 1)
                    .1
                    .unwrap_err()
                    .message
                    .contains("work limit")
            );
            assert!(
                check(source, 0)
                    .1
                    .unwrap_err()
                    .message
                    .contains("work limit")
            );
        }
        "seeds" => {
            for (source, must_fail) in [
                (
                    include_str!("../../hiko-vm/fuzz/seeds/fuzz_typecheck/alias-nullary.hml"),
                    true,
                ),
                (
                    include_str!("../../hiko-vm/fuzz/seeds/fuzz_typecheck/alias-parameterized.hml"),
                    true,
                ),
                (
                    include_str!("../../hiko-vm/fuzz/seeds/fuzz_typecheck/boolean-matrix.hml"),
                    false,
                ),
            ] {
                let (used, result, _) = check(source, 200_000);
                assert!(used <= 200_000);
                if must_fail {
                    assert!(result.unwrap_err().message.contains("type alias expansion"));
                } else {
                    result.unwrap();
                }
            }
        }
        "semantics" => {
            let (_, result, warnings) = check(
                "fun f (true, _) = 1 | f (false, _) = 2 | f (_, _) = 3",
                100_000,
            );
            result.unwrap();
            assert_eq!(warnings, 1);
            let error = check("fun f (_, true) = 1", 100_000).1.unwrap_err();
            assert_eq!(error.message, "non-exhaustive match");
            check("fun f (_, true) = 1 | f (_, false) = 2", 100_000)
                .1
                .unwrap();
        }
        other => panic!("unknown case {other}"),
    }
}

#[test]
fn budget_cases_have_a_hard_deadline() {
    for case in [
        "wide",
        "nullary",
        "parameterized",
        "boundaries",
        "semantics",
        "seeds",
    ] {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "budget_child", "--nocapture"])
            .env("HIKO_TYPE_BUDGET_CASE", case)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let output = child.wait_with_output().expect("reap killed checker");
                panic!(
                    "{case} exceeded 8s; killed and reaped: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{case}: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
