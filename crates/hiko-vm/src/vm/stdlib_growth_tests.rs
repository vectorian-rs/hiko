//! Fixed profiles for #203, not wall-clock benchmarks or global quota minima.
//!
//! Setup imports and tail-builds input, roots both input and thunk in globals,
//! completes main, then collects with normal VM roots. Only that completed main's
//! frames/stack are removed. Input remains rooted for the ENTIRE operation and
//! output stays on the VM stack during iterative Rust validation. This deliberately
//! retains more than a consuming caller might. Result profiles include constant
//! elimination of the outer Result. Log keys/values have fixed one-byte width.
//!
//! Each phase uses one completed, generously fueled slice. Snapshots precede Rust
//! validation: weighted stack-opcode fuel and host-work units are separate, not
//! register instruction counts/time. Successful GC-object allocations are cumulative
//! across GC; phase peak is absolute estimated retained bytes INCLUDING garbage,
//! initialized at entry bytes. Lifetime peak is never reset; neither peak is RSS.
//!
//! Growth uses 25k/50k/100k. Fresh quota trials use a separately labeled 1k profile
//! to keep repeated CI searches bounded, varying just one absolute quota at a time.
//! Expansion caps are 4M objects/512MiB, followed by bisection and reproduction of
//! an adjacent observed fail/pass. GC is not proved monotone: this is NOT a global
//! minimum or a CLI minimum. Each phase pass is separately verified from VM creation
//! (expanded if setup needs more); 100k growth runs also use creation-time caps.
//! No exporter, GC/scheduling changes, refunds, or live-count allocation estimates.

use super::*;
use crate::test_support::compile_source_hermetic;

const WORK: u64 = 500_000_000;
const OBJECT_CAP: usize = 4_000_000;
const BYTE_CAP: usize = 512 * 1024 * 1024;
const QUOTA_SIZE: usize = 1_000;

#[derive(Clone, Copy, Debug)]
enum Output {
    Count,
    FirstError,
    List,
    Append,
    Pairs,
    Parts,
    Text,
}

struct Profile {
    name: &'static str,
    element: &'static str,
    expression: &'static str,
    output: Output,
}

