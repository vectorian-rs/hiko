use super::super::tests::compile_vm as machine;
use super::*;
use crate::value::HostHandleKind;

fn invoke(vm: &mut VM, op: ReaderOp, arg: Value) -> Result<Value, RuntimeError> {
    // Production call_builtin retains its arguments here across preflight GC.
    vm.stack.push(arg);
    let result = vm.call_json_reader(op, arg);
    vm.stack.pop();
    result
}
fn open(vm: &mut VM, text: &str) -> Value {
    let input = Value::Heap(vm.heap.alloc(HeapObject::String(text.into())).unwrap());
    invoke(vm, ReaderOp::OfString, input).unwrap()
}

#[test]
fn bad_search_name_closes_a_live_owner_but_bad_close_handle_spends_no_work() {
    let mut vm = machine("val unused = 0");
    let r = open(&mut vm, "{}");
    let pair = Value::Heap(
        vm.heap
            .alloc(HeapObject::Tuple(smallvec::smallvec![Value::Int(7), r]))
            .unwrap(),
    );
    assert!(
        invoke(&mut vm, ReaderOp::FindNextField, pair)
            .unwrap_err()
            .message
            .contains("expected String")
    );
    assert_eq!(vm.heap.host_resource_count(), 0);
    let dangling = Value::Heap(
        vm.heap
            .alloc(HeapObject::HostHandle {
                kind: HostHandleKind::JsonReader,
                id: crate::value::HostHandleId(9999),
            })
            .unwrap(),
    );
    let before = vm.heap.host_work_used();
    assert!(invoke(&mut vm, ReaderOp::Close, dangling).is_err());
    assert_eq!(vm.heap.host_work_used(), before);
}

#[test]
fn display_is_opaque_and_identity_survives_closed_markers() {
    let mut vm = machine("val unused = 0");
    let a = open(&mut vm, "{\"secret\":37}");
    let b = open(&mut vm, "{}");
    assert_eq!(vm.display_value(&a), "<json-reader>");
    assert!(crate::vm::values_equal(a, a, &vm.heap));
    assert!(!crate::vm::values_equal(a, b, &vm.heap));
    invoke(&mut vm, ReaderOp::Close, a).unwrap();
    invoke(&mut vm, ReaderOp::Close, b).unwrap();
    assert!(crate::vm::values_equal(a, a, &vm.heap));
    assert!(!crate::vm::values_equal(a, b, &vm.heap));
    assert_eq!(vm.display_value(&a), "<json-reader>");
}

#[test]
fn finish_and_close_cycles_do_not_exhaust_32_slots() {
    for finish in [false, true] {
        let mut vm = machine("val unused = 0");
        vm.heap.set_max_host_resources(32);
        vm.heap.set_max_bytes(4_000_000);
        for _ in 0..64 {
            let reader = open(&mut vm, "[1,2]");
            invoke(
                &mut vm,
                if finish {
                    ReaderOp::Finish
                } else {
                    ReaderOp::Close
                },
                reader,
            )
            .unwrap();
            assert_eq!(vm.heap.host_resource_count(), 0);
            invoke(&mut vm, ReaderOp::Close, reader).unwrap();
            assert!(invoke(&mut vm, ReaderOp::Finish, reader).is_err());
        }
        assert_eq!(vm.heap.host_resource_peak_count(), 1);
    }
}

#[test]
fn resource_pressure_collects_unreachable_open_readers_before_cadence() {
    let mut vm = machine("val unused = 0");
    vm.heap.set_max_host_resources(2);
    open(&mut vm, "null");
    open(&mut vm, "null");
    assert_eq!(vm.heap.host_resource_count(), 2);
    assert!(!vm.heap.should_collect());
    let value = open(&mut vm, "[3]");
    assert_eq!(vm.heap.host_resource_count(), 1);
    invoke(&mut vm, ReaderOp::Finish, value).unwrap();
}

