#![no_main]
use libfuzzer_sys::fuzz_target;
use hiko_syntax::lexer::Lexer;

fuzz_target!(|data: &[u8]| {
    if let Ok(input) = std::str::from_utf8(data) {
        // The lexer should never panic on any valid UTF-8 input.
        // Errors are fine - we just want to ensure no panics/crashes.
        let mut lexer = Lexer::new(input, 0);
        let _ = lexer.tokenize();
    }
});