const PROFILES: &[Profile] = &[
    Profile {
        name: "length",
        element: "n",
        expression: "List.length input",
        output: Output::Count,
    },
    Profile {
        name: "map",
        element: "n",
        expression: "List.map (fn x => x) input",
        output: Output::List,
    },
    Profile {
        name: "filter",
        element: "n",
        expression: "List.filter (fn _ => true) input",
        output: Output::List,
    },
    Profile {
        name: "append",
        element: "n",
        expression: "List.append input [~1]",
        output: Output::Append,
    },
    Profile {
        name: "take",
        element: "n",
        expression: "List.take size input",
        output: Output::List,
    },
    Profile {
        name: "zip",
        element: "n",
        expression: "List.zip input input",
        output: Output::Pairs,
    },
    Profile {
        name: "foldr",
        element: "n",
        expression: "List.foldr (fn (_, acc) => acc + 1) 0 input",
        output: Output::Count,
    },
    Profile {
        name: "all",
        element: "Result.Ok n",
        expression: "Result.unwrap_or_else (fn _ => panic \"unexpected Err\") (Result.all input)",
        output: Output::List,
    },
    Profile {
        name: "sequence",
        element: "Result.Ok n",
        expression: "Result.unwrap_or_else (fn _ => panic \"unexpected Err\") (Result.sequence input)",
        output: Output::List,
    },
    Profile {
        name: "traverse",
        element: "n",
        expression: "Result.unwrap_or_else (fn _ => panic \"unexpected Err\") (Result.traverse (fn x => Result.Ok x) input)",
        output: Output::List,
    },
    Profile {
        name: "validate_all",
        element: "Result.Ok n",
        expression: "Result.unwrap_or_else (fn _ => panic \"unexpected Err\") (Result.validate_all input)",
        output: Output::List,
    },
    Profile {
        name: "validate_traverse",
        element: "n",
        expression: "Result.unwrap_or_else (fn _ => panic \"unexpected Err\") (Result.validate_traverse (fn x => Result.Ok x) input)",
        output: Output::List,
    },
    Profile {
        name: "validate_all_errors",
        element: "Result.Err n",
        expression: "Result.fold (fn _ => panic \"unexpected Ok\") (fn errs => errs) (Result.validate_all input)",
        output: Output::List,
    },
    Profile {
        name: "validate_traverse_errors",
        element: "n",
        expression: "Result.fold (fn _ => panic \"unexpected Ok\") (fn errs => errs) (Result.validate_traverse (fn x => Result.Err x) input)",
        output: Output::List,
    },
    Profile {
        name: "all_first_error",
        element: "if n = 1 then Result.Err n else Result.Ok n",
        expression: "Result.fold (fn _ => panic \"unexpected Ok\") (fn err => err) (Result.all input)",
        output: Output::FirstError,
    },
    Profile {
        name: "sequence_first_error",
        element: "if n = 1 then Result.Err n else Result.Ok n",
        expression: "Result.fold (fn _ => panic \"unexpected Ok\") (fn err => err) (Result.sequence input)",
        output: Output::FirstError,
    },
    Profile {
        name: "traverse_first_error",
        element: "n",
        expression: "Result.fold (fn _ => panic \"unexpected Ok\") (fn err => err) (Result.traverse (fn x => Result.Err x) input)",
        output: Output::FirstError,
    },
    Profile {
        name: "field_parts",
        element: "(\"k\", \"v\")",
        expression: "Log.field_parts input",
        output: Output::Parts,
    },
    Profile {
        name: "kv",
        element: "(\"k\", \"v\")",
        expression: "Log.kv \"m\" input",
        output: Output::Text,
    },
];

#[derive(Clone, Copy, Debug)]
enum Engine {
    Stack,
    #[cfg(feature = "register-vm")]
    Translated,
}

fn engines() -> Vec<Engine> {
    vec![
        Engine::Stack,
        #[cfg(feature = "register-vm")]
        Engine::Translated,
    ]
}

fn vm_for(program: &CompiledProgram, engine: Engine) -> VM {
    #[cfg(feature = "register-vm")]
    let mut vm = {
        let codes = register_check::both_codes(program).unwrap();
        let code = match engine {
            Engine::Stack => &codes[0],
            Engine::Translated => &codes[1],
        };
        VM::from_parts(program.clone(), None, Arc::clone(code))
    };
    #[cfg(not(feature = "register-vm"))]
    let mut vm = {
        let _ = engine;
        VM::new(program.clone())
    };
    vm.register_builtins();
    vm
}

fn program(profile: &Profile, size: usize) -> CompiledProgram {
    compile_source_hermetic(&format!(
        "import Std.List\nimport Std.Result\nimport Std.Log\n\
         val size = {size}\n\
         fun build (n, acc) = if n = 0 then acc else build (n - 1, ({}) :: acc)\n\
         val input = build (size, [])\n\
         val thunk = fn () => {}\n",
        profile.element, profile.expression
    ))
}

#[derive(Clone, Copy, Debug)]
enum Quota {
    Objects,
    Bytes,
}

impl Quota {
    fn cap(self) -> usize {
        match self {
            Self::Objects => OBJECT_CAP,
            Self::Bytes => BYTE_CAP,
        }
    }

    fn install(self, vm: &mut VM, limit: usize) {
        match self {
            Self::Objects => vm.set_max_heap(limit),
            Self::Bytes => vm.set_max_memory_bytes(limit),
        }
    }

    fn entry(self, vm: &VM) -> usize {
        match self {
            Self::Objects => vm.heap.live_count(),
            Self::Bytes => vm.heap.live_bytes(),
        }
    }

