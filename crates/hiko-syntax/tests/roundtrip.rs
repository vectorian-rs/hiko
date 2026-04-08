use hiko_syntax::lexer::Lexer;
use hiko_syntax::parser::Parser;
use hiko_syntax::pretty::pretty_program;

fn roundtrip(input: &str) -> String {
    let tokens = Lexer::new(input, 0).tokenize().expect("lex error");
    let ast = Parser::new(tokens).parse_program().expect("parse error");
    pretty_program(&ast)
}

fn assert_roundtrip(input: &str) {
    let first = roundtrip(input);
    let second = roundtrip(&first);
    assert_eq!(
        first, second,
        "round-trip failed:\n  input:  {input}\n  first:  {first}\n  second: {second}"
    );
}

// ── Value bindings ───────────────────────────────────────────────────

#[test]
fn rt_val_int() {
    assert_roundtrip("val x = 42");
}

#[test]
fn rt_val_string() {
    assert_roundtrip(r#"val s = "hello""#);
}

#[test]
fn rt_val_unit() {
    assert_roundtrip("val u = ()");
}

#[test]
fn rt_val_tuple() {
    assert_roundtrip("val t = (1, 2, 3)");
}

#[test]
fn rt_val_list() {
    assert_roundtrip("val xs = [1, 2, 3]");
}

#[test]
fn rt_val_empty_list() {
    assert_roundtrip("val xs = []");
}

#[test]
fn rt_val_bool() {
    assert_roundtrip("val b = true");
}

#[test]
fn rt_val_rec() {
    assert_roundtrip("val rec f = fn x => x");
}

// ── Functions ────────────────────────────────────────────────────────

#[test]
fn rt_fun_simple() {
    assert_roundtrip("fun add x y = x + y");
}

#[test]
fn rt_fun_clausal() {
    assert_roundtrip("fun f 0 = 1\n  | f n = n * f (n - 1)");
}

#[test]
fn rt_fun_mutual() {
    assert_roundtrip("fun f x = g x\nand g y = f y");
}

// ── Datatypes ────────────────────────────────────────────────────────

#[test]
fn rt_datatype_simple() {
    assert_roundtrip("datatype shape = Circle of Float | Rect of Float * Float");
}

#[test]
fn rt_datatype_param() {
    assert_roundtrip("datatype 'a option = None | Some of 'a");
}

#[test]
fn rt_datatype_multi_param() {
    assert_roundtrip("datatype ('a, 'b) either = Left of 'a | Right of 'b");
}

// ── Type aliases ─────────────────────────────────────────────────────

#[test]
fn rt_type_alias() {
    assert_roundtrip("type point = Float * Float");
}

#[test]
fn rt_type_arrow() {
    assert_roundtrip("type f = Int -> Bool");
}

#[test]
fn rt_type_app() {
    assert_roundtrip("type xs = Int list");
}

// ── Expressions ──────────────────────────────────────────────────────

#[test]
fn rt_if_expr() {
    assert_roundtrip("val x = if true then 1 else 2");
}

#[test]
fn rt_let_expr() {
    assert_roundtrip("val x = let\n  val a = 1\nin\n  a + 2\nend");
}

#[test]
fn rt_case_expr() {
    assert_roundtrip("val x = case y of\n    0 => 1\n  | n => n");
}

#[test]
fn rt_fn_expr() {
    assert_roundtrip("val f = fn x => x + 1");
}

#[test]
fn rt_cons_expr() {
    assert_roundtrip("val x = 1 :: 2 :: []");
}

#[test]
fn rt_nested_fn() {
    assert_roundtrip("val f = fn x => fn y => x + y");
}

// ── Literals ─────────────────────────────────────────────────────────

#[test]
fn rt_char_lit() {
    assert_roundtrip(r#"val c = #"a""#);
}

#[test]
fn rt_char_escape() {
    assert_roundtrip(r#"val c = #"\n""#);
}

#[test]
fn rt_float_lit() {
    assert_roundtrip("val x = 3.14");
}

#[test]
fn rt_string_escape() {
    assert_roundtrip(r#"val s = "hello\tworld\n""#);
}

// ── Operators and precedence ─────────────────────────────────────────

#[test]
fn rt_arith_precedence() {
    assert_roundtrip("val x = 1 + 2 * 3");
}

#[test]
fn rt_float_ops() {
    assert_roundtrip("val x = 1.0 +. 2.0 *. 3.0");
}

#[test]
fn rt_float_comparison() {
    assert_roundtrip("val b = x <. 1.0");
}

#[test]
fn rt_float_comparison_le() {
    assert_roundtrip("val b = x <=. 1.0");
}

#[test]
fn rt_float_comparison_gt() {
    assert_roundtrip("val b = x >. 1.0");
}

#[test]
fn rt_float_comparison_ge() {
    assert_roundtrip("val b = x >=. 1.0");
}

#[test]
fn rt_comparison() {
    assert_roundtrip("val b = x < 10");
}

#[test]
fn rt_mod_op() {
    assert_roundtrip("val r = x mod 3");
}

#[test]
fn rt_orelse_andalso() {
    assert_roundtrip("val b = a orelse b andalso c");
}

#[test]
fn rt_orelse_chain() {
    assert_roundtrip("val b = a orelse b orelse c");
}

#[test]
fn rt_andalso_chain() {
    assert_roundtrip("val b = a andalso b andalso c");
}

#[test]
fn rt_string_concat() {
    assert_roundtrip(r#"val s = "hello" ^ " " ^ "world""#);
}

#[test]
fn rt_unary_neg() {
    assert_roundtrip("val x = ~42");
}

#[test]
fn rt_unary_not() {
    assert_roundtrip("val b = not true");
}

#[test]
fn rt_unary_neg_expr() {
    assert_roundtrip("val x = ~(a + b)");
}

#[test]
fn rt_equality() {
    assert_roundtrip("val b = x = 0");
}

#[test]
fn rt_not_equal() {
    assert_roundtrip("val b = x <> 0");
}

// ── Patterns ─────────────────────────────────────────────────────────

#[test]
fn rt_pattern_constructor() {
    assert_roundtrip("fun f (Some x) = x\n  | f None = 0");
}

#[test]
fn rt_pattern_cons() {
    assert_roundtrip("fun f (x :: xs) = x\n  | f [] = 0");
}

#[test]
fn rt_pattern_wildcard() {
    assert_roundtrip("val _ = 42");
}

#[test]
fn rt_pattern_tuple() {
    assert_roundtrip("val (x, y) = (1, 2)");
}

#[test]
fn rt_pattern_negative() {
    assert_roundtrip("fun f ~1 = true\n  | f _ = false");
}

#[test]
fn rt_pattern_negative_float() {
    assert_roundtrip("fun f ~1.0 = true\n  | f _ = false");
}

#[test]
fn rt_pattern_char() {
    assert_roundtrip(r#"fun f #"a" = true | f _ = false"#);
}

#[test]
fn rt_pattern_string() {
    assert_roundtrip(r#"fun f "yes" = true | f _ = false"#);
}

#[test]
fn rt_pattern_as() {
    assert_roundtrip("val (x as Some _) = y");
}

#[test]
fn rt_pattern_annotated() {
    assert_roundtrip("val (x : Int) = 42");
}

#[test]
fn rt_pattern_list() {
    assert_roundtrip("fun f [x, y] = x + y\n  | f _ = 0");
}

#[test]
fn rt_pattern_nested_cons() {
    assert_roundtrip("fun f (x :: y :: zs) = x\n  | f _ = 0");
}

// ── Declarations ─────────────────────────────────────────────────────

#[test]
fn rt_local() {
    assert_roundtrip("local\n  val x = 1\nin\n  val y = x\nend");
}

#[test]
fn rt_local_multi() {
    assert_roundtrip("local\n  val a = 1\n  val b = 2\nin\n  val c = a + b\nend");
}

#[test]
fn rt_type_alias_param() {
    assert_roundtrip("type 'a box = 'a");
}

#[test]
fn rt_type_alias_multi_param() {
    assert_roundtrip("type ('a, 'b) pair = 'a * 'b");
}

// ── Imports ──────────────────────────────────────────────────────────

#[test]
fn rt_use() {
    assert_roundtrip(r#"use "foo.hk""#);
}

#[test]
fn rt_use_escaped_path() {
    assert_roundtrip(r#"use "dir\\file.hk""#);
}

// ── Application with unary operators ─────────────────────────────────

#[test]
fn rt_app_unary_neg() {
    assert_roundtrip("val x = f ~1");
}

#[test]
fn rt_app_not() {
    assert_roundtrip("val x = f not true");
}

#[test]
fn rt_constructor_unary_neg() {
    assert_roundtrip("val x = Some ~1");
}

// ── Type expressions ────────────────────────────────────────────────

#[test]
fn rt_type_nested_arrow() {
    assert_roundtrip("type f = Int -> Bool -> String");
}

#[test]
fn rt_type_arrow_parens() {
    assert_roundtrip("type f = (Int -> Bool) -> String");
}

#[test]
fn rt_type_nested_app() {
    assert_roundtrip("type xs = Int list list");
}

#[test]
fn rt_type_tuple_in_arrow() {
    assert_roundtrip("type f = Int * Bool -> String");
}

#[test]
fn rt_type_tyvar() {
    assert_roundtrip("type t = 'a");
}

// ── Expressions ──────────────────────────────────────────────────────

#[test]
fn rt_annotation() {
    assert_roundtrip("val x = (42 : Int)");
}

#[test]
fn rt_application_chain() {
    assert_roundtrip("val x = f a b c");
}

#[test]
fn rt_nested_let() {
    assert_roundtrip("val x = let\n  val a = let\n    val b = 1\n  in\n    b\n  end\nin\n  a\nend");
}

#[test]
fn rt_nested_case() {
    assert_roundtrip(
        "val x = case a of\n    0 => case b of\n      0 => 1\n    | _ => 2\n  | _ => 3",
    );
}

#[test]
fn rt_if_nested() {
    assert_roundtrip("val x = if a then if b then 1 else 2 else 3");
}

// ── Complex programs ─────────────────────────────────────────────────

#[test]
fn rt_option_map() {
    let src = "datatype 'a option = None | Some of 'a\nfun map_option f opt = case opt of\n    None => None\n  | Some x => Some (f x)";
    assert_roundtrip(src);
}

#[test]
fn rt_list_map() {
    assert_roundtrip("fun map f xs = case xs of\n    [] => []\n  | x :: xs => f x :: map f xs");
}

#[test]
fn rt_foldl() {
    assert_roundtrip(
        "fun foldl f acc xs = case xs of\n    [] => acc\n  | x :: xs => foldl f (f (acc, x)) xs",
    );
}

#[test]
fn rt_compose() {
    assert_roundtrip("fun compose f g = fn x => f (g x)");
}

#[test]
fn rt_multi_decl_program() {
    let src = "datatype 'a option = None | Some of 'a\nfun id x = x\nval result = id (Some 42)";
    assert_roundtrip(src);
}
