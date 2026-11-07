//! Lexical identities must survive storage reuse on both VM feature configurations.
#![cfg(feature = "builtin-default")]

mod common;

use hiko_vm::process::ProcessStatus;
use hiko_vm::runtime::Runtime;
use hiko_vm::threaded::ThreadedRuntime;
use hiko_vm::value::Value;
use hiko_vm::vm::VM;

#[test]
fn lexical_global_bindings_conformance() {
    let program = common::compile_file_hermetic("../../tests/run/test_global_bindings.hml");
    VM::new(program).run().expect("lexical binding assertions");
}

#[test]
fn lexical_global_bindings_child_snapshots() {
    let program = common::compile_file_hermetic("../../tests/run/test_global_bindings_spawn.hml");
    let mut runtime = Runtime::new();
    let root = runtime.spawn_root(program.clone());
    runtime.run_to_completion().unwrap();
    assert!(matches!(
        runtime.get_status(root),
        Some(ProcessStatus::Done)
    ));
    assert_eq!(runtime.get_output(root), ["binding snapshots passed\n"]);
    for workers in [1, 4] {
        let runtime = ThreadedRuntime::new(workers);
        runtime.spawn_root(program.clone());
        let output = runtime.run_to_completion().unwrap();
        assert_eq!(output, ["binding snapshots passed\n"]);
    }
}

#[test]
fn lexical_global_bindings_release_unread_lists() {
    let mut source =
        String::from("fun build n acc = if n = 0 then acc else build (n - 1) (n :: acc)\n");
    for _ in 0..20 {
        source.push_str("val xs = build 2000 []\n");
    }
    source.push_str("val result = case xs of x :: _ => x | _ => 0");
    let mut vm = VM::new(common::compile_source(&source));
    vm.set_max_heap(10_000);
    vm.run()
        .expect("unread historical lists must be collectible");
    assert!(matches!(vm.get_global("result"), Some(Value::Int(1))));
}

#[test]
fn lexical_global_bindings_latest_value_after_suffix_gaps() {
    let program = common::compile_source(
        "val x = 1 val x = 2 fun f () = x val x = 3 val x = 4
         val result = f ()",
    );
    let mut vm = VM::new(program);
    vm.run().unwrap();
    assert!(matches!(vm.get_global("result"), Some(Value::Int(2))));
    assert!(matches!(vm.get_global("x"), Some(Value::Int(4))));
}
