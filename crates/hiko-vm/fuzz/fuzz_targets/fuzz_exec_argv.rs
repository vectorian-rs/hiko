//! `exec` argv rules. For any rule and arguments: a match means the
//! arguments start with the rule's fixed arguments, and with
//! `rest_any = false` are exactly them; the rule's own arguments always
//! match; and changing any fixed argument, or adding one to an exact rule,
//! stops the match.
#![no_main]
use arbitrary::Arbitrary;
use hiko_vm::builder::ExecArgvRule;
use libfuzzer_sys::fuzz_target;

#[derive(Debug, Arbitrary)]
struct Input {
    argv: Vec<String>,
    rest_any: bool,
    args: Vec<String>,
    change: usize,
    suffix: String,
}

fuzz_target!(|input: Input| {
    let rule = ExecArgvRule {
        argv: input.argv,
        rest_any: input.rest_any,
    };
    let fixed: Vec<String> = rule.argv.iter().skip(1).cloned().collect();

    if rule.args_match(&input.args) {
        assert!(input.args.starts_with(&fixed), "{rule:?} {:?}", input.args);
        if !rule.rest_any {
            assert_eq!(input.args, fixed);
        }
    }

    assert!(rule.args_match(&fixed), "{rule:?}");

    if !fixed.is_empty() {
        let mut changed = fixed.clone();
        let i = input.change % changed.len();
        changed[i].push_str(&input.suffix);
        if !input.suffix.is_empty() {
            assert!(!rule.args_match(&changed), "{rule:?} {changed:?}");
        }
    }

    if !rule.rest_any {
        let mut longer = fixed;
        longer.push(input.suffix);
        assert!(!rule.args_match(&longer), "{rule:?} {longer:?}");
    }
});
