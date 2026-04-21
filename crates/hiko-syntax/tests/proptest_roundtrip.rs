use proptest::prelude::*;

use hiko_syntax::lexer::Lexer;
use hiko_syntax::parser::Parser;
use hiko_syntax::pretty::pretty_program;

// ── Keywords to avoid when generating identifiers ───────────────────

const KEYWORDS: &[&str] = &[
    "val",
    "fun",
    "fn",
    "let",
    "in",
    "if",
    "then",
    "else",
    "case",
    "of",
    "end",
    "type",
    "datatype",
    "effect",
    "handle",
    "with",
    "perform",
    "resume",
    "return",
    "structure",
    "struct",
    "signature",
    "sig",
    "open",
    "use",
    "as",
    "and",
    "rec",
    "where",
    "true",
    "false",
    "not",
    "andalso",
    "orelse",
    "raise",
    "try",
    "catch",
    "local",
    "import",
    "mod",
];

// ── Roundtrip helper ────────────────────────────────────────────────

fn roundtrip_stable(input: &str) -> bool {
    let tokens = match Lexer::new(input, 0).tokenize() {
        Ok(t) => t,
        Err(_) => return true, // skip unparseable input
    };
    let ast = match Parser::new(tokens).parse_program() {
        Ok(a) => a,
        Err(_) => return true, // skip unparseable input
    };
    let pretty1 = pretty_program(&ast);
    let tokens2 = match Lexer::new(&pretty1, 0).tokenize() {
        Ok(t) => t,
        Err(_) => return false, // pretty output should be parseable!
    };
    let ast2 = match Parser::new(tokens2).parse_program() {
        Ok(a) => a,
        Err(_) => return false,
    };
    let pretty2 = pretty_program(&ast2);
    pretty1 == pretty2
}

// ── Strategies ──────────────────────────────────────────────────────

/// Generate a valid Hiko lowercase identifier (not a keyword).
fn ident_strategy() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9_]{0,7}".prop_filter("must not be a keyword", |s| {
        !KEYWORDS.contains(&s.as_str()) && s != "_"
    })
}

/// Generate a string that is safe to embed in Hiko string literals.
/// We restrict to printable ASCII to avoid escaping issues with control
/// characters. The lexer only supports \n, \t, \r, \0, \\, \", and \xHH.
fn safe_string_strategy() -> impl Strategy<Value = String> {
    prop::collection::vec(prop::char::range(' ', '~'), 0..16)
        .prop_map(|chars| chars.into_iter().collect::<String>())
}

/// Generate a Hiko string literal including quotes and proper escaping.
fn string_literal_strategy() -> impl Strategy<Value = String> {
    safe_string_strategy().prop_map(|s| {
        let mut out = String::from('"');
        for c in s.chars() {
            match c {
                '\n' => out.push_str("\\n"),
                '\t' => out.push_str("\\t"),
                '\\' => out.push_str("\\\\"),
                '"' => out.push_str("\\\""),
                c => out.push(c),
            }
        }
        out.push('"');
        out
    })
}

/// Generate a Hiko char literal: #"x" or #"\n" etc.
fn char_literal_strategy() -> impl Strategy<Value = String> {
    prop::char::range(' ', '~').prop_map(|c| {
        let mut out = String::from("#\"");
        match c {
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            c => out.push(c),
        }
        out.push('"');
        out
    })
}

/// Generate a Hiko int literal.
fn int_literal_strategy() -> impl Strategy<Value = String> {
    // Use a reasonable range to avoid overflow issues in parsing
    (-1_000_000i64..1_000_000i64).prop_map(|n| n.to_string())
}

/// Generate a Hiko float literal (non-NaN, non-Inf, always has decimal point).
fn float_literal_strategy() -> impl Strategy<Value = String> {
    // Use simple floats that survive roundtrip formatting
    (-1000.0f64..1000.0f64)
        .prop_filter("must be finite", |f| f.is_finite())
        .prop_map(|f| {
            let s = format!("{f}");
            // Ensure it has a decimal point so the lexer sees it as float
            if !s.contains('.') && !s.contains('e') && !s.contains('E') {
                format!("{s}.0")
            } else {
                s
            }
        })
}

/// Generate a Hiko literal expression.
fn literal_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        int_literal_strategy(),
        float_literal_strategy(),
        string_literal_strategy(),
        char_literal_strategy(),
        Just("true".to_string()),
        Just("false".to_string()),
        Just("()".to_string()),
    ]
}

/// Generate a Hiko expression, depth-limited via recursion.
fn expr_strategy(depth: u32) -> BoxedStrategy<String> {
    if depth == 0 {
        // Base case: only literals and variables
        prop_oneof![literal_strategy(), ident_strategy(),].boxed()
    } else {
        let leaf = prop_oneof![literal_strategy(), ident_strategy(),];

        let inner = expr_strategy(depth - 1);

        prop_oneof![
            // Leaf expressions (weighted higher to keep things small)
            8 => leaf,
            // Parenthesized
            2 => inner.clone().prop_map(|e| format!("({e})")),
            // If-then-else
            1 => (inner.clone(), inner.clone(), inner.clone())
                .prop_map(|(c, t, e)| format!("if {c} then {t} else {e}")),
            // Let expression
            1 => (ident_strategy(), inner.clone(), inner.clone())
                .prop_map(|(id, bound, body)| format!("let\n  val {id} = {bound}\nin\n  {body}\nend")),
            // Lambda
            1 => (ident_strategy(), inner.clone())
                .prop_map(|(id, body)| format!("fn {id} => {body}")),
            // Application (always parenthesize to avoid ambiguity)
            1 => (inner.clone(), inner.clone())
                .prop_map(|(f, a)| format!("({f}) ({a})")),
            // Tuple (2-3 elements)
            1 => prop::collection::vec(inner.clone(), 2..=3)
                .prop_map(|elems| format!("({})", elems.join(", "))),
            // List (0-3 elements)
            1 => prop::collection::vec(inner.clone(), 0..=3)
                .prop_map(|elems| format!("[{}]", elems.join(", "))),
        ]
        .boxed()
    }
}

