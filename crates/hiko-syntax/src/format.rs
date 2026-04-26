use crate::lexer::LexError;
use crate::parser::ParseError;
use tree_sitter::{Node, Parser as TsParser};
use tree_sitter_hiko::LANGUAGE;

#[derive(Debug, Clone)]
pub enum FormatError {
    Lex(LexError),
    Parse(ParseError),
    TreeSitter(String),
}

impl From<LexError> for FormatError {
    fn from(error: LexError) -> Self {
        Self::Lex(error)
    }
}

impl From<ParseError> for FormatError {
    fn from(error: ParseError) -> Self {
        Self::Parse(error)
    }
}

pub fn format_source(source: &str, _file_id: u32) -> Result<String, FormatError> {
    cst_format_source(source)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CstToken<'a> {
    kind: &'a str,
    text: &'a str,
    start: usize,
    end: usize,
}

fn cst_format_source(source: &str) -> Result<String, FormatError> {
    let language = tree_sitter::Language::from(LANGUAGE);
    let mut parser = TsParser::new();
    parser.set_language(&language).map_err(|err| {
        FormatError::TreeSitter(format!("failed to load tree-sitter-hiko: {err}"))
    })?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| FormatError::TreeSitter("tree-sitter-hiko returned no parse tree".into()))?;
    let root = tree.root_node();
    if root.has_error() {
        return Err(FormatError::TreeSitter(
            "tree-sitter-hiko reported syntax errors".into(),
        ));
    }

    let mut tokens = Vec::new();
    collect_cst_tokens(root, source, &mut tokens);
    Ok(print_cst_tokens(source, &tokens))
}

fn collect_cst_tokens<'a>(node: Node<'a>, source: &'a str, tokens: &mut Vec<CstToken<'a>>) {
    if node.child_count() == 0 {
        let start = node.start_byte();
        let end = node.end_byte();
        if start < end {
            tokens.push(CstToken {
                kind: node.kind(),
                text: &source[start..end],
                start,
                end,
            });
        }
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_cst_tokens(child, source, tokens);
    }
}

