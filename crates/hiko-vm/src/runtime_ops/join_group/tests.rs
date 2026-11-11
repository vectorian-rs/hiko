use super::*;
#[cfg(feature = "builtin-process")]
use crate::runtime::Runtime;
#[cfg(feature = "builtin-process")]
use crate::threaded::ThreadedRuntime;
use crate::vm::VM;

fn vm() -> VM {
    VM::new(crate::test_support::compile_source_hermetic("val x = 0"))
}

fn list(vm: &mut VM, pids: &[u64]) -> Value {
    let elems = pids.iter().map(|&pid| Value::Pid(pid)).collect();
    crate::builtins::support::alloc_list(&mut vm.heap, elems).unwrap()
}

fn admit(vm: &mut VM, pids: &[u64], classify: impl FnMut(Pid) -> ChildState) -> JoinGroup {
    let input = list(vm, pids);
    let request = GroupAdmission::read(vm, input).unwrap();
    JoinGroup::admit(request, Pid(1), 1, vm.heap.max_message_bytes(), classify).unwrap()
}

#[test]
fn metadata_admission_exact_one_short_and_drop_refund() {
    let mut vm = vm();
    vm.configure_join_group_bytes(GROUP_BYTES + 2 * ENTRY_BYTES - 1);
    let input = list(&mut vm, &[2, 3]);
    assert!(
        GroupAdmission::read(&mut vm, input)
            .unwrap_err()
            .contains("max_join_group_bytes")
    );
    assert_eq!(vm.join_group_budget().used(), 0);
    vm.configure_join_group_bytes(GROUP_BYTES + 2 * ENTRY_BYTES);
    let request = GroupAdmission::read(&mut vm, input).unwrap();
    assert_eq!(vm.join_group_budget().used(), GROUP_BYTES + 2 * ENTRY_BYTES);
    drop(request);
    assert_eq!(vm.join_group_budget().used(), 0);
}

#[test]
fn empty_exempts_group_bytes_but_not_aggregate_envelope() {
    for (limit, succeeds) in [(2 * NODE_BYTES - 1, false), (2 * NODE_BYTES, true)] {
        let mut vm = vm();
        vm.configure_join_group_bytes(0);
        vm.set_max_message_bytes(limit);
        let group = admit(&mut vm, &[], |_| unreachable!());
        assert_eq!(vm.join_group_budget().used(), 0);
        vm.stack.push(Value::Unit);
        assert_eq!(group.deliver(&mut vm).is_ok(), succeeds);
        assert_eq!(vm.join_group_budget().used(), 0);
    }
}

#[test]
fn malformed_and_oversized_reject_before_outcome_admission_and_refund_metadata() {
    let mut vm = vm();
    assert!(GroupAdmission::read(&mut vm, Value::Int(0)).is_err());
    let input = list(&mut vm, &vec![2; MAX_JOIN_GROUP_ENTRIES + 1]);
    assert!(
        GroupAdmission::read(&mut vm, input)
            .unwrap_err()
            .contains("10000")
    );
    assert_eq!(vm.join_group_budget().used(), 0);
    let input = list(&mut vm, &vec![2; MAX_JOIN_GROUP_ENTRIES]);
    let request = GroupAdmission::read(&mut vm, input).unwrap();
    assert_eq!(request.pids.len(), MAX_JOIN_GROUP_ENTRIES);
    drop(request);
    assert_eq!(vm.join_group_budget().used(), 0);
}

#[test]
fn duplicate_precedes_ready_failure_and_cleanup_needs_no_fresh_work() {
    let mut vm = vm();
    let mut group = admit(&mut vm, &[2, 2, 3], |_| ChildState::Live);
    assert!(matches!(
        group.cause,
        Some(GroupCause::Typed(FiberJoinError::AlreadyJoined))
    ));
    assert_eq!(group.inbox.members, [Pid(2), Pid(3)]);
    vm.set_max_host_work(vm.heap.host_work_used());
    assert!(group.begin_cleanup());
    group.complete(
        Pid(2),
        ChildOutcome::Err(ProcessFailure::runtime("later child failure")),
        &mut vm.heap,
    );
    group.complete(
        Pid(3),
        ChildOutcome::Ok(SendableValue::Int(3)),
        &mut vm.heap,
    );
    assert!(group.ready());
    assert!(!group.begin_cleanup());
    assert!(matches!(
        group.cause,
        Some(GroupCause::Typed(FiberJoinError::AlreadyJoined))
    ));
    drop(group);
    assert_eq!(vm.join_group_budget().used(), 0);
}