    fn expected_failure(self, failure: &ProcessFailure) -> bool {
        match (self, failure) {
            (Self::Objects, ProcessFailure::HeapObjectLimitExceeded { .. }) => true,
            (Self::Bytes, ProcessFailure::RuntimeError(message)) => message
                .strip_prefix("string_join: ")
                .unwrap_or(message)
                .starts_with("memory limit exceeded:"),
            _ => false,
        }
    }
}

fn prepared(
    program: &CompiledProgram,
    engine: Engine,
    creation_quota: Option<(Quota, usize)>,
) -> Result<VM, ProcessFailure> {
    let mut vm = vm_for(program, engine);
    vm.set_max_heap(OBJECT_CAP);
    vm.set_max_memory_bytes(BYTE_CAP);
    if let Some((quota, limit)) = creation_quota {
        quota.install(&mut vm, limit);
    }
    match vm.run_slice(WORK) {
        RunResult::Done => (),
        RunResult::Failed(failure) => return Err(failure),
        other => panic!("setup did not complete: {other:?}"),
    }
    assert!(vm.pending_runtime_request.is_none());
    assert!(vm.handlers.is_empty() && vm.hidden_handlers.is_empty());
    assert!(
        !vm.cancelled
            && vm.fuel_remaining.is_none()
            && vm.fuel_entry.is_none()
            && vm.fuel_budget.is_none()
    );
    #[cfg(feature = "register-vm")]
    assert_eq!(vm.register_paid, 0);
    // Only a completed main frame may be discarded; no suspended call state.
    assert_eq!(vm.frames.len(), 1);
    assert_eq!(vm.frames[0].proto_idx, usize::MAX);
    vm.frames.clear();
    vm.stack.clear();
    for name in ["input", "thunk"] {
        let slot = vm
            .global_names
            .iter()
            .position(|s| s.as_ref() == name)
            .unwrap();
        assert!(matches!(vm.globals[slot], Some(Value::Heap(_))));
    }
    vm.gc_collect_with_extra_roots(std::iter::empty()).unwrap();
    Ok(vm)
}

#[derive(Debug)]
struct Sample {
    fuel: u64,
    host_work: u64,
    allocations: u64,
    entry_objects: usize,
    entry_bytes: usize,
    phase_peak_bytes: usize,
    lifetime_peak_bytes: usize,
}

fn invoke(vm: &mut VM) -> Result<Sample, ProcessFailure> {
    let slot = vm
        .global_names
        .iter()
        .position(|s| s.as_ref() == "thunk")
        .unwrap();
    let Some(Value::Heap(reference)) = vm.globals[slot] else {
        panic!("missing thunk")
    };
    let HeapObject::Closure {
        proto_idx,
        captures,
    } = vm.heap.get(reference).unwrap()
    else {
        panic!("not a closure")
    };
    let (proto_idx, captures) = (*proto_idx, Arc::clone(captures));
    let allocations = vm.heap.successful_allocations();
    let host_work = vm.heap.host_work_used();
    let entry_objects = vm.heap.live_count();
    let entry_bytes = vm.heap.live_bytes();
    vm.heap.begin_test_phase();
    vm.set_max_work(WORK);
    vm.setup_closure_call(proto_idx, &captures);
    let outcome = vm.run_slice(WORK);
    let sample = Sample {
        fuel: vm.fuel_budget.as_ref().unwrap().used(),
        host_work: vm.heap.host_work_used() - host_work,
        allocations: vm.heap.successful_allocations() - allocations,
        entry_objects,
        entry_bytes,
        phase_peak_bytes: vm.heap.end_test_phase(),
        lifetime_peak_bytes: vm.heap.peak_bytes(),
    };
    match outcome {
        RunResult::Done => {
            assert!(vm.frames.is_empty());
            assert_eq!(vm.stack.len(), 1);
            assert!(sample.entry_objects > 0);
            assert!(sample.phase_peak_bytes >= sample.entry_bytes);
            assert!(sample.lifetime_peak_bytes >= sample.phase_peak_bytes);
            Ok(sample)
        }
        RunResult::Failed(failure) => Err(failure),
        other => panic!("operation did not complete: {other:?}"),
    }
}

