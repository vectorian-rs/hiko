#![cfg(not(feature = "builtin-json-reader"))]
mod common;
#[test]
fn reader_import_requires_its_independent_build_feature() {
    use hiko_compile::compiler::Compiler;
    use hiko_syntax::{lexer::Lexer, parser::Parser};
    let source = "import Std.JsonReader\nval r = JsonReader.of_string \"null\"";
    let ast = Parser::new(Lexer::new(source, 0).tokenize().unwrap())
        .parse_program()
        .unwrap();
    let entry = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("inline.hml");
    let error = Compiler::compile_file_with_options(ast, &entry, common::hermetic_loader_options())
        .unwrap_err();
    assert!(
        format!("{error:?}").contains("builtin-json-reader"),
        "{error:?}"
    );
}