#[test]
fn invalidity_tracks_only_distinct_positively_owned_named_children() {
    let mut vm = vm();
    let group = admit(&mut vm, &[2, 77, 3, 2, 88, 99], |pid| match pid.0 {
        2 | 3 => ChildState::Live,
        77 => ChildState::NotChild,
        88 => ChildState::Reaped,
        _ => ChildState::Unknown,
    });
    assert_eq!(group.inbox.members, [Pid(2), Pid(3)]);
    let Some(GroupCause::Typed(FiberJoinError::RuntimeError(message))) = &group.cause else {
        panic!("wrong cause");
    };
    assert!(message.contains("not a child"), "{message}");
    drop(group);
    assert_eq!(vm.join_group_budget().used(), 0);
}

#[test]
fn retained_successes_release_on_failure_and_credits_span_transfer() {
    let mut vm = vm();
    let mut group = admit(&mut vm, &[2, 3], |_| ChildState::Live);
    let metadata = vm.join_group_budget().used();
    group.complete(
        Pid(2),
        ChildOutcome::Ok(SendableValue::String(Arc::from("payload"))),
        &mut vm.heap,
    );
    assert_eq!(vm.join_group_budget().used(), metadata + NODE_BYTES + 7);
    group.complete(
        Pid(3),
        ChildOutcome::Err(ProcessFailure::Cancelled),
        &mut vm.heap,
    );
    assert!(group.begin_cleanup());
    assert_eq!(vm.join_group_budget().used(), metadata + 2 * NODE_BYTES);
    vm.stack.push(Value::Unit);
    group.deliver(&mut vm).unwrap();
    assert_eq!(vm.join_group_budget().used(), 0);
}

#[test]
fn result_storage_and_aggregate_have_independent_exact_boundaries() {
    let payload = NODE_BYTES + 8;
    let metadata = GROUP_BYTES + ENTRY_BYTES;
    for (group_limit, message_limit, succeeds) in [
        (metadata + payload, 2 * NODE_BYTES + payload, true),
        (metadata + payload - 1, 2 * NODE_BYTES + payload, false),
        (metadata + payload, 2 * NODE_BYTES + payload - 1, false),
    ] {
        let mut vm = vm();
        vm.configure_join_group_bytes(group_limit);
        vm.set_max_message_bytes(message_limit);
        let mut group = admit(&mut vm, &[2], |_| ChildState::Live);
        group.complete(
            Pid(2),
            ChildOutcome::Ok(SendableValue::Int(42)),
            &mut vm.heap,
        );
        vm.stack.push(Value::Unit);
        assert_eq!(group.deliver(&mut vm).is_ok(), succeeds);
        assert_eq!(vm.join_group_budget().used(), 0);
    }
}

#[test]
fn tree_shares_group_budget_and_rejects_second_group_without_resetting_usage() {
    let mut vm = vm();
    vm.configure_join_group_bytes(GROUP_BYTES + ENTRY_BYTES);
    let input = list(&mut vm, &[2]);
    let request = GroupAdmission::read(&mut vm, input).unwrap();
    let mut child = vm.create_child();
    assert!(Arc::ptr_eq(
        vm.join_group_budget(),
        child.join_group_budget()
    ));
    let input = list(&mut child, &[3]);
    assert!(GroupAdmission::read(&mut child, input).is_err());
    assert_eq!(vm.join_group_budget().used(), GROUP_BYTES + ENTRY_BYTES);
    drop(request);
    assert_eq!(child.join_group_budget().used(), 0);
    assert!(GroupAdmission::read(&mut child, input).is_ok());
}