#[test]
fn reachable_reader_roots_survive_forced_collection_and_still_refuse_limit() {
    let mut vm = machine("val unused = 0");
    vm.heap.set_max_host_resources(1);
    let r = open(&mut vm, "[3]");
    vm.stack.push(r);
    let input = Value::Heap(vm.heap.alloc(HeapObject::String("null".into())).unwrap());
    let e = invoke(&mut vm, ReaderOp::OfString, input).unwrap_err();
    assert!(e.message.contains("host resource limit exceeded"));
    assert_eq!(vm.heap.host_resource_count(), 1);
    invoke(&mut vm, ReaderOp::BeginArray, r).unwrap();
    assert!(matches!(
        invoke(&mut vm, ReaderOp::NextItem, r).unwrap(),
        Value::Bool(true)
    ));
    assert!(matches!(
        invoke(&mut vm, ReaderOp::ReadWord, r).unwrap(),
        Value::Word(3)
    ));
    invoke(&mut vm, ReaderOp::Finish, r).unwrap();
}

#[test]
fn whole_input_proxy_tariff_fits_one_mib_in_ten_million_credits() {
    let mut vm = machine("val unused = 0");
    vm.heap.set_max_host_work(10_000_000);
    vm.heap.set_max_bytes(8_000_000);
    let input = Value::Heap(
        vm.heap
            .alloc(HeapObject::String(" ".repeat(1 << 20).into()))
            .unwrap(),
    );
    let r = invoke(&mut vm, ReaderOp::OfString, input).unwrap();
    assert_eq!(vm.heap.host_work_used(), 2_232_365);
    invoke(&mut vm, ReaderOp::Close, r).unwrap();
    assert_eq!(vm.heap.host_work_used(), 2_232_410);
}

