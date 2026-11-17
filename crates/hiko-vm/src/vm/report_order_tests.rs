//! #255: untimed whole-fixture work, ordering parity and real resource limits.
//! The frozen stack fixture is deliberately separate from the quadratic oracle.
use super::*;
use crate::config::RunConfig;
use crate::test_support::compile_source_hermetic;

const WORK: u64 = 1_000_000_000;

#[cfg(not(feature = "register-vm"))]
#[test]
fn report_ordered_frozen_stack_work() {
    let mut rows = Vec::new();
    for n in [32usize, 64, 128] {
        // Exact report_ordered template from the rev2 frozen probe. Include
        // setup, construction and validation; do not include an old-sort oracle.
        let source = format!("import Sre.Report\nimport Sre.Health\nimport __Builtin.Int
            fun build (k, xs) = if k = 0 then xs else
              build (k - 1, (BuiltinInt.to_string k, Health.Healthy, \"\", \"\") :: xs)
            fun valid (xs, k) = case xs of [] => k = {n} + 1
              | (name,_,_,_) :: rest => if name = BuiltinInt.to_string k then valid (rest,k+1) else false
            val result = valid (Report.ordered (build ({n}, [])), 1)");
        println!("REPORT_ORDER_SOURCE n={n} source={source:?}");
        let mut vm = VM::new(compile_source_hermetic(&source));
        run_checked(&mut vm);
        let work = vm.fuel_budget().unwrap().used();
        let host = vm.heap.host_work_used();
        println!("REPORT_ORDER_WORK n={n} work={work} host={host}");
        rows.push((n, work, host));
    }
    for &(n, work, host) in &rows {
        assert!(work <= 10_000 + 2_500 * n as u64, "{rows:?}");
        assert_eq!(host, 0, "{rows:?}");
    }
}

fn vms(program: &CompiledProgram) -> Vec<(&'static str, VM)> {
    #[cfg(feature = "register-vm")]
    {
        ["stack", "register-enabled"]
            .into_iter()
            .zip(register_check::both_codes(program).unwrap())
            .map(|(engine, code)| {
                let mut vm = VM::from_parts(program.clone(), None, code);
                vm.register_builtins();
                (engine, vm)
            })
            .collect()
    }
    #[cfg(not(feature = "register-vm"))]
    {
        vec![("stack", VM::new(program.clone()))]
    }
}

fn run_checked(vm: &mut VM) {
    vm.set_fuel(WORK);
    vm.set_max_host_work(WORK);
    vm.run().unwrap();
    assert!(matches!(vm.get_global("result"), Some(Value::Bool(true))));
}

#[test]
fn report_ordered_matches_public_insert_reference() {
    let program = compile_source_hermetic(
        r#"
import Sre.Report
import Sre.Health
import Std.List
import __Builtin.Int
fun same_records ((left, right) : Health.check list * Health.check list) =
  case (left, right) of
      ([], []) => true
    | ((a, sa, ta, da) :: xs, (b, sb, tb, db) :: ys) =>
        a = b andalso Health.urgency sa = Health.urgency sb andalso
        ta = tb andalso da = db andalso same_records (xs, ys)
    | _ => false
fun same checks = same_records (Report.ordered checks,
  List.foldl (fn (sorted, check) => Report.insert (check, sorted)) [] checks)
fun visit (checks, depth) =
  if same checks then
    if depth = 0 then true else
      let
        val name = BuiltinInt.to_string depth
        val since = "since\n" ^ name
        val detail = "é\"λ" ^ name
        fun next status = visit ((name, status, since, detail) :: checks, depth - 1)
      in next Health.Critical andalso next Health.Unknown andalso
         next Health.Warning andalso next Health.Healthy end
  else false
val duplicate = ("duplicate", Health.Warning, "", "")
val result = visit ([], 4) andalso same [duplicate, duplicate, duplicate]
"#,
    );
    // 341 prefixes cover every status sequence of lengths zero through four.
    // Status normalization avoids treating the nominal status as an equality type.
    for (engine, mut vm) in vms(&program) {
        run_checked(&mut vm);
        println!("REPORT_ORDER_REFERENCE engine={engine} prefixes=341");
    }
}

#[test]
fn report_ordered_large_stable_groups() {
    for mixed in [false, true] {
        let status = if mixed {
            "case k mod 4 of 0 => Health.Critical | 1 => Health.Unknown | 2 => Health.Warning | _ => Health.Healthy"
        } else {
            "Health.Healthy"
        };
        let validation = if mixed {
            r#"
fun group (xs, k, urgency) =
  if k > size then (true, xs) else
    case xs of
        [] => (false, [])
      | (name, status, since, detail) :: rest =>
          if name = BuiltinInt.to_string k andalso Health.urgency status = urgency
             andalso since = "since" andalso detail = "detail"
          then group (rest, k + 4, urgency) else (false, [])
val (a, r1) = group (sorted, 4, 3)
val (b, r2) = group (r1, 1, 2)
val (c, r3) = group (r2, 2, 1)
val (d, r4) = group (r3, 3, 0)
val result = a andalso b andalso c andalso d andalso List.length r4 = 0
"#
        } else {
            r#"
fun valid (xs, k) = case xs of
    [] => k = size + 1
  | (name, status, since, detail) :: rest =>
      if name = BuiltinInt.to_string k andalso Health.urgency status = 0
         andalso since = "since" andalso detail = "detail"
      then valid (rest, k + 1) else false
val result = valid (sorted, 1)
"#
        };
        let program = compile_source_hermetic(&format!(
            r#"
import Sre.Report
import Sre.Health
import Std.List
import __Builtin.Int
val size = 2048
fun build (k, xs) = if k = 0 then xs else
  build (k - 1, (BuiltinInt.to_string k, ({status}), "since", "detail") :: xs)
val sorted = Report.ordered (build (size, []))
{validation}
"#
        ));
        for (engine, mut vm) in vms(&program) {
            run_checked(&mut vm);
            println!("REPORT_ORDER_LARGE engine={engine} mixed={mixed} size=2048");
        }
    }
}

#[test]
fn report_ordered_needs_no_runtime_capability() {
    let program = compile_source_hermetic(
        r#"
import Sre.Report
import Sre.Health
val checks = [("h1", Health.Healthy, "", ""), ("w1", Health.Warning, "", ""),
              ("u1", Health.Unknown, "", ""), ("c1", Health.Critical, "", ""),
              ("h2", Health.Healthy, "", ""), ("c2", Health.Critical, "", ""),
              ("u2", Health.Unknown, "", ""), ("w2", Health.Warning, "", "")]
fun names ((checks, expected) : Health.check list * string list) =
  case (checks, expected) of
      ([], []) => true
    | ((name, _, _, _) :: xs, first :: rest) => name = first andalso names (xs, rest)
    | _ => false
val generic_ok = case Report.ordered [(9, Health.Healthy, 7, false), (2, Health.Critical, 3, true)] of
    [(2, _, 3, true), (9, _, 7, false)] => true
  | _ => false
val result = generic_ok andalso names (Report.ordered checks,
  ["c1", "c2", "u1", "u2", "w1", "w2", "h1", "h2"])
"#,
    );
    let mut vm = RunConfig::from_toml("").unwrap().build_vm(program);
    run_checked(&mut vm);
}

#[test]
fn report_ordered_obeys_phase_object_byte_and_fuel_limits() {
    let program = compile_source_hermetic(
        r#"
import Sre.Report
import Sre.Health
import Std.List
fun build (n, xs) = if n = 0 then xs else
  build (n - 1, ("same", Health.Healthy, "", "") :: xs)
val input = build (64, [])
val thunk = fn () => List.length (Report.ordered input) = 64
"#,
    );
    for (case, object_room, byte_room, fuel) in [
        ("success", 4096, 1024 * 1024, WORK),
        ("objects", 0, 1024 * 1024, WORK),
        ("bytes", 4096, 0, WORK),
        ("fuel", 4096, 1024 * 1024, 1),
    ] {
        for (engine, mut vm) in vms(&program) {
            // Setup completes before installing phase caps; keep input and
            // thunk rooted in globals, just like the stdlib growth profiles.
            assert!(matches!(vm.run_slice(WORK), RunResult::Done));
            assert_eq!(vm.frames.len(), 1);
            assert_eq!(vm.frames[0].proto_idx, usize::MAX);
            assert!(vm.pending_runtime_request.is_none());
            assert!(vm.handlers.is_empty() && vm.hidden_handlers.is_empty());
            vm.frames.clear();
            vm.stack.clear();
            vm.gc_collect_with_extra_roots(std::iter::empty()).unwrap();
            let entry_objects = vm.heap.live_count();
            let entry_bytes = vm.heap.live_bytes();
            let Some(Value::Heap(reference)) = vm.get_global("thunk") else {
                panic!("missing thunk")
            };
            let HeapObject::Closure {
                proto_idx,
                captures,
            } = vm.heap.get(*reference).unwrap()
            else {
                panic!("thunk is not a closure")
            };
            let (proto_idx, captures) = (*proto_idx, Arc::clone(captures));
            vm.set_max_heap(entry_objects + object_room);
            vm.set_max_memory_bytes(entry_bytes + byte_room);
            vm.set_max_work(fuel);
            vm.setup_closure_call(proto_idx, &captures);
            let outcome = match vm.run_slice(WORK) {
                // The final shared-budget unit can end a slice by yielding;
                // the next entry must refuse without a refill or new grant.
                RunResult::Yielded if case == "fuel" => {
                    assert_eq!(vm.fuel_budget().unwrap().used(), 1);
                    vm.run_slice(WORK)
                }
                outcome => outcome,
            };
            match (case, &outcome) {
                ("success", RunResult::Done) => {
                    assert_eq!(vm.stack.len(), 1);
                    assert!(matches!(vm.stack[0], Value::Bool(true)));
                    assert!(vm.heap.live_count() <= entry_objects + object_room);
                    assert!(vm.heap.live_bytes() <= entry_bytes + byte_room);
                }
                ("objects", RunResult::Failed(ProcessFailure::HeapObjectLimitExceeded { .. })) => {}
                ("bytes", RunResult::Failed(ProcessFailure::RuntimeError(message)))
                    if message.starts_with("memory limit exceeded:") => {}
                ("fuel", RunResult::Failed(ProcessFailure::FuelExhausted)) => {
                    assert_eq!(vm.fuel_budget().unwrap().used(), 1);
                }
                _ => panic!("{engine}/{case}: {outcome:?}"),
            }
            println!("REPORT_ORDER_LIMIT engine={engine} case={case} outcome={outcome:?}");
        }
    }
}