fn print_cst_tokens(source: &str, tokens: &[CstToken<'_>]) -> String {
    let mut out = String::new();
    let mut indent = 0usize;
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    let mut last_end = 0usize;
    let mut prev_text: Option<&str> = None;
    let mut at_line_start = true;

    for (idx, token) in tokens.iter().enumerate() {
        let text = token.text;
        let next = tokens.get(idx + 1).map(|token| token.text);
        let top_level_decl =
            is_decl_starter(text) && indent == 0 && paren_depth == 0 && bracket_depth == 0;

        if matches!(text, "in" | "end") {
            indent = indent.saturating_sub(2);
        }

        if text == "|" && prev_text.is_some_and(|prev| prev != "of" && prev != "with") {
            ensure_newline(&mut out, indent.saturating_sub(2));
            at_line_start = false;
        } else if top_level_decl && !out.trim().is_empty() && !out.ends_with("\n\n") {
            if !is_comment_text(prev_text) {
                ensure_blank_line(&mut out);
                at_line_start = true;
            }
        } else if is_decl_starter(text)
            && indent > 0
            && has_line_break(source, last_end, token.start)
        {
            ensure_newline(&mut out, indent);
            at_line_start = true;
        } else if gap_has_blank_line(source, last_end, token.start) && !out.trim().is_empty() {
            ensure_blank_line(&mut out);
            at_line_start = true;
        } else if should_break_before(text, prev_text) {
            ensure_newline(&mut out, indent);
            at_line_start = true;
        }

        if token.kind == "comment" {
            if has_line_break(source, last_end, token.start) && !at_line_start {
                ensure_newline(&mut out, indent);
                at_line_start = true;
            }
            if !at_line_start && !out.ends_with(' ') {
                out.push_str("  ");
            } else if at_line_start {
                write_indent(&mut out, indent);
            }
            out.push_str(text.trim());
            out.push('\n');
            at_line_start = true;
            last_end = token.end;
            prev_text = Some(text);
            continue;
        }

        if at_line_start {
            write_indent(&mut out, indent);
            at_line_start = false;
        } else if needs_space_between(prev_text, text) {
            trim_trailing_spaces(&mut out);
            out.push(' ');
        }

        if matches!(text, ")" | "]" | "," | ";") {
            trim_trailing_spaces(&mut out);
        }
        out.push_str(text);

        match text {
            "let" | "struct" | "sig" | "with" => indent += 2,
            "in" => indent += 2,
            "(" => paren_depth += 1,
            ")" => paren_depth = paren_depth.saturating_sub(1),
            "[" => bracket_depth += 1,
            "]" => bracket_depth = bracket_depth.saturating_sub(1),
            _ => {}
        }

        if should_break_after(text, next) {
            out.push('\n');
            at_line_start = true;
        }

        last_end = token.end;
        prev_text = Some(text);
    }

    trim_trailing_blank_lines(&mut out);
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn is_decl_starter(text: &str) -> bool {
    matches!(
        text,
        "val"
            | "fun"
            | "datatype"
            | "type"
            | "import"
            | "use"
            | "signature"
            | "structure"
            | "effect"
            | "local"
            | "and"
    )
}

fn is_word_like(text: &str) -> bool {
    text.chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '\'' || ch == '"')
}

fn is_comment_text(text: Option<&str>) -> bool {
    text.is_some_and(|text| text.starts_with("(*"))
}

fn has_line_break(source: &str, start: usize, end: usize) -> bool {
    source[start..end].contains('\n')
}

fn is_operator(text: &str) -> bool {
    matches!(
        text,
        "=" | "=>"
            | ":"
            | ":>"
            | "+"
            | "-"
            | "*"
            | "/"
            | "%"
            | "=="
            | "<>"
            | "<"
            | ">"
            | "<="
            | ">="
            | "::"
            | "|>"
            | "andalso"
            | "orelse"
    )
}

fn needs_space_between(prev: Option<&str>, current: &str) -> bool {
    let Some(prev) = prev else { return false };
    if matches!(current, ")" | "]" | "," | ";") || matches!(prev, "(" | "[" | "#") {
        return false;
    }
    if matches!(current, "(" | "[") {
        return is_operator(prev) || is_word_like(prev) || matches!(prev, ")" | "]");
    }
    if is_operator(prev) || is_operator(current) {
        return true;
    }
    if current == "|" || prev == "|" {
        return true;
    }
    is_word_like(prev) && is_word_like(current)
}

fn should_break_before(text: &str, prev: Option<&str>) -> bool {
    matches!(text, "in" | "end") || (text == "|" && prev != Some("of") && prev != Some("with"))
}

fn should_break_after(text: &str, next: Option<&str>) -> bool {
    matches!(text, "let" | "in" | "struct" | "sig")
        || (matches!(text, "of" | "with") && next != Some("|"))
        || matches!(text, ";")
}

fn ensure_newline(out: &mut String, indent: usize) {
    trim_trailing_spaces(out);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    write_indent(out, indent);
}

fn ensure_blank_line(out: &mut String) {
    trim_trailing_spaces(out);
    while out.ends_with("\n\n\n") {
        out.pop();
    }
    if !out.ends_with("\n\n") {
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push('\n');
    }
}

fn trim_trailing_spaces(out: &mut String) {
    while out.ends_with(' ') || out.ends_with('\t') {
        out.pop();
    }
}

fn trim_trailing_blank_lines(out: &mut String) {
    trim_trailing_spaces(out);
    while out.ends_with("\n\n") {
        out.pop();
    }
}

fn gap_has_blank_line(source: &str, start: usize, end: usize) -> bool {
    let mut after_newline = false;
    let mut current_line_has_content = false;

    for byte in source.as_bytes()[start..end].iter().copied() {
        if byte == b'\n' {
            if after_newline && !current_line_has_content {
                return true;
            }
            after_newline = true;
            current_line_has_content = false;
        } else if !matches!(byte, b' ' | b'\t' | b'\r') {
            current_line_has_content = true;
        }
    }

    false
}

fn write_indent(buf: &mut String, indent: usize) {
    for _ in 0..indent {
        buf.push(' ');
    }
}

#[cfg(test)]
mod tests {
    use super::format_source;

    fn fmt(source: &str) -> String {
        format_source(source, 0).expect("formatting should succeed")
    }

    #[test]
    fn preserves_header_and_trailing_comments() {
        let source = "(* header *)\nval _=println \"hi\" (* tail *)\n";
        assert_eq!(
            fmt(source),
            "(* header *)\nval _ = println \"hi\"  (* tail *)\n"
        );
    }

    #[test]
    fn keeps_comment_markers_inside_strings() {
        let source = "val s = \"(* not a comment *)\"\n(* real comment *)\nval _ = println s\n";
        assert_eq!(
            fmt(source),
            "val s = \"(* not a comment *)\"\n(* real comment *)\nval _ = println s\n"
        );
    }

    #[test]
    fn is_idempotent_with_nested_comments() {
        let source = "fun f x =\n  case x of\n      [] => 0\n    | y :: ys => (* branch *) y\n";
        let once = fmt(source);
        let twice = fmt(&once);
        assert_eq!(twice, once);
    }

    #[test]
    fn formats_comment_only_files() {
        let source = "(* hello *)\n(* world *)\n";
        assert_eq!(fmt(source), "(* hello *)\n(* world *)\n");
    }

    #[test]
    fn preserves_single_blank_lines_between_comment_groups() {
        let source = "(* header *)\n\n(* section *)\nval x=1\n\n(* next *)\nval y=2\n";
        assert_eq!(
            fmt(source),
            "(* header *)\n\n(* section *)\nval x = 1\n\n(* next *)\nval y = 2\n"
        );
    }

    #[test]
    fn preserves_single_blank_lines_between_top_level_decls() {
        let source = "import Std.List\nval answer=41\nfun inc x=x+1\nfun dec x=x-1\n";
        assert_eq!(
            fmt(source),
            "import Std.List\n\nval answer = 41\n\nfun inc x = x + 1\n\nfun dec x = x - 1\n"
        );
    }

    #[test]
    fn nested_let_decls_do_not_gain_forced_blank_lines() {
        let source = "val x = let\n  val a=1\n  val b=2\nin\n  a + b\nend\n";
        assert_eq!(
            fmt(source),
            "val x = let\n    val a = 1\n    val b = 2\nin\n  a + b\nend\n"
        );
    }

    #[test]
    fn mutual_fun_after_let_body_keeps_and_on_new_line() {
        let source = "fun walk dir = let\n  val entries = list_dir dir\nin\n  walk_entries dir entries\nend\nand walk_entries dir entries = case entries of\n  [] => ()\n| name :: rest => walk_entries dir rest\n";
        assert_eq!(
            fmt(source),
            "fun walk dir = let\n    val entries = list_dir dir\nin\n  walk_entries dir entries\nend\n\nand walk_entries dir entries = case entries of\n[] => ()\n | name :: rest => walk_entries dir rest\n"
        );
    }
}
