#![no_main]
use libfuzzer_sys::fuzz_target;
use hiko_syntax::lexer::Lexer;
use hiko_syntax::parser::Parser;

fuzz_target!(|data: &[u8]| {
    if let Ok(input) = std::str::from_utf8(data) {
        let mut lexer = Lexer::new(input, 0);
        if let Ok(tokens) = lexer.tokenize() {
            // The parser should never panic on any valid token stream.
            let mut parser = Parser::new(tokens);
            let _ = parser.parse_program();
        }
    }
});
