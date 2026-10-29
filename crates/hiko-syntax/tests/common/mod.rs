//! Helpers shared by the integration tests.

use hiko_syntax::lexer::Lexer;
use hiko_syntax::parser::Parser;

/// The program's AST as text without spans, so two parses of differently
/// laid out source compare equal. Symbols are numbered in order of first
/// appearance, so equal names compare equal as long as their order is kept.
pub fn normalized_ast(source: &str) -> Result<String, String> {
    let tokens = Lexer::new(source, 0).tokenize().map_err(|e| e.message)?;
    let program = Parser::new(tokens).parse_program().map_err(|e| e.message)?;
    let debug = format!("{:#?}", program.decls);
    let mut out = String::with_capacity(debug.len());
    for line in debug.lines() {
        let field = line.trim_start();
        if ["file_id:", "start:", "end:"]
            .iter()
            .any(|name| field.starts_with(name))
        {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    Ok(out)
}