/// Generate a Hiko val declaration.
fn val_decl_strategy(depth: u32) -> BoxedStrategy<String> {
    (ident_strategy(), expr_strategy(depth))
        .prop_map(|(id, expr)| format!("val {id} = {expr}"))
        .boxed()
}

/// Generate a simple Hiko fun declaration (single clause, 1-2 params).
fn fun_decl_strategy(depth: u32) -> BoxedStrategy<String> {
    (
        ident_strategy(),
        prop::collection::vec(ident_strategy(), 1..=2),
        expr_strategy(depth),
    )
        .prop_map(|(name, params, body)| format!("fun {} {} = {}", name, params.join(" "), body))
        .boxed()
}

/// Generate a single declaration.
fn decl_strategy(depth: u32) -> BoxedStrategy<String> {
    prop_oneof![
        3 => val_decl_strategy(depth),
        2 => fun_decl_strategy(depth),
    ]
    .boxed()
}

/// Generate a program (sequence of declarations).
fn program_strategy() -> impl Strategy<Value = String> {
    prop::collection::vec(decl_strategy(2), 1..=4).prop_map(|decls| decls.join("\n"))
}

// ── Property tests ──────────────────────────────────────────────────

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn roundtrip_generated_programs(prog in program_strategy()) {
        prop_assert!(
            roundtrip_stable(&prog),
            "roundtrip failed for:\n{prog}"
        );
    }

    #[test]
    fn roundtrip_val_with_literal(
        id in ident_strategy(),
        lit in literal_strategy(),
    ) {
        let src = format!("val {id} = {lit}");
        prop_assert!(
            roundtrip_stable(&src),
            "roundtrip failed for:\n{src}"
        );
    }

    #[test]
    fn roundtrip_nested_let(
        outer in ident_strategy(),
        inner in ident_strategy(),
        lit in int_literal_strategy(),
    ) {
        let src = format!(
            "val {outer} = let\n  val {inner} = {lit}\nin\n  {inner}\nend"
        );
        prop_assert!(
            roundtrip_stable(&src),
            "roundtrip failed for:\n{src}"
        );
    }

    #[test]
    fn roundtrip_if_expression(
        id in ident_strategy(),
        cond in literal_strategy(),
        then_e in literal_strategy(),
        else_e in literal_strategy(),
    ) {
        let src = format!("val {id} = if {cond} then {then_e} else {else_e}");
        prop_assert!(
            roundtrip_stable(&src),
            "roundtrip failed for:\n{src}"
        );
    }

    #[test]
    fn roundtrip_fn_expression(
        name in ident_strategy(),
        param in ident_strategy(),
        body in literal_strategy(),
    ) {
        let src = format!("val {name} = fn {param} => {body}");
        prop_assert!(
            roundtrip_stable(&src),
            "roundtrip failed for:\n{src}"
        );
    }

    #[test]
    fn roundtrip_fun_declaration(
        name in ident_strategy(),
        param1 in ident_strategy(),
        param2 in ident_strategy(),
        body in literal_strategy(),
    ) {
        let src = format!("fun {name} {param1} {param2} = {body}");
        prop_assert!(
            roundtrip_stable(&src),
            "roundtrip failed for:\n{src}"
        );
    }

    #[test]
    fn roundtrip_list_expression(
        id in ident_strategy(),
        elems in prop::collection::vec(int_literal_strategy(), 0..=5),
    ) {
        let src = format!("val {id} = [{}]", elems.join(", "));
        prop_assert!(
            roundtrip_stable(&src),
            "roundtrip failed for:\n{src}"
        );
    }

    #[test]
    fn roundtrip_tuple_expression(
        id in ident_strategy(),
        elems in prop::collection::vec(int_literal_strategy(), 2..=4),
    ) {
        let src = format!("val {id} = ({})", elems.join(", "));
        prop_assert!(
            roundtrip_stable(&src),
            "roundtrip failed for:\n{src}"
        );
    }

    #[test]
    fn roundtrip_string_literal(
        id in ident_strategy(),
        s in string_literal_strategy(),
    ) {
        let src = format!("val {id} = {s}");
        prop_assert!(
            roundtrip_stable(&src),
            "roundtrip failed for:\n{src}"
        );
    }

    #[test]
    fn roundtrip_char_literal(
        id in ident_strategy(),
        c in char_literal_strategy(),
    ) {
        let src = format!("val {id} = {c}");
        prop_assert!(
            roundtrip_stable(&src),
            "roundtrip failed for:\n{src}"
        );
    }

    #[test]
    fn roundtrip_deep_expression(
        prog in prop::collection::vec(decl_strategy(3), 1..=2)
            .prop_map(|decls| decls.join("\n"))
    ) {
        prop_assert!(
            roundtrip_stable(&prog),
            "roundtrip failed for:\n{prog}"
        );
    }
}