fn int(value: Value) -> i64 {
    match value {
        Value::Int(n) => n,
        other => panic!("expected integer: {other:?}"),
    }
}

fn text<'a>(vm: &'a VM, value: &Value) -> &'a str {
    match value {
        Value::Heap(reference) => match vm.heap.get(*reference).unwrap() {
            HeapObject::String(s) => s,
            other => panic!("expected string: {other:?}"),
        },
        other => panic!("expected heap value: {other:?}"),
    }
}

fn validate(vm: &VM, profile: &Profile, size: usize) {
    let value = *vm.stack.last().unwrap();
    if matches!(profile.output, Output::Count | Output::FirstError) {
        let expected = if matches!(profile.output, Output::FirstError) {
            1
        } else {
            size as i64
        };
        assert_eq!(int(value), expected);
        return;
    }
    if matches!(profile.output, Output::Text) {
        let bytes = text(vm, &value).as_bytes();
        assert_eq!(bytes.len(), 1 + size * 4);
        assert_eq!(bytes[0], b'm');
        let (chunks, remainder) = bytes[1..].as_chunks::<4>();
        assert!(remainder.is_empty());
        assert!(chunks.iter().all(|chunk| chunk == b" k=v"));
        return;
    }
    let mut tail = value;
    let mut count = 0;
    loop {
        let Value::Heap(reference) = tail else {
            panic!("expected list")
        };
        let HeapObject::Data { tag, fields } = vm.heap.get(reference).unwrap() else {
            panic!("expected list data")
        };
        if *tag == TAG_NIL {
            assert!(fields.is_empty());
            break;
        }
        assert_eq!(*tag, TAG_CONS);
        assert_eq!(fields.len(), 2);
        match profile.output {
            Output::Pairs => {
                let Value::Heap(pair) = fields[0] else {
                    panic!("expected pair")
                };
                let HeapObject::Tuple(pair) = vm.heap.get(pair).unwrap() else {
                    panic!("expected tuple")
                };
                assert_eq!(pair.len(), 2);
                assert_eq!(int(pair[0]), (count + 1) as i64);
                assert_eq!(int(pair[1]), (count + 1) as i64);
            }
            Output::Parts => assert_eq!(text(vm, &fields[0]), [" ", "k", "=", "v"][count % 4]),
            Output::Append if count == size => assert_eq!(int(fields[0]), -1),
            _ => assert_eq!(int(fields[0]), (count + 1) as i64),
        }
        count += 1;
        assert!(count <= size * 4 + 1, "cyclic/oversized output");
        tail = fields[1];
    }
    let expected = match profile.output {
        Output::Parts => size * 4,
        Output::Append => size + 1,
        _ => size,
    };
    assert_eq!(count, expected, "{}", profile.name);
}

#[test]
fn stdlib_semantic_controls_and_effect_order() {
    let program = crate::test_support::compile_file_hermetic(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/run/test_stdlib_stack_safe.hml"),
    );
    let expected = [
        "map",
        "1",
        "2",
        "3",
        "filter",
        "1",
        "2",
        "3",
        "foldr",
        "3",
        "2",
        "1",
        "traverse",
        "1",
        "2",
        "validate",
        "1",
        "2",
        "3",
        "4",
        "5",
        "effect-map",
        "1",
        "2",
        "3",
    ];
    for engine in engines() {
        let mut vm = vm_for(&program, engine);
        vm.enable_output_capture();
        assert!(matches!(vm.run_slice(WORK), RunResult::Done));
        let expected: Vec<_> = expected.iter().map(|line| format!("{line}\n")).collect();
        assert_eq!(vm.get_output(), expected, "{engine:?}");
    }
}

