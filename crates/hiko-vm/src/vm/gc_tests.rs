//! Abstract-work, transaction and entry-lifetime regressions for #201/#242.
use super::tests::compile_vm as machine;
use super::*;
use crate::heap::GcStage;
use std::panic::{AssertUnwindSafe, catch_unwind};
fn garbage(vm: &mut VM, count: usize) {
    for _ in 0..count {
        vm.heap.alloc(HeapObject::String("garbage".into())).unwrap();
    }
}
fn reserved_vm(budget: &Arc<crate::budget::SharedBudget>, requested: u64) -> VM {
    let mut vm = machine("val unused = 0");
    vm.fuel_remaining = Some(budget.reserve_up_to(requested));
    vm.fuel_entry = Some(ActiveFuel {
        grant: Some(budget.clone()),
    });
    vm
}
fn debit(vm: &mut VM, cost: Option<u64>) -> Result<(), &'static str> {
    vm.fuel_entry
        .as_ref()
        .unwrap()
        .debit_gc(&mut vm.fuel_remaining, cost)
}
fn assert_settled(vm: &VM) {
    assert!(vm.fuel_remaining.is_none());
    assert!(vm.fuel_entry.is_none());
}
#[test]
fn credit_100_10_20_refunds_70_and_never_double_charges() {
    let budget = crate::budget::SharedBudget::new(1000);
    let mut vm = reserved_vm(&budget, 100);
    debit(&mut vm, Some(10)).unwrap();
    debit(&mut vm, Some(20)).unwrap();
    assert_eq!(vm.fuel_remaining, Some(70));
    assert_eq!(budget.used(), 100);
    drop(vm);
    assert_eq!(budget.used(), 30);
}
#[test]
fn deficit_is_atomic_before_local_credit_and_only_unspent_grant_refunds() {
    let budget = crate::budget::SharedBudget::new(105);
    let mut vm = reserved_vm(&budget, 100);
    assert!(debit(&mut vm, Some(110)).is_err());
    assert_eq!(vm.fuel_remaining, Some(100));
    assert_eq!(budget.used(), 100);
    drop(vm);
    assert_eq!(budget.used(), 0);
    let budget = crate::budget::SharedBudget::new(200);
    let mut vm = reserved_vm(&budget, 10);
    debit(&mut vm, Some(15)).unwrap();
    assert_eq!(vm.fuel_remaining, Some(0));
    drop(vm);
    assert_eq!(budget.used(), 15);
}
#[test]
fn credit_exact_zero_overflow_and_no_cap_quantum_are_distinct() {
    let budget = crate::budget::SharedBudget::new(u64::MAX);
    let mut vm = reserved_vm(&budget, u64::MAX);
    assert!(debit(&mut vm, None).is_err());
    assert_eq!(vm.fuel_remaining, Some(u64::MAX));
    debit(&mut vm, Some(u64::MAX)).unwrap();
    assert_eq!(vm.fuel_remaining, Some(0));
    assert!(debit(&mut vm, Some(1)).is_err());
    debit(&mut vm, Some(0)).unwrap();
    drop(vm);
    assert_eq!(budget.used(), u64::MAX);
    let mut quantum = machine("val unused = 0");
    {
        let entry = FuelEntry::new(&mut quantum, Some(1)).unwrap();
        debit(entry.vm, Some(20)).unwrap();
        assert_eq!(entry.vm.fuel_remaining, Some(0));
        debit(entry.vm, None).unwrap();
    }
    assert_settled(&quantum);
    let mut unlimited = machine("val unused = 0");
    {
        let entry = FuelEntry::new(&mut unlimited, None).unwrap();
        debit(entry.vm, None).unwrap();
        assert_eq!(entry.vm.fuel_remaining, None);
        assert!(entry.vm.fuel_entry.is_some());
    }
    assert_settled(&unlimited);
}
#[test]
fn sibling_leases_share_one_cap_and_refund_once() {
    let budget = crate::budget::SharedBudget::new(100);
    let barrier = std::sync::Barrier::new(2);
    std::thread::scope(|scope| {
        for _ in 0..2 {
            let budget = &budget;
            let barrier = &barrier;
            scope.spawn(move || {
                let mut vm = reserved_vm(budget, 60);
                barrier.wait();
                debit(&mut vm, Some(40)).unwrap();
            });
        }
    });
    assert_eq!(budget.used(), 80);
}
#[test]
fn actual_run_and_slice_unwind_refund_with_vm_surviving() {
    for slice in [false, true] {
        let mut vm = machine("val _ = panic \"rust panic\"");
        vm.register_builtin("panic", |_, _| panic!("injected Rust panic"));
        vm.set_fuel(100);
        let original = vm.fuel_budget().unwrap().clone();
        assert!(
            catch_unwind(AssertUnwindSafe(|| {
                if slice {
                    let _ = vm.run_slice(100);
                } else {
                    let _ = vm.run();
                }
            }))
            .is_err()
        );
        assert_settled(&vm);
        assert!(original.used() > 0 && original.used() < 100);
        let spent = original.used();
        vm.set_fuel(500);
        drop(vm);
        assert_eq!(original.used(), spent);
    }
}
#[test]
fn lease_refunds_the_original_arc_even_if_vm_budget_is_replaced() {
    let mut vm = machine("val x = 1");
    vm.set_fuel(100);
    let original = vm.fuel_budget().unwrap().clone();
    {
        let entry = FuelEntry::new(&mut vm, Some(100)).unwrap();
        debit(entry.vm, Some(30)).unwrap();
        entry.vm.fuel_budget = Some(crate::budget::SharedBudget::new(1000));
    }
    assert_settled(&vm);
    assert_eq!(original.used(), 30);
    assert_eq!(vm.fuel_budget().unwrap().used(), 0);
}
#[test]
fn set_fuel_and_repeated_settlement_clear_both_fields_and_refund_once() {
    let mut vm = machine("val unused = 0");
    vm.set_fuel(100);
    let original = vm.fuel_budget().unwrap().clone();
    {
        let entry = FuelEntry::new(&mut vm, Some(100)).unwrap();
        debit(entry.vm, Some(30)).unwrap();
        entry.vm.set_fuel(1000);
        assert_settled(entry.vm);
        assert_eq!(original.used(), 30);
        // Entry Drop will call the same helper again, without a second refund.
        entry.vm.settle_fuel_entry();
    }
    assert_settled(&vm);
    assert_eq!(original.used(), 30);
    assert_eq!(vm.fuel_budget().unwrap().used(), 0);
    drop(vm);
    assert_eq!(original.used(), 30);
}
#[test]
fn active_uncapped_gc_does_not_debit_a_replacement_budget() {
    let mut vm = machine("val unused = 0");
    garbage(&mut vm, 1);
    let replacement = crate::budget::SharedBudget::new(1000);
    {
        let entry = FuelEntry::new(&mut vm, None).unwrap();
        assert!(entry.vm.fuel_remaining.is_none());
        assert!(entry.vm.fuel_entry.is_some());
        entry.vm.fuel_budget = Some(replacement.clone());
        entry.vm.gc_collect_checked(None, &[], true).unwrap();
        assert_eq!(replacement.used(), 0);
    }
    assert_settled(&vm);
    garbage(&mut vm, 1);
    // Without an active entry, direct forced GC must charge the budget.
    vm.gc_collect_checked(None, &[], true).unwrap();
    assert!(replacement.used() > 0);
}
fn root_fixture() -> (VM, GcRef, Arc<crate::sendable::SendableValue>) {
    let mut vm = machine("val unused = 0");
    let leaf = vm.heap.alloc(HeapObject::String("live".into())).unwrap();
    let root = vm
        .heap
        .alloc(HeapObject::Tuple(
            vec![Value::Int(1), Value::Heap(leaf), Value::Heap(leaf)].into(),
        ))
        .unwrap();
    garbage(&mut vm, 1);
    vm.stack = vec![
        Value::Int(0),
        Value::Unit,
        Value::Heap(root),
        Value::Heap(root),
    ];
    vm.globals = vec![None, Some(Value::Int(7)), Some(Value::Heap(leaf))];
    vm.frames.push(CallFrame {
        proto_idx: usize::MAX,
        ip: 0,
        base: 0,
        captures: Captures::Closure(vec![Value::Unit, Value::Heap(leaf), Value::Heap(leaf)].into()),
    });
    vm.handlers.push(HandlerFrame {
        call_frame_idx: 0,
        stack_base: 0,
        clauses: Vec::new(),
        proto_idx: usize::MAX,
        captures: Captures::Closure(vec![Value::Unit, Value::Heap(root)].into()),
    });
    vm.hidden_handlers.push(HiddenHandlers {
        clause_frame_idx: 0,
        handlers: vec![HandlerFrame {
            call_frame_idx: 0,
            stack_base: 0,
            clauses: Vec::new(),
            proto_idx: usize::MAX,
            captures: Captures::Closure(vec![Value::Unit, Value::Heap(leaf)].into()),
        }],
    });
    vm.string_cache.rows = vec![vec![None, Some(leaf), None], vec![]];
    let (cached, fresh) =
        crate::sendable::serialize_shared(Value::Heap(root), &mut vm.heap).unwrap();
    assert!(fresh);
    (vm, root, cached)
}
#[test]
fn raw_root_containers_scalar_none_duplicate_edges_and_scalar_cached_nodes_are_paid() {
    let (mut vm, root, _) = root_fixture();
    vm.gc_test_trace = Some(Vec::new());
    vm.set_fuel(100_000);
    vm.gc_collect_checked(None, &[root, root], true).unwrap();
    let trace = vm.gc_test_trace.as_ref().unwrap();
    assert!(trace.contains(&(GcStage::Stack, Some(5))));
    assert!(trace.contains(&(GcStage::Globals, Some(4))));
    assert!(trace.contains(&(GcStage::Frames, Some(2))));
    assert!(trace.contains(&(GcStage::Captures, Some(4))));
    assert_eq!(
        trace
            .iter()
            .filter(|(s, n)| *s == GcStage::Captures && *n == Some(3))
            .count(),
        2
    );
    assert!(trace.contains(&(GcStage::HiddenHandlers, Some(2))));
    assert_eq!(
        trace
            .iter()
            .filter(|(s, _)| *s == GcStage::Cache)
            .map(|(_, n)| *n)
            .collect::<Vec<_>>(),
        vec![Some(3), Some(4), Some(1)]
    );
    assert!(trace.contains(&(GcStage::ExtraRoots, Some(3))));
    assert_eq!(
        trace
            .iter()
            .filter(|(s, _)| *s == GcStage::Reference)
            .count(),
        12
    );
    assert_eq!(
        trace
            .iter()
            .filter(|(s, _)| *s == GcStage::WorkItem)
            .count(),
        2
    );
    assert!(trace.contains(&(GcStage::Outgoing, Some(3))));
    assert!(trace.contains(&(GcStage::Outgoing, Some(0))));
    let footprint = 4 * std::mem::size_of::<crate::sendable::SendableValue>() + 8;
    let commit = trace
        .iter()
        .find(|(s, _)| *s == GcStage::Commit)
        .unwrap()
        .1
        .unwrap();
    assert!(commit > 2 * 3 + footprint as u64 + 1);
    assert_eq!(
        vm.fuel_budget().unwrap().used(),
        trace.iter().map(|(_, n)| n.unwrap()).sum::<u64>()
    );
    assert_eq!(vm.heap.live_count(), 2);
    assert!(vm.heap.sent(root).is_none());
}
#[test]
fn every_checked_scan_fault_is_atomic_and_first_cause_terminal() {
    for stage in [
        GcStage::Stack,
        GcStage::Globals,
        GcStage::Frames,
        GcStage::Captures,
        GcStage::Handlers,
        GcStage::HiddenHandlers,
        GcStage::Cache,
        GcStage::ExtraRoots,
        GcStage::ClearMarks,
        GcStage::Reference,
        GcStage::WorkItem,
        GcStage::Outgoing,
        GcStage::Commit,
    ] {
        let (mut vm, root, cached) = root_fixture();
        let before = (
            vm.heap.live_count(),
            vm.heap.live_bytes(),
            vm.heap.successful_allocations(),
        );
        vm.set_fuel(100_000);
        vm.gc_test_fault = Some(stage);
        let error = vm.gc_collect_checked(None, &[root], true).unwrap_err();
        assert_eq!(error.message, "GC scratch allocation failed", "{stage:?}");
        assert_eq!(
            before,
            (
                vm.heap.live_count(),
                vm.heap.live_bytes(),
                vm.heap.successful_allocations()
            )
        );
        assert!(Arc::ptr_eq(&cached, &vm.heap.sent(root).unwrap()));
        let frames = vm.frames.len();
        vm.set_fuel(200_000);
        vm.cancelled = true;
        vm.startup_error = Some("later startup failure".into());
        vm.pending_runtime_request = Some(RuntimeRequest::Await(99));
        assert_eq!(vm.run().unwrap_err().message, error.message);
        assert!(
            matches!(vm.run_slice(10), RunResult::Failed(ProcessFailure::RuntimeError(ref m)) if m == &error.message)
        );
        assert_eq!(
            vm.alloc(HeapObject::String("blocked".into()))
                .unwrap_err()
                .message,
            error.message
        );
        assert_eq!(vm.frames.len(), frames);
        assert_eq!(vm.fuel_budget().unwrap().used(), 0);
    }
}
#[test]
fn ordinary_gc_scratch_failure_is_terminal_without_publishing_allocation() {
    let mut vm = machine("val unused = 0");
    garbage(&mut vm, 1024);
    let before = (
        vm.heap.live_count(),
        vm.heap.live_bytes(),
        vm.heap.successful_allocations(),
    );
    vm.gc_test_fault = Some(GcStage::Commit);
    assert_eq!(
        vm.alloc(HeapObject::String("pending".into()))
            .unwrap_err()
            .message,
        "GC scratch allocation failed"
    );
    assert_eq!(
        before,
        (
            vm.heap.live_count(),
            vm.heap.live_bytes(),
            vm.heap.successful_allocations()
        )
    );
    assert_eq!(
        vm.run().unwrap_err().message,
        "GC scratch allocation failed"
    );
}
#[test]
fn pending_scalar_slots_are_prepaid_before_creating_extra_roots() {
    let mut vm = machine("val unused = 0");
    garbage(&mut vm, 1);
    vm.set_max_heap(1);
    vm.set_fuel(3);
    vm.gc_test_trace = Some(Vec::new());
    assert!(
        vm.alloc(HeapObject::Tuple(vec![Value::Unit; 8].into()))
            .unwrap_err()
            .is_fuel_exhausted()
    );
    assert_eq!(
        vm.gc_test_trace.as_ref().unwrap(),
        &vec![(GcStage::PendingObject, Some(9))]
    );
    assert_eq!(vm.heap.live_count(), 1);
    assert_eq!(vm.fuel_budget().unwrap().used(), 0);
}
#[test]
fn ordinary_boundary_fault_overrides_yield_but_done_never_adds_gc() {
    let mut vm = machine("val value = 1");
    garbage(&mut vm, 300);
    vm.gc_test_fault = Some(GcStage::Commit);
    vm.set_fuel(1000);
    assert!(
        matches!(vm.run_slice(1), RunResult::Failed(ProcessFailure::RuntimeError(ref m)) if m == "GC scratch allocation failed")
    );
    assert_settled(&vm);
    assert!(vm.fuel_budget().unwrap().used() < 1000);
    let mut done = machine("val value = 1");
    garbage(&mut done, 300);
    done.gc_test_fault = Some(GcStage::Commit);
    assert!(matches!(done.run_slice(1000), RunResult::Done));
    assert_eq!(done.gc_test_fault, Some(GcStage::Commit));
    assert_eq!(done.heap.live_count(), 300);
}