#[test]
fn chosen_cause_survives_failed_err_encoding() {
    let mut vm = vm();
    let mut group = admit(&mut vm, &[2, 2], |_| ChildState::Live);
    group.complete(
        Pid(2),
        ChildOutcome::Ok(SendableValue::Int(0)),
        &mut vm.heap,
    );
    vm.set_max_heap(0);
    vm.stack.push(Value::Unit);
    let error = group.deliver(&mut vm).unwrap_err().to_string();
    assert!(
        error.contains("result unavailable/reaped; ownership no longer known"),
        "{error}"
    );
    assert!(error.contains("transfer failed"), "{error}");
    assert_eq!(vm.join_group_budget().used(), 0);
}

#[test]
fn reverse_completions_preserve_order_with_linear_work_and_heap_growth() {
    let mut previous = None;
    for n in [256usize, 512, 1024] {
        let mut vm = vm();
        let pids: Vec<_> = (2..2 + n as u64).collect();
        let before = vm.heap.successful_allocations();
        let work = vm.heap.host_work_used();
        let mut group = admit(&mut vm, &pids, |_| ChildState::Live);
        assert_eq!(group.pending_pids().count(), n); // initial registration pass
        for &pid in pids.iter().rev() {
            group.complete(
                Pid(pid),
                ChildOutcome::Ok(SendableValue::Int(pid as i64)),
                &mut vm.heap,
            );
        }
        let member_work = group.member_work();
        assert!(
            member_work <= 3 * n as u64 + 8,
            "per-event member rescan: n={n}, member_work={member_work}"
        );
        vm.stack.push(Value::Unit);
        group.deliver(&mut vm).unwrap();
        let output = crate::sendable::serialize(*vm.stack.last().unwrap(), &vm.heap).unwrap();
        let SendableValue::Data { fields, .. } = &output else {
            panic!("not Ok");
        };
        let SendableValue::List(values) = &fields[0] else {
            panic!("not a list");
        };
        assert_eq!(values.len(), n);
        for (i, value) in values.iter().enumerate() {
            assert!(matches!(value, SendableValue::Int(v) if *v == i as i64 + 2));
        }
        let allocations = vm.heap.successful_allocations() - before;
        let work = vm.heap.host_work_used() - work;
        eprintln!(
            "bulk growth n={n} successful_allocations={allocations} host_work={work} instrumented_member_work={member_work}"
        );
        if let Some((prior_allocations, prior_work)) = previous {
            assert!(allocations <= 2 * prior_allocations + 8);
            assert!(work <= 2 * prior_work + 8);
        }
        previous = Some((allocations, work));
        assert_eq!(vm.join_group_budget().used(), 0);
    }
}

#[test]
fn host_work_failure_releases_successes_then_drains_without_new_admission() {
    let mut vm = vm();
    let mut group = admit(&mut vm, &[2, 3, 4], |_| ChildState::Live);
    group.complete(
        Pid(2),
        ChildOutcome::Ok(SendableValue::Int(1)),
        &mut vm.heap,
    );
    vm.set_max_host_work(vm.heap.host_work_used());
    group.complete(
        Pid(3),
        ChildOutcome::Ok(SendableValue::Int(2)),
        &mut vm.heap,
    );
    assert!(matches!(group.cause, Some(GroupCause::Fatal(_))));
    assert!(group.begin_cleanup());
    assert_eq!(vm.join_group_budget().used(), GROUP_BYTES + 3 * ENTRY_BYTES);
    group.complete(
        Pid(4),
        ChildOutcome::Ok(SendableValue::Int(3)),
        &mut vm.heap,
    );
    assert!(group.ready());
    assert!(group.deliver(&mut vm).is_err());
    assert_eq!(vm.join_group_budget().used(), 0);
}

#[test]
fn partial_parent_transfer_is_not_published_and_refunds_once() {
    let mut vm = vm();
    let mut group = admit(&mut vm, &[2, 3, 4], |_| ChildState::Live);
    for pid in [2, 3, 4] {
        group.complete(
            Pid(pid),
            ChildOutcome::Ok(SendableValue::String(Arc::from("value"))),
            &mut vm.heap,
        );
    }
    vm.set_max_heap(vm.heap.live_count() + 1);
    vm.stack.push(Value::Unit);
    assert!(group.deliver(&mut vm).is_err());
    assert!(matches!(vm.stack.as_slice(), [Value::Unit]));
    assert_eq!(vm.join_group_budget().used(), 0);
}