#[test]
fn stdlib_callback_failure_order() {
    for (call, expected) in [
        ("List.map visit [1, 2, 3]", vec!["1", "2"]),
        (
            "List.filter (fn x => visit x = 1) [1, 2, 3]",
            vec!["1", "2"],
        ),
        (
            "Result.traverse (fn x => Result.Ok (visit x)) [1, 2, 3]",
            vec!["1", "2"],
        ),
        (
            "Result.validate_traverse (fn x => Result.Ok (visit x)) [1, 2, 3]",
            vec!["1", "2"],
        ),
        (
            "List.foldr (fn (x, acc) => visit x + acc) 0 [1, 2, 3]",
            vec!["3", "2"],
        ),
    ] {
        let program = compile_source_hermetic(&format!(
            "import Std.List\nimport Std.Result\n\
             fun visit (x : int) = let val _ = println (int_to_string x) in\n\
             if x = 2 then panic \"sentinel\" else x end\n\
             val answer = {call}"
        ));
        for engine in engines() {
            let mut vm = vm_for(&program, engine);
            vm.enable_output_capture();
            match vm.run_slice(WORK) {
                RunResult::Failed(ProcessFailure::RuntimeError(message)) => {
                    assert_eq!(message, "sentinel")
                }
                other => panic!("wrong failure for {call}: {other:?}"),
            }
            let expected: Vec<_> = expected.iter().map(|line| format!("{line}\n")).collect();
            assert_eq!(vm.get_output(), expected, "{engine:?} {call}");
        }
    }
}

// A separately labeled feasible comparison for the immutable pre-change library.
// Run with baseline module sources/pins as well as the candidate; never assign
// measurements to a failed large baseline run.
#[test]
fn stdlib_small_profile_comparison() {
    for engine in engines() {
        for profile in PROFILES {
            let program = program(profile, QUOTA_SIZE);
            let mut vm = prepared(&program, engine, None).unwrap();
            let sample = invoke(&mut vm).unwrap();
            validate(&vm, profile, QUOTA_SIZE);
            println!(
                "small-profile engine={engine:?} op={} n={QUOTA_SIZE} {sample:?}",
                profile.name
            );
        }
    }
}

#[test]
fn stdlib_growth_and_100k_scale() {
    for engine in engines() {
        for profile in PROFILES {
            let mut previous = None;
            for size in [25_000, 50_000, 100_000] {
                let program = program(profile, size);
                let mut vm = prepared(&program, engine, None).unwrap();
                let sample = invoke(&mut vm)
                    .unwrap_or_else(|e| panic!("{} {size} {engine:?}: {e:?}", profile.name));
                validate(&vm, profile, size);
                let total = sample.fuel + sample.host_work;
                if let Some(prior) = previous {
                    assert!(
                        10 * total <= 22 * prior + 320,
                        "{}: {prior} -> {total}",
                        profile.name
                    );
                }
                assert!(sample.allocations > 0);
                println!(
                    "growth engine={engine:?} op={} n={size} {sample:?}",
                    profile.name
                );
                previous = Some(total);
            }
        }
    }
}

fn trial(
    program: &CompiledProgram,
    engine: Engine,
    quota: Quota,
    limit: usize,
    from_creation: bool,
    profile: &Profile,
) -> bool {
    let creation = from_creation.then_some((quota, limit));
    let mut vm = match prepared(program, engine, creation) {
        Ok(vm) => vm,
        Err(failure) => {
            assert!(
                from_creation && quota.expected_failure(&failure),
                "unexpected setup error: {failure:?}"
            );
            return false;
        }
    };
    assert!(
        from_creation || limit >= quota.entry(&vm),
        "infeasible phase setup"
    );
    if !from_creation {
        quota.install(&mut vm, limit);
    }
    match invoke(&mut vm) {
        Ok(_) => {
            validate(&vm, profile, QUOTA_SIZE);
            true
        }
        Err(failure) => {
            assert!(
                quota.expected_failure(&failure),
                "wrong quota failure: {failure:?}"
            );
            println!(
                "quota-error engine={engine:?} op={} {quota:?} limit={limit} from-creation={from_creation} cause={failure:?}",
                profile.name
            );
            false
        }
    }
}

