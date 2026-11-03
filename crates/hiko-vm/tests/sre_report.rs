//! `examples/sre/report_demo.hml`: `Sre.Report` runs with exactly the
//! capabilities its module comment lists, and prints the summary line and
//! the table of the checks that need attention.
#![cfg(feature = "builtin-default")]

mod common;

use std::sync::Arc;

use common::Captured;
use hiko_vm::builder::VMBuilder;
use hiko_vm::config::RunConfig;

const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");

#[test]
fn sre_report_needs_only_the_capabilities_it_lists() {
    let policy: String = [
        "stdio.println",
        "convert.int_to_string",
        "string.string_length",
        "string.string_join",
        "string.string_replace",
    ]
    .iter()
    .map(|leaf| format!("[capabilities.{leaf}]\nenabled = true\n"))
    .collect();
    let config = RunConfig::from_toml(&policy).unwrap();
    let program = common::compile_file_hermetic(&format!("{ROOT}/examples/sre/report_demo.hml"));
    let mut vm = config.apply_to_builder(VMBuilder::new(program)).build();
    let out = Arc::new(Captured::default());
    vm.set_output_sink(out.clone());
    vm.run().unwrap_or_else(|e| panic!("{}", e.message));
    assert_eq!(
        out.0.lock().unwrap().as_str(),
        "status=CRIT source=demo total=2 crit=1 unknown=0 warn=0 ok=1\n\
         | STATUS | NAME    | SINCE                | DETAIL      |\n\
         | ------ | ------- | -------------------- | ----------- |\n\
         | CRIT   | api-5xx | 2026-11-03T10:00:00Z | 5XX above 5 |\n\
         2\n"
    );
}
