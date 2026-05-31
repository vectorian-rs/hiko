use crate::ast::{Decl, DeclKind, Program};
use crate::lexer::{LexError, Lexer};
use crate::parser::{ParseError, Parser};
use crate::pretty::{Comment, pretty_program_with_comments};
use crate::span::Span;

#[derive(Debug, Clone)]
pub enum FormatError {
    Lex(LexError),
    Parse(ParseError),
    UnsupportedComment { message: String, span: Span },
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

pub fn format_source(source: &str, file_id: u32) -> Result<String, FormatError> {
    let line_offsets = line_offsets(source);
    let comments = collect_comments(source, &line_offsets);
    let tokens = Lexer::new(source, file_id).tokenize()?;
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program()?;
    validate_comment_convention(&program, &comments, &line_offsets, file_id)?;
    let mut formatted = pretty_program_with_comments(&program, &comments, &line_offsets);
    if !formatted.is_empty() && !formatted.ends_with('\n') {
        formatted.push('\n');
    }
    Ok(formatted)
}

fn line_offsets(source: &str) -> Vec<usize> {
    let mut offsets = vec![0];
    for (idx, byte) in source.bytes().enumerate() {
        if byte == b'\n' {
            offsets.push(idx + 1);
        }
    }
    offsets
}

fn line_for_offsets(line_offsets: &[usize], byte: usize) -> usize {
    match line_offsets.binary_search(&byte) {
        Ok(line) => line,
        Err(0) => 0,
        Err(line) => line - 1,
    }
}

fn collect_comments(source: &str, line_offsets: &[usize]) -> Vec<Comment> {
    let bytes = source.as_bytes();
    let mut comments = Vec::new();
    let mut pos = 0;

    while pos < bytes.len() {
        match bytes[pos] {
            b'"' => skip_string(bytes, &mut pos),
            b'#' if bytes.get(pos + 1) == Some(&b'"') => skip_char(bytes, &mut pos),
            b'(' if bytes.get(pos + 1) == Some(&b'*') => {
                let start = pos;
                pos += 2;
                let mut depth = 1usize;
                while pos < bytes.len() && depth > 0 {
                    if bytes[pos] == b'(' && bytes.get(pos + 1) == Some(&b'*') {
                        depth += 1;
                        pos += 2;
                    } else if bytes[pos] == b'*' && bytes.get(pos + 1) == Some(&b')') {
                        depth -= 1;
                        pos += 2;
                    } else {
                        pos += 1;
                    }
                }
                let end = pos.min(bytes.len());
                comments.push(Comment {
                    text: source[start..end].to_string(),
                    start: start as u32,
                    end: end as u32,
                    start_line: line_for_offsets(line_offsets, start),
                    end_line: line_for_offsets(line_offsets, end),
                });
            }
            _ => pos += 1,
        }
    }

    comments
}

fn skip_string(bytes: &[u8], pos: &mut usize) {
    *pos += 1;
    while *pos < bytes.len() {
        match bytes[*pos] {
            b'\\' => *pos = (*pos + 2).min(bytes.len()),
            b'"' => {
                *pos += 1;
                break;
            }
            _ => *pos += 1,
        }
    }
}

fn skip_char(bytes: &[u8], pos: &mut usize) {
    *pos += 2;
    while *pos < bytes.len() {
        match bytes[*pos] {
            b'\\' => *pos = (*pos + 2).min(bytes.len()),
            b'"' => {
                *pos += 1;
                break;
            }
            _ => *pos += 1,
        }
    }
}

fn validate_comment_convention(
    program: &Program,
    comments: &[Comment],
    line_offsets: &[usize],
    file_id: u32,
) -> Result<(), FormatError> {
    if comments.is_empty() || program.decls.is_empty() {
        return Ok(());
    }

    let mut allowed = vec![false; comments.len()];
    mark_allowed_decl_comments(
        &program.decls,
        0,
        u32::MAX,
        comments,
        line_offsets,
        &mut allowed,
    );

    if let Some((idx, comment)) = comments.iter().enumerate().find(|(idx, _)| !allowed[*idx]) {
        let _ = idx;
        return Err(FormatError::UnsupportedComment {
            message: "formatter only supports declaration-leading comments and short trailing declaration comments; move this comment before the declaration it describes".into(),
            span: Span::new(file_id, comment.start, comment.end),
        });
    }

    Ok(())
}

fn mark_allowed_decl_comments(
    decls: &[Decl],
    container_start: u32,
    container_end: u32,
    comments: &[Comment],
    line_offsets: &[usize],
    allowed: &mut [bool],
) {
    if decls.is_empty() {
        mark_comments_in_range(container_start, container_end, comments, allowed);
        return;
    }

    let mut cursor = container_start;
    for (idx, decl) in decls.iter().enumerate() {
        let next_start = decls
            .get(idx + 1)
            .map(|decl| decl.span.start)
            .unwrap_or(container_end);

        mark_comments_in_range(cursor, decl.span.start, comments, allowed);
        mark_trailing_decl_comments(decl.span.end, next_start, comments, line_offsets, allowed);
        mark_nested_decl_comments(decl, comments, line_offsets, allowed);

        cursor = decl.span.end;
    }

    mark_comments_in_range(cursor, container_end, comments, allowed);
}

fn mark_nested_decl_comments(
    decl: &Decl,
    comments: &[Comment],
    line_offsets: &[usize],
    allowed: &mut [bool],
) {
    match &decl.kind {
        DeclKind::Structure { decls, .. } => mark_allowed_decl_comments(
            decls,
            decl.span.start,
            decl.span.end,
            comments,
            line_offsets,
            allowed,
        ),
        DeclKind::Local(locals, body) => {
            mark_allowed_decl_comments(
                locals,
                decl.span.start,
                decl.span.end,
                comments,
                line_offsets,
                allowed,
            );
            mark_allowed_decl_comments(
                body,
                decl.span.start,
                decl.span.end,
                comments,
                line_offsets,
                allowed,
            );
        }
        _ => {}
    }
}

fn mark_comments_in_range(start: u32, end: u32, comments: &[Comment], allowed: &mut [bool]) {
    for (idx, comment) in comments.iter().enumerate() {
        if comment.start >= start && comment.end <= end {
            allowed[idx] = true;
        }
    }
}

fn mark_trailing_decl_comments(
    decl_end: u32,
    next_start: u32,
    comments: &[Comment],
    line_offsets: &[usize],
    allowed: &mut [bool],
) {
    let decl_end_line = line_for_offsets(line_offsets, decl_end as usize);
    for (idx, comment) in comments.iter().enumerate() {
        if comment.start >= decl_end
            && comment.start < next_start
            && comment.start_line == decl_end_line
        {
            allowed[idx] = true;
        }
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
    fn rejects_expression_level_comments() {
        let source = "fun f x =\n  case x of\n      [] => 0\n    | y :: ys => (* branch *) y\n";
        let err = format_source(source, 0).expect_err("expression comment should be rejected");
        assert!(matches!(err, super::FormatError::UnsupportedComment { .. }));
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
    fn keeps_consecutive_imports_together() {
        let source = "import Aws.Config\nimport Aws.S3\nimport Std.Option\nfun main x=x\n";
        assert_eq!(
            fmt(source),
            "import Aws.Config\nimport Aws.S3\nimport Std.Option\n\nfun main x = x\n"
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
            "val x =\n  let\n    val a = 1\n    val b = 2\n  in\n    a + b\n  end\n"
        );
    }

    #[test]
    fn formats_datatype_branches_with_comments_via_ast() {
        let source = "structure Json = struct\n(* Constructor tags are fixed. *)\ndatatype json = JNull | JBool of bool | JArray of json list | JObject of (string * json) list\nend\n";
        assert_eq!(
            fmt(source),
            "structure Json = struct\n  (* Constructor tags are fixed. *)\n  datatype json =\n      JNull\n    | JBool of bool\n    | JArray of json list\n    | JObject of (string * json) list\nend\n"
        );
    }

    #[test]
    fn formats_pipeline_chains_on_separate_lines() {
        let source = "import Aws.Config\nimport Aws.S3\nimport Std.Result\n(* Using pipe operators *)\nval _=Config.sso_profile \"datadeft-dev\" |> S3.client |> S3.list_buckets |> Result.either print_buckets (fn err => println err) |> Result.ignore\n";
        assert_eq!(
            fmt(source),
            "import Aws.Config\nimport Aws.S3\nimport Std.Result\n\n(* Using pipe operators *)\nval _ =\n  Config.sso_profile \"datadeft-dev\"\n    |> S3.client\n    |> S3.list_buckets\n    |> Result.either print_buckets (fn err => println err)\n    |> Result.ignore\n"
        );
    }

    #[test]
    fn formats_signature_types_with_spaces() {
        let source = "signature EXEC = sig\nval run:string->string list->int*string*string\nend\n";
        assert_eq!(
            fmt(source),
            "signature EXEC = sig\n  val run : string -> string list -> int * string * string\nend\n"
        );
    }

    #[test]
    fn mutual_fun_after_let_body_keeps_and_on_new_line() {
        let source = "fun walk dir = let\n  val entries = list_dir dir\nin\n  walk_entries dir entries\nend\nand walk_entries dir entries = case entries of\n  [] => ()\n| name :: rest => walk_entries dir rest\n";
        assert_eq!(
            fmt(source),
            "fun walk dir =\n  let\n    val entries = list_dir dir\n  in\n    walk_entries dir entries\n  end\nand walk_entries dir entries =\n  case entries of\n      [] => ()\n    | name :: rest => walk_entries dir rest\n"
        );
    }
}
