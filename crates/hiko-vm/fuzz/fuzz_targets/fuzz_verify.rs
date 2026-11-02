//! Arbitrary bytes → `CompiledProgram` → verify. A program the verifier
//! accepts must run (with small limits) to success or a runtime error,
//! never a panic. Seed with `seed_programs` to start from real programs.
#![no_main]
use hiko_vm::verify::verify_program;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some(program) = hiko_vm_fuzz::decode(data) else {
        return;
    };
    if verify_program(&program).is_ok() {
        hiko_vm_fuzz::run_bounded(program);
    }
});
