#![no_main]
use libfuzzer_sys::fuzz_target;
use hiko_syntax::lexer::Lexer;
use hiko_syntax::parser::Parser;
use hiko_syntax::pretty::pretty_program;

fuzz_target!(|data: &[u8]| {
    if let Ok(input) = std::str::from_utf8(data) {
        let mut lexer = Lexer::new(input, 0);
        let tokens = match lexer.tokenize() {
            Ok(t) => t,
            Err(_) => return,
        };
        let mut parser = Parser::new(tokens);
        let ast = match parser.parse_program() {
            Ok(a) => a,
            Err(_) => return,
        };
        let pretty1 = pretty_program(&ast);

        // If we can parse and pretty-print, the output must also parse
        let mut lexer2 = Lexer::new(&pretty1, 0);
        let tokens2 = lexer2
            .tokenize()
            .expect("pretty-printed output must lex");
        let mut parser2 = Parser::new(tokens2);
        let ast2 = parser2
            .parse_program()
            .expect("pretty-printed output must parse");
        let pretty2 = pretty_program(&ast2);

        // Roundtrip must be stable
        assert_eq!(pretty1, pretty2, "roundtrip not stable");
    }
});
