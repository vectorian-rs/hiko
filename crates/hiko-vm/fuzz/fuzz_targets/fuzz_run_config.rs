//! TOML → run config. Parsing must not panic, and an accepted config must
//! generate valid Rust (`build-vm` compiles it) that keeps the config's
//! strings: the entry path and every host-resource kind appear unchanged
//! as string literals.
#![no_main]
use std::collections::BTreeSet;

use hiko_vm::config::RunConfig;
use libfuzzer_sys::fuzz_target;
use syn::visit::Visit;

#[derive(Default)]
struct Strings(BTreeSet<String>);

impl Visit<'_> for Strings {
    fn visit_lit_str(&mut self, lit: &syn::LitStr) {
        self.0.insert(lit.value());
    }
}

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(config) = RunConfig::from_toml(text) else {
        return;
    };
    // A config with [extensions] needs to_rust_source_for; the error is fine.
    let Ok(source) = config.to_rust_source() else {
        assert!(
            !config.extensions.is_empty(),
            "only extensions make generation fail"
        );
        return;
    };
    let file = match syn::parse_file(&source) {
        Ok(file) => file,
        Err(e) => panic!("generated source does not parse: {e}\n{source}"),
    };
    let mut strings = Strings::default();
    strings.visit_file(&file);
    if let Some(entry) = &config.entry {
        assert!(
            strings.0.contains(entry),
            "entry {entry:?} lost in\n{source}"
        );
    }
    for kind in config.limits.host_resources.keys() {
        assert!(
            strings.0.contains(kind),
            "resource {kind:?} lost in\n{source}"
        );
    }
});
