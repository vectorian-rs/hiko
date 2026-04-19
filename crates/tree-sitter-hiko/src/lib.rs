//! Tree-sitter grammar for the Hiko language.

use tree_sitter_language::LanguageFn;

unsafe extern "C" {
    fn tree_sitter_hiko() -> *const ();
}

/// The Tree-sitter language for Hiko.
pub const LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_hiko) };

/// Tree-sitter language name for Hiko.
pub const LANGUAGE_NAME: &str = "hiko";

/// Syntax highlighting query for Hiko source.
pub const HIGHLIGHTS_QUERY: &str = include_str!("../queries/highlights.scm");

/// Returns the Tree-sitter language name.
pub fn language_name() -> &'static str {
    LANGUAGE_NAME
}

#[cfg(test)]
mod tests {
    use super::LANGUAGE;
    use tree_sitter::Parser;

    #[test]
    fn loads_grammar_and_parses_hiko() {
        let mut parser = Parser::new();
        parser
            .set_language(&LANGUAGE.into())
            .expect("tree-sitter-hiko language should load");

        let tree = parser
            .parse(
                "structure Option = struct datatype 'a option = None | Some of 'a end",
                None,
            )
            .expect("source should parse");

        assert!(
            !tree.root_node().has_error(),
            "parser produced syntax errors"
        );
    }
}
