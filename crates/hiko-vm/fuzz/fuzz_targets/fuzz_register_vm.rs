//! Random integer programs on the stack VM alone and with register code:
//! output, errors, error locations, fuel stops and scheduler slices must
//! be the same (`hiko_vm::vm::register_check`).
#![no_main]
use hiko_vm::vm::register_check::check_bytes;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Err(e) = check_bytes(data) {
        panic!("{e}");
    }
});
