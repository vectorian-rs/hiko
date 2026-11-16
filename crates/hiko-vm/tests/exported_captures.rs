#![cfg(feature = "builtin-default")]

mod common;

use hiko_vm::process::ProcessStatus;
use hiko_vm::runtime::Runtime;
use hiko_vm::threaded::ThreadedRuntime;
use hiko_vm::value::Value;
use hiko_vm::vm::{RunResult, VM};

#[test]
fn exported_private_captures_conformance() {
    VM::new(common::compile_file_hermetic(
        "../../tests/run/test_exported_private_captures.hml",
    ))
    .run()
    .expect("private capture assertions");
}

#[test]
fn exported_private_captures_survive_gc() {
    let mut source = String::from(
        "fun build n acc = if n = 0 then acc else build (n - 1) (n :: acc)
         local val xs = build 64 [] in
             fun saved () = case xs of x :: _ => x | _ => 0
         end\n",
    );
    for _ in 0..100 {
        source.push_str("val junk = build 30 []\n");
    }
    source.push_str("val result = saved ()");
    let mut vm = VM::new(common::compile_source(&source));
    vm.set_max_heap(200);
    vm.run()
        .expect("private list remains rooted across collections");
    assert!(matches!(vm.get_global("result"), Some(Value::Int(1))));
}

#[test]
fn exported_private_captures_spawn() {
    let program = common::compile_source(
        "import __Builtin.Fiber
         import __Builtin.Stdio
         import __Builtin.Testing
         local val xs = [9, 8] in
             fun first (n : int) = if n = 0 then second 0 else second (n - 1)
             and second n = if n = 0 then (case xs of x :: _ => x | _ => 0) else first (n - 1)
             fun child () = first 4
             fun captured_child () = case xs of x :: _ => x | _ => 0
         end
         val _ = BuiltinTesting.assert_eq (BuiltinFiber.await (BuiltinFiber.spawn child), 9, \"child private captures\")
         val _ = BuiltinTesting.assert_eq (BuiltinFiber.await (BuiltinFiber.spawn captured_child), 9, \"direct closure transfer\")
         val _ = BuiltinStdio.println \"private captures passed\"",
    );
    let mut runtime = Runtime::new();
    let root = runtime.spawn_root(program.clone());
    runtime.run_to_completion().unwrap();
    assert!(matches!(
        runtime.get_status(root),
        Some(ProcessStatus::Done)
    ));
    assert_eq!(runtime.get_output(root), ["private captures passed\n"]);
    for workers in [1, 4] {
        let runtime = ThreadedRuntime::new(workers);
        runtime.spawn_root(program.clone());
        assert_eq!(
            runtime.run_to_completion().unwrap(),
            ["private captures passed\n"]
        );
    }
}

#[test]
fn exported_private_captures_reject_unsendable_values() {
    let program = common::compile_source(
        "import __Builtin.Bytes
         import __Builtin.Fiber
         import __Builtin.Random
         local val r = BuiltinRandom.seed (BuiltinBytes.of_string \"seed\") in
             fun child () = let val _ = r in 1 end
         end
         val _ = BuiltinFiber.spawn child",
    );
    let mut runtime = Runtime::new();
    let root = runtime.spawn_root(program);
    runtime.run_to_completion().unwrap();
    let Some(ProcessStatus::Failed(error)) = runtime.get_status(root) else {
        panic!("spawn of an unsendable closure must fail explicitly");
    };
    assert!(
        error
            .to_string()
            .contains("cannot send Rng state across processes"),
        "{error}"
    );
}

#[test]
fn capture_free_work_baseline() {
    let programs = [
        "fun add (a : int, b : int) = a + b
         fun twice n = add (n, n)
         val result = twice 21",
        "local val ignored = 9 in
             fun even (n : int) = if n = 0 then true else odd (n - 1)
             and odd n = if n = 0 then false else even (n - 1)
         end
         val result = even 6",
    ];
    for (index, source) in programs.iter().enumerate() {
        let program = common::compile_source(source);
        let mut vm = VM::new(program);
        vm.set_fuel(100_000);
        assert!(matches!(vm.run_slice(100_000), RunResult::Done));
        let result = vm.get_global("result");
        assert!(
            matches!(result, Some(Value::Int(42)) | Some(Value::Bool(true))),
            "{result:?}"
        );
        // Measured at main 38fe092 on the stack and register VMs.
        assert_eq!(vm.fuel_budget().unwrap().used(), [20, 58][index]);
    }
}
