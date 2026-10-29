//! `hiko fmt` over every `.hml` file in the repository: the output must parse
//! to the same AST as the input (ignoring spans), keep every comment, and be a
//! fixed point of the formatter.

mod common;

use common::normalized_ast;
use hiko_syntax::format::format_source;
use std::path::{Path, PathBuf};

const CORPUS_DIRS: [&str; 4] = ["libraries", "tests", "examples", "tools"];

fn collect_hml(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let path = entry.expect("read dir entry").path();
        if path.is_dir() {
            collect_hml(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "hml") {
            out.push(path);
        }
    }
}

fn comments(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = source;
    while let Some(start) = rest.find("(*") {
        let end = rest[start..]
            .find("*)")
            .map_or(rest.len(), |e| start + e + 2);
        out.push(rest[start..end].trim().to_string());
        rest = &rest[end..];
    }
    out
}

#[test]
fn normalized_ast_ignores_layout_only() {
    let ast = |src| normalized_ast(src).unwrap();
    assert_eq!(ast("val x =\n  f 1"), ast("val x = f 1"));
    assert_ne!(ast("val x = f 1"), ast("val x = f 2"));
    assert_ne!(ast("val x = f 1"), ast("val x = (f 1)"));
}

#[test]
fn fmt_preserves_ast_and_is_idempotent_on_repo_sources() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    for dir in CORPUS_DIRS {
        collect_hml(&root.join(dir), &mut files);
    }
    files.sort();
    assert!(
        files.len() > 50,
        "corpus not found under {}",
        root.display()
    );

    let mut failures = Vec::new();
    let mut formatted_count = 0;
    for path in &files {
        let source = std::fs::read_to_string(path).expect("read source");
        let name = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .display()
            .to_string();
        let Ok(before) = normalized_ast(&source) else {
            // Sources that do not parse are not formatted.
            continue;
        };
        let once = match format_source(&source, 0) {
            Ok(once) => once,
            Err(e) => {
                failures.push(format!("{name}: fmt failed: {e:?}"));
                continue;
            }
        };
        formatted_count += 1;
        match normalized_ast(&once) {
            Ok(after) if after == before => {}
            Ok(_) => failures.push(format!(
                "{name}: formatted output parses to a different AST"
            )),
            Err(e) => failures.push(format!("{name}: formatted output does not parse: {e}")),
        }
        if comments(&once) != comments(&source) {
            failures.push(format!("{name}: comments changed"));
        }
        match format_source(&once, 0) {
            Ok(twice) if twice == once => {}
            Ok(_) => failures.push(format!("{name}: fmt is not idempotent")),
            Err(e) => failures.push(format!("{name}: second fmt failed: {e:?}")),
        }
    }
    eprintln!("formatted {formatted_count} of {} files", files.len());
    assert!(
        failures.is_empty(),
        "{} of {formatted_count} files failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn fmt_preserves_ast_on_tricky_sources() {
    let sources = [
        "val x = ~ ~1",
        "val x = not not true",
        "val x = ~ f x",
        "val x = not f x",
        "val x = f ~1",
        "val x = f not true",
        "val x = f ~fn y => y",
        "val x = f not if a then b else c",
        "val x = f ~(perform E 1)",
        "val x = a :: b :: c",
        "val x = (a :: b) :: c",
        "val x = a orelse b andalso c",
        "val x = (a orelse b) andalso c",
        "val x = (a < b) = c",
        "val x = 1 - (2 - 3)",
        "val x = 1 - 2 - 3",
        "val x = a |> f |> g",
        "val x = a |> (f |> g)",
        "val x = (fn x => x) 1",
        "val x = f (fn x => x)",
        "val x = if a then b else c + 1",
        "val x = (if a then b else c) + 1",
        "val x = (case a of _ => 1) + 1",
        "val x = a : int",
        "val x = (a : int) :: []",
        "val x = f (a : int)",
        "val x = let in 1 end + 1",
        "val x = case a of 1 => fn x => x | _ => fn y => y",
        "val x = case a of 1 => case b of _ => 1 | _ => 2",
        "val x = case a of 1 => (case b of _ => 1) | _ => 2",
        "val x = f x y",
        "val x = (f x) y",
        "val x = perform E (1 + 2)",
        "val x = resume k (f x)",
        "val x = (handle 1 with return y => y) + 1",
        "val x = handle f 1 with return y => y | E v k => resume k v",
        "fun f (x :: xs) = 1",
        "fun f (Some x :: xs) = 1",
        "val x = case y of Some x :: M.C z :: xs => 1",
        "val x = case y of x as y :: z => 1",
        "val x = case y of (x : int, y) => 1",
        "val x = case y of A (B c) => 1",
        "fun f (Some (x, y)) = 1",
        "val (a, b) : int * int = (1, 2)",
        "fun f (x as Some y) = 1",
        "fun f ~1 = 1",
        "fun f ~1.5 = 1",
        "fun f ~0.0 = 1",
        "val x = case x of A B => 1",
        "val x = case x of x :: y :: z => 1",
        "val x = case x of (x :: y) :: z => 1",
        "val x = case x of x as y as z => 1",
        "val x = case x of x : int => 1",
        "val f : int -> int -> int = g",
        "val f : (int -> int) -> int = g",
        "val f : int * int -> int = g",
        "val f : (int * int) list = g",
        "val f : int list list = g",
        "val f : (int, string) either = g",
        "val f : int * (int * int) = g",
        "val f : (int * int) * int = g",
        "val f : (int -> int) * int = g",
        "val x = 1.0e10",
        "val x = 6.02e23",
        "val x = 1e-7",
        "val x = 0.1",
        "val x = 1e300",
        "val x = \"\\t\\n\\\\\\\"\\x01\\x7f\"",
        "val x = \"\u{e9}\u{1F600}\"",
        "val x = #\"\\n\"",
        "val x = #\"\\\"\"",
        "val x = #\"\\\\\"",
        "val x = 0w0",
        "val x = 0wxFF",
        "val x = 9223372036854775807",
        "val x = M.f M.x",
        "val x = M.C 1",
        "fun f (M.C x) = x",
        "val x = f (g, h)",
        "val x = f ()",
        "val x = [fn x => x, fn y => y]",
        "val x = (1, fn x => x)",
        "val x = f (fn x => x) 1",
        "val x = fn x => fn y => x",
        "val x = fn (a, b) => a",
        "val x = fn x : int => x",
        "val x = (fn x => x) : int -> int",
        "val x = if a then if b then c else d else e",
        "val x = if a then fn x => x else fn y => y",
        "val x = 1 + (if a then 1 else 2)",
        "val x = 1 + let val y = 1 in y end",
        "val x = f (let val y = 1 in y end)",
        "val x = not (a = b)",
        "val x = ~(a + b)",
        "val x = ~a + b",
        "val x = (a = b) = c",
        "val x = a ^ b ^ c",
        "val x = a ^ (b ^ c)",
        "val x = a * b + c * d",
        "val x = (a + b) * (c + d)",
        "val x = a mod b mod c",
        "val x = a :: (b orelse c)",
        "val x = (a orelse b) :: c",
        "val x = a |> fn x => x",
        "val x = (a |> f) + 1",
        "val x = a :: b |> f",
        "datatype 'a t = A | B of 'a * 'a t | C of ('a -> 'a) list",
        "type ('a, 'b) t = 'a * 'b -> ('a, 'b) either",
        "effect E of int -> int",
        "val x = perform E ()",
        "val x = perform E (f x)",
        "val x = resume k ()",
        "local val x = 1 in val y = x end",
        "signature S = sig type 'a t val f : 'a t -> int end",
        "structure M :> S = struct type 'a t = 'a list fun f _ = 1 end",
        "structure M : S = struct end",
        "import Std.List",
        "import Std.List (map, Option)",
        "val x = let val a = 1 val b = 2 in a + b end",
        "fun f 0 = 1 | f n = n * f (n - 1) and g x = x",
    ];
    let mut failures = Vec::new();
    for source in sources {
        let Ok(before) = normalized_ast(source) else {
            failures.push(format!("{source:?}: input does not parse"));
            continue;
        };
        let once = match format_source(source, 0) {
            Ok(once) => once,
            Err(e) => {
                failures.push(format!("{source:?}: fmt failed: {e:?}"));
                continue;
            }
        };
        match normalized_ast(&once) {
            Ok(after) if after == before => {}
            Ok(_) => failures.push(format!(
                "{source:?}: reformatted as {once:?}, a different AST"
            )),
            Err(e) => failures.push(format!(
                "{source:?}: reformatted as {once:?}, which does not parse: {e}"
            )),
        }
        if format_source(&once, 0).ok().as_deref() != Some(once.as_str()) {
            failures.push(format!("{source:?}: fmt is not idempotent"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