#[test]
fn work_and_actual_output_refusals_close_valid_owner_without_double_refund() {
    for work in [false, true] {
        let mut vm = machine("val unused = 0");
        vm.heap.set_max_bytes(1_000_000);
        let r = open(&mut vm, r#""text""#);
        if work {
            vm.heap.set_max_host_work(vm.heap.host_work_used() + 44);
        } else {
            vm.heap.set_max_objects(0);
        }
        assert!(invoke(&mut vm, ReaderOp::ReadString, r).is_err());
        assert_eq!(vm.heap.host_resource_count(), 0);
        assert!(vm.heap.json_reader(r).is_err());
        let before = vm.heap.memory_budget().unwrap().used();
        vm.heap.close_json_reader(r).unwrap();
        assert_eq!(vm.heap.memory_budget().unwrap().used(), before);
    }
}

#[test]
fn checked_gc_failure_releases_payload_after_live_owner_dispatch() {
    let mut vm = machine("val unused = 0");
    vm.heap.set_max_bytes(1_000_000);
    let r = open(&mut vm, r#""text""#);
    vm.heap.set_max_objects(0);
    vm.gc_test_fault = Some(crate::heap::GcStage::Stack);
    let error = invoke(&mut vm, ReaderOp::ReadString, r).unwrap_err();
    assert!(error.message.contains("GC scratch allocation failed"));
    assert_eq!(vm.heap.host_resource_count(), 0);
    assert!(vm.gc_failed().is_err());
}

#[test]
fn scalar_alias_and_late_errors_close_every_alias() {
    let mut vm = machine("val unused = 0");
    let r = open(&mut vm, "[37,]");
    let alias = r;
    invoke(&mut vm, ReaderOp::BeginArray, r).unwrap();
    invoke(&mut vm, ReaderOp::NextItem, r).unwrap();
    assert!(matches!(
        invoke(&mut vm, ReaderOp::ReadInt, alias).unwrap(),
        Value::Int(37)
    ));
    let error = invoke(&mut vm, ReaderOp::Finish, alias).unwrap_err();
    assert!(error.message.contains("at byte 4"));
    assert_eq!(vm.heap.host_resource_count(), 0);
    assert!(invoke(&mut vm, ReaderOp::Kind, r).is_err());
}

#[test]
fn public_module_reads_nested_age_and_sums_array() {
    let program = crate::test_support::compile_source_hermetic(
        r#"
import Std.JsonReader
val r = JsonReader.of_string "{\"user\":{\"age\":37},\"events\":[10,20]}"
val _ = JsonReader.begin_object r
val found_user = JsonReader.find_next_field "user" r
val _ = JsonReader.begin_object r
val found_age = JsonReader.find_next_field "age" r
val age = JsonReader.read_int r
val _ = JsonReader.finish_object r
val found_events = JsonReader.find_next_field "events" r
val _ = JsonReader.begin_array r
fun sum total = if JsonReader.next_item r then sum (total + JsonReader.read_int r) else total
val total = sum 0
val _ = JsonReader.finish r
"#,
    );
    let mut vm = VM::new(program);
    vm.run().unwrap();
    for name in ["found_user", "found_age", "found_events"] {
        assert!(matches!(vm.get_global(name), Some(Value::Bool(true))));
    }
    assert!(matches!(vm.get_global("age"), Some(Value::Int(37))));
    assert!(matches!(vm.get_global("total"), Some(Value::Int(30))));
    assert_eq!(vm.heap.host_resource_count(), 0);
}

#[test]
fn rooted_reader_survives_output_pressure_on_stack_and_register_paths() {
    let program = crate::test_support::compile_source_hermetic(
        r#"
import Std.JsonReader
fun allocating r = let
    fun loop n = if n = 0 then () else let val _ = JsonReader.kind r in loop (n - 1) end
    val _ = loop 256
in 42 end
fun caller r = let val _ = allocating r in r end
val owner = caller (JsonReader.of_string "[1,2]")
val _ = JsonReader.finish owner
val total = 42
"#,
    );
    for _stack in [true, false] {
        let mut vm = VM::new(program.clone());
        #[cfg(feature = "register-vm")]
        if _stack {
            vm.register_code = Arc::new(RegisterCode::default());
        } else {
            let caller = vm
                .protos
                .iter()
                .position(|p| p.name.as_deref() == Some("caller"))
                .unwrap();
            assert!(vm.register_code.is_register(caller));
        }
        vm.heap.set_max_objects(100);
        vm.set_fuel(1_000_000_000);
        vm.set_max_host_work(1_000_000_000);
        vm.gc_test_trace = Some(Vec::new());
        vm.run().unwrap();
        assert!(matches!(vm.get_global("total"), Some(Value::Int(42))));
        assert_eq!(vm.heap.host_resource_count(), 0);
        assert!(!vm.gc_test_trace.as_ref().unwrap().is_empty());
    }
}

#[test]
fn parsed_text_survives_gc_while_owner_is_leased() {
    for (text, field, op, expected) in [
        (r#"["\u0061\u0062"]"#, false, ReaderOp::ReadString, "ab"),
        (r#"[1.00]"#, false, ReaderOp::ReadNumberText, "1.00"),
        (r#"{"\u0061\u0062":null}"#, true, ReaderOp::NextField, "ab"),
    ] {
        let mut vm = machine("val unused = 0");
        vm.heap.set_max_bytes(1_000_000);
        let r = open(&mut vm, text);
        invoke(
            &mut vm,
            if field {
                ReaderOp::BeginObject
            } else {
                ReaderOp::BeginArray
            },
            r,
        )
        .unwrap();
        if !field {
            invoke(&mut vm, ReaderOp::NextItem, r).unwrap();
        }
        // Two unreachable objects can fund the field's two output objects.
        vm.heap.alloc(HeapObject::String("garbage".into())).unwrap();
        vm.heap.set_max_objects(vm.heap.live_count());
        vm.gc_test_trace = Some(Vec::new());
        let Value::Heap(mut output) = invoke(&mut vm, op, r).unwrap() else {
            panic!()
        };
        if field {
            let HeapObject::Data { fields, .. } = vm.heap.get(output).unwrap() else {
                panic!()
            };
            let Value::Heap(s) = fields[0] else { panic!() };
            output = s;
        }
        let HeapObject::String(s) = vm.heap.get(output).unwrap() else {
            panic!()
        };
        assert_eq!(&**s, expected);
        assert!(!vm.gc_test_trace.as_ref().unwrap().is_empty());
        assert_eq!(vm.heap.host_resource_count(), 1);
        assert!(vm.heap.json_reader(r).is_ok());
        invoke(&mut vm, ReaderOp::Close, r).unwrap();
    }
}

#[test]
fn repeated_small_fields_do_not_force_gc_or_charge_unread_suffix() {
    let n = 128;
    let text = format!(
        "{{{}\"padding\":\"{}\"}}",
        "\"ab\":null,".repeat(n),
        "x".repeat(65_536)
    );
    let mut vm = machine("val unused = 0");
    vm.heap.set_max_bytes(1_000_000);
    let input = Value::Heap(vm.heap.alloc(HeapObject::String(text.into())).unwrap());
    vm.stack.push(input);
    let r = invoke(&mut vm, ReaderOp::OfString, input).unwrap();
    invoke(&mut vm, ReaderOp::BeginObject, r).unwrap();
    let before = vm.heap.live_bytes();
    let (_, field_bytes) = Heap::json_output_bound(2, true).unwrap();
    vm.heap.set_max_bytes(before + n * field_bytes);
    vm.heap.begin_test_phase();
    vm.gc_test_trace = Some(Vec::new());
    for i in 1..=n {
        invoke(&mut vm, ReaderOp::NextField, r).unwrap();
        invoke(&mut vm, ReaderOp::ReadNull, r).unwrap();
        assert_eq!(
            vm.heap.memory_budget().unwrap().used(),
            (before + i * field_bytes) as u64
        );
    }
    assert!(vm.gc_test_trace.as_ref().unwrap().is_empty());
    assert_eq!(vm.heap.end_test_phase(), before + n * field_bytes);
    invoke(&mut vm, ReaderOp::Close, r).unwrap();
}

#[test]
fn absent_field_admits_one_object_and_kind_admits_its_actual_name() {
    for (text, field, len) in [("{}", true, 0), ("null", false, 4)] {
        let mut vm = machine("val unused = 0");
        vm.heap.set_max_bytes(1_000_000);
        let input = Value::Heap(vm.heap.alloc(HeapObject::String(text.into())).unwrap());
        vm.stack.push(input);
        let r = invoke(&mut vm, ReaderOp::OfString, input).unwrap();
        if field {
            invoke(&mut vm, ReaderOp::BeginObject, r).unwrap();
        }
        let (_, actual) = Heap::json_output_bound(len, false).unwrap();
        let before = vm.heap.live_bytes();
        vm.heap.set_max_bytes(before + actual);
        invoke(
            &mut vm,
            if field {
                ReaderOp::NextField
            } else {
                ReaderOp::Kind
            },
            r,
        )
        .unwrap();
        assert_eq!(vm.heap.live_bytes(), before + actual);
        invoke(&mut vm, ReaderOp::Close, r).unwrap();
    }
}

#[test]
fn tiny_outputs_admit_actual_size_not_unread_input() {
    let padding = "x".repeat(32_768);
    for (text, field, op, len) in [
        (
            format!("[\"ok\",\"{padding}\"]"),
            false,
            ReaderOp::ReadString,
            2,
        ),
        (
            format!("[12,\"{padding}\"]"),
            false,
            ReaderOp::ReadNumberText,
            2,
        ),
        (
            format!("{{\"k\":0,\"padding\":\"{padding}\"}}"),
            true,
            ReaderOp::NextField,
            1,
        ),
    ] {
        let mut vm = machine("val unused = 0");
        vm.heap.set_max_bytes(1_000_000);
        let input = Value::Heap(vm.heap.alloc(HeapObject::String(text.into())).unwrap());
        vm.stack.push(input); // Keep the source object live as well as its native owner.
        let r = invoke(&mut vm, ReaderOp::OfString, input).unwrap();
        invoke(
            &mut vm,
            if field {
                ReaderOp::BeginObject
            } else {
                ReaderOp::BeginArray
            },
            r,
        )
        .unwrap();
        if !field {
            invoke(&mut vm, ReaderOp::NextItem, r).unwrap();
        }
        let before = vm.heap.live_bytes();
        let (_, actual) = Heap::json_output_bound(len, field).unwrap();
        vm.heap.set_max_bytes(before + actual);
        vm.gc_test_trace = Some(Vec::new());
        vm.heap.begin_test_phase();
        let output = invoke(&mut vm, op, r).unwrap();
        assert!(matches!(output, Value::Heap(_)));
        assert_eq!(vm.heap.live_bytes(), before + actual);
        assert_eq!(
            vm.heap.memory_budget().unwrap().used(),
            (before + actual) as u64
        );
        assert_eq!(vm.heap.end_test_phase(), before + actual);
        assert!(vm.gc_test_trace.as_ref().unwrap().is_empty());
        invoke(&mut vm, ReaderOp::Close, r).unwrap();
    }
}

#[test]
fn native_entries_are_appended_and_dispatch_only_through_the_vm() {
    let vm = machine("val unused = 0");
    for (name, _) in entries() {
        let id = vm.builtins.by_name[*name];
        assert!(matches!(
            vm.builtins.kinds[id as usize],
            BuiltinKind::JsonReader(_)
        ));
    }
}