fn force_machine(bytes: bool, fuel: u64) -> VM {
    let mut vm = machine("val pair = (1, 2)");
    garbage(&mut vm, 5);
    if bytes {
        vm.set_max_memory_bytes(vm.heap.live_bytes());
    } else {
        vm.set_max_heap(5);
    }
    vm.set_fuel(fuel);
    vm.gc_test_trace = Some(Vec::new());
    vm
}
#[test]
fn forced_object_and_byte_gc_use_reserved_credit_exactly_in_run_and_slice() {
    for bytes in [false, true] {
        for slice in [false, true] {
            let mut plain = machine("val pair = (1, 2)");
            plain.set_fuel(100_000);
            if slice {
                assert!(matches!(plain.run_slice(100_000), RunResult::Done));
            } else {
                plain.run().unwrap();
            }
            let opcodes = plain.fuel_budget().unwrap().used();
            let mut vm = force_machine(bytes, 100_000);
            if slice {
                assert!(matches!(vm.run_slice(100_000), RunResult::Done));
            } else {
                vm.run().unwrap();
            }
            let gc: u64 = vm
                .gc_test_trace
                .as_ref()
                .unwrap()
                .iter()
                .map(|(_, n)| n.unwrap())
                .sum();
            assert!(gc > 0);
            assert_eq!(vm.fuel_budget().unwrap().used(), opcodes + gc);
            assert_settled(&vm);
            assert!(vm.gc_failure.is_none());
            let exact = opcodes + gc;
            let mut equal = force_machine(bytes, exact);
            equal.run().unwrap();
            assert_eq!(equal.fuel_budget().unwrap().used(), exact);
            // Stop immediately AFTER the allocating instruction, before its
            // tail opcodes: the one-short control must refuse GC, not a later op.
            let mut through_gc = force_machine(bytes, 100_000);
            assert!(matches!(through_gc.run_slice(3), RunResult::Yielded));
            assert_eq!(through_gc.heap.live_count(), 1);
            let through_gc_cost = through_gc.fuel_budget().unwrap().used();
            let mut short = force_machine(bytes, through_gc_cost - 1);
            assert!(short.run().unwrap_err().is_fuel_exhausted());
            assert!(short.gc_failure.is_some());
            assert_eq!(short.heap.live_count(), 5); // No precommit collection/publication.
        }
    }
}
#[test]
fn quantum_alone_and_persistent_deficit_can_overrun_synchronously_then_yield() {
    for persistent in [false, true] {
        let mut vm = machine("val pair = (1, 2)");
        garbage(&mut vm, 5);
        vm.set_max_heap(5);
        if persistent {
            vm.set_fuel(1000);
        }
        vm.gc_test_trace = Some(Vec::new());
        assert!(matches!(vm.run_slice(3), RunResult::Yielded));
        assert!(vm.gc_failure.is_none());
        assert_eq!(vm.heap.live_count(), 1);
        if persistent {
            assert!(vm.fuel_budget().unwrap().used() > 3);
        }
        assert_settled(&vm);
        assert!(matches!(vm.run_slice(1000), RunResult::Done));
        assert_settled(&vm);
    }
}