#[test]
fn fixed_payload_member_growth_and_separate_byte_growth_report_logical_costs() {
    for (label, profiles) in [
        (
            "fixed-B",
            vec![(256usize, 2048usize), (512, 2048), (1024, 2048)],
        ),
        ("fixed-n", vec![(8, 4096), (8, 8192), (8, 16384)]),
    ] {
        let mut previous = None;
        for (n, bytes) in profiles {
            let mut vm = vm();
            let before = vm.heap.successful_allocations();
            let work = vm.heap.host_work_used();
            vm.heap.begin_test_phase();
            let pids: Vec<_> = (2..2 + n as u64).collect();
            let mut group = admit(&mut vm, &pids, |_| ChildState::Live);
            for &pid in pids.iter().rev() {
                let value = if pid == 2 {
                    SendableValue::String("x".repeat(bytes).into())
                } else {
                    SendableValue::Unit
                };
                group.complete(Pid(pid), ChildOutcome::Ok(value), &mut vm.heap);
            }
            let retained = vm.join_group_budget().used();
            vm.stack.push(Value::Unit);
            group.deliver(&mut vm).unwrap();
            let peak = vm.heap.end_test_phase();
            let allocations = vm.heap.successful_allocations() - before;
            let work = vm.heap.host_work_used() - work;
            eprintln!(
                "bulk {label} n={n} B={bytes} successful_allocations={allocations} host_work={work} phase_peak_estimated_heap_bytes={peak} retained_logical_group_bytes={retained}"
            );
            if let Some((prior_allocations, prior_work, prior_peak, prior_retained)) = previous {
                assert!(allocations <= 2 * prior_allocations + 8);
                assert!(work <= 2 * prior_work + 8);
                assert!(peak <= 2 * prior_peak + 1024);
                assert!(retained <= 2 * prior_retained + 1024);
            }
            previous = Some((allocations, work, peak, retained));
            assert_eq!(vm.join_group_budget().used(), 0);
        }
    }
}

#[cfg(feature = "builtin-process")]
#[test]
fn real_runtimes_bulk_order_duplicates_failures_and_no_bookkeeping_leaks() {
    super::runtime_tests::bounded!();
    let source = r#"
        import __Builtin.Process
        val left = spawn (fn () => 10)
        val right = spawn (fn () => 20)
        val _ = case BuiltinProcess.await_all_raw [left, right] of
            BuiltinProcess.Ok [10, 20] => ()
          | _ => panic "order"
        val owned = spawn (fn () => 30)
        val _ = case BuiltinProcess.await_all_raw [owned, owned] of
            BuiltinProcess.Err BuiltinProcess.AlreadyJoined => ()
          | _ => panic "duplicate"
        val _ = case BuiltinProcess.await_all_raw [owned] of
            BuiltinProcess.Err BuiltinProcess.AlreadyJoined => ()
          | _ => panic "reaped"
        val failed = spawn (fn () => panic "chosen")
        val peer = spawn (fn () => 42)
        val _ = case BuiltinProcess.await_all_raw [failed, peer] of
            BuiltinProcess.Err (BuiltinProcess.RuntimeError "chosen") => ()
          | _ => panic "failure"
        val _ = println "bulk ok"
    "#;
    let program = crate::test_support::compile_source_hermetic(source);
    let mut runtime = Runtime::new();
    let pid = runtime.spawn_root(program.clone());
    runtime.run_to_completion().unwrap();
    let report = runtime.lifecycle_report(pid);
    assert_eq!(report.status, "Done", "{report:?}");
    assert_eq!(report.output, ["bulk ok\n"]);
    assert!(report.leftovers.is_empty(), "{report:?}");
    for run in 0..40 {
        let runtime = ThreadedRuntime::new(1 + run % 4);
        let pid = runtime.spawn_root(program.clone());
        runtime.run_to_completion().unwrap();
        let report = runtime.lifecycle_report(pid);
        assert_eq!(report.status, "Done", "run{run}: {report:?}");
        assert_eq!(report.output, ["bulk ok\n"]);
        assert!(report.leftovers.is_empty(), "run{run}: {report:?}");
    }
}