#[test]
fn stdlib_observed_quota_transitions() {
    for engine in engines() {
        for profile in PROFILES {
            let program = program(profile, QUOTA_SIZE);
            for quota in [Quota::Objects, Quota::Bytes] {
                let vm = prepared(&program, engine, None).unwrap();
                let entry = quota.entry(&vm);
                drop(vm);
                // Below entry is infeasible retained setup, NOT an operation failure.
                let mut low = entry;
                if trial(&program, engine, quota, low, false, profile) {
                    assert!(trial(&program, engine, quota, low, false, profile));
                    println!(
                        "quota engine={engine:?} op={} n={QUOTA_SIZE} {quota:?} entry={entry} feasibility-bound/pass={low}; no failing operation transition",
                        profile.name
                    );
                } else {
                    let mut high = (entry + 1).min(quota.cap());
                    while !trial(&program, engine, quota, high, false, profile) {
                        low = high;
                        assert!(high < quota.cap(), "no passing quota below cap");
                        high = high.saturating_mul(2).min(quota.cap());
                    }
                    while high - low > 1 {
                        let mid = low + (high - low) / 2;
                        if trial(&program, engine, quota, mid, false, profile) {
                            high = mid;
                        } else {
                            low = mid;
                        }
                    }
                    assert!(!trial(&program, engine, quota, low, false, profile));
                    assert!(trial(&program, engine, quota, high, false, profile));
                    println!(
                        "quota engine={engine:?} op={} n={QUOTA_SIZE} {quota:?} entry={entry} observed-fail={low} observed-pass={high}; NOT global minimum",
                        profile.name
                    );
                    low = high;
                }
                let phase_pass = low;
                let mut full_pass = phase_pass;
                while !trial(&program, engine, quota, full_pass, true, profile) {
                    assert!(
                        full_pass < quota.cap(),
                        "setup-inclusive passing quota unmet"
                    );
                    full_pass = full_pass.saturating_mul(2).min(quota.cap());
                }
                println!(
                    "quota-full engine={engine:?} op={} n={QUOTA_SIZE} {quota:?} phase-pass={phase_pass} creation-time-pass={full_pass}",
                    profile.name
                );
            }
        }
    }
}

#[cfg(feature = "register-vm")]
#[test]
fn stdlib_named_translation_coverage() {
    let program = program(&PROFILES[0], 0);
    let codes = register_check::both_codes(&program).unwrap();
    let mut found = 0;
    for (index, proto) in program.functions.iter().enumerate() {
        if let Some(name) = &proto.name {
            println!(
                "translation name={name} translated={}",
                codes[1].is_register(index)
            );
            found += 1;
        }
    }
    assert!(found > 0);
    for name in [
        "List.length",
        "List.map",
        "List.filter",
        "List.append",
        "List.take",
        "List.zip",
        "List.foldr",
        "Result.all",
        "Result.sequence",
        "Result.traverse",
        "Result.validate_all",
        "Result.validate_traverse",
        "Log.field_parts",
        "Log.kv",
    ] {
        assert!(
            program
                .functions
                .iter()
                .any(|p| p.name.as_deref() == Some(name)),
            "missing coverage row: {name}"
        );
    }
    let scalar = compile_source_hermetic("fun scalar (x : int) = x + 1\nval answer = scalar 41");
    let codes = register_check::both_codes(&scalar).unwrap();
    let index = scalar
        .functions
        .iter()
        .position(|p| p.name.as_deref() == Some("scalar"))
        .unwrap();
    assert!(codes[1].is_register(index));
    let mut vm = vm_for(&scalar, Engine::Translated);
    assert!(matches!(vm.run_slice(WORK), RunResult::Done));
    let slot = vm
        .global_names
        .iter()
        .position(|s| s.as_ref() == "answer")
        .unwrap();
    assert_eq!(int(vm.globals[slot].unwrap()), 42);
}
