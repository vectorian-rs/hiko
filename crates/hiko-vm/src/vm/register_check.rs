//! Equivalence checking for the register VM: random integer programs run
//! on the stack VM alone and with register code, and every observable
//! result must be the same. The proptest below and the `fuzz_register_vm`
//! target share the generator and the checks.

use std::fmt::Write as _;
use std::sync::Arc;

use hiko_compile::chunk::CompiledProgram;
use hiko_compile::compiler::Compiler;
use hiko_syntax::lexer::Lexer;
use hiko_syntax::parser::Parser;

use super::{RegisterCode, RunResult, VM};
use crate::verify::verify_program;

/// Fuel for one run; it bounds generated programs that never finish.
const FUEL: u64 = 20_000;

/// Reads generator choices from bytes, then zeros once they run out, so
/// every input gives one finite program.
struct Choices<'a> {
    bytes: &'a [u8],
}

impl Choices<'_> {
    fn byte(&mut self) -> u8 {
        match self.bytes.split_first() {
            Some((&b, rest)) => {
                self.bytes = rest;
                b
            }
            None => 0,
        }
    }

    fn below(&mut self, n: u8) -> u8 {
        self.byte() % n
    }
}

struct Generator<'a> {
    choices: Choices<'a>,
    /// Arity of each function `f0`, `f1`, ...
    arities: Vec<u8>,
    /// Names in scope: parameters, then `let` bindings.
    scope: Vec<String>,
    lets: usize,
    /// Whether calls may be generated: not in a function's base case.
    calls: bool,
    out: String,
}

// Repeated entries weight the choice. Most picks are what the register VM
// translates; the rest keep the fallback to the stack VM, and the
// translation of more operations, under test.
const LITERALS: [&str; 10] = [
    "0",
    "1",
    "2",
    "3",
    "~1",
    "~2",
    "7",
    "100",
    "9223372036854775807",
    "~9223372036854775807",
];
const OPS: [&str; 8] = ["+", "+", "-", "-", "*", "*", "/", "mod"];
const CMPS: [&str; 10] = ["<", "<", "<=", "<=", ">", ">", ">=", ">=", "=", "<>"];

impl Generator<'_> {
    fn pick<'s>(&mut self, items: &[&'s str]) -> &'s str {
        items[self.choices.below(items.len() as u8) as usize]
    }

    fn expr(&mut self, depth: u8) {
        let choice = if depth == 0 {
            self.choices.below(2)
        } else {
            self.choices.below(8)
        };
        match choice {
            0 => {
                let lit = self.pick(&LITERALS);
                self.out.push_str(lit);
            }
            1 if !self.scope.is_empty() => {
                let i = self.choices.below(self.scope.len() as u8) as usize;
                let name = self.scope[i].clone();
                self.out.push_str(&name);
            }
            1 => self.out.push('3'),
            2 | 3 => {
                let op = self.pick(&OPS);
                self.out.push('(');
                self.expr(depth - 1);
                write!(self.out, " {op} ").unwrap();
                self.expr(depth - 1);
                self.out.push(')');
            }
            4 => {
                self.out.push_str("(if ");
                self.cond(depth - 1);
                self.out.push_str(" then ");
                self.expr(depth - 1);
                self.out.push_str(" else ");
                self.expr(depth - 1);
                self.out.push(')');
            }
            5 => {
                let name = format!("x{}", self.lets);
                self.lets += 1;
                write!(self.out, "(let val {name} = ").unwrap();
                self.expr(depth - 1);
                self.out.push_str(" in ");
                self.scope.push(name);
                self.expr(depth - 1);
                self.scope.pop();
                self.out.push_str(" end)");
            }
            _ if self.calls => self.call(depth - 1, "(p0 - 1)"),
            _ => self.expr(depth - 1),
        }
    }

    fn cond(&mut self, depth: u8) {
        let choice = if depth == 0 { 0 } else { self.choices.below(8) };
        match choice {
            0..=4 => {
                let cmp = self.pick(&CMPS);
                self.out.push('(');
                self.expr(depth);
                write!(self.out, " {cmp} ").unwrap();
                self.expr(depth);
                self.out.push(')');
            }
            5 => {
                self.out.push_str("(not ");
                self.cond(depth - 1);
                self.out.push(')');
            }
            n => {
                let op = if n == 6 { "andalso" } else { "orelse" };
                self.out.push('(');
                self.cond(depth - 1);
                write!(self.out, " {op} ").unwrap();
                self.cond(depth - 1);
                self.out.push(')');
            }
        }
    }

    /// A call whose first argument, the countdown, is `counter`.
    fn call(&mut self, depth: u8, counter: &str) {
        let f = self.choices.below(self.arities.len() as u8) as usize;
        write!(self.out, "(f{f} ({counter}").unwrap();
        for _ in 1..self.arities[f] {
            self.out.push_str(", ");
            self.expr(depth);
        }
        self.out.push_str("))");
    }
}

/// A program of up to four mutually recursive integer functions, and a
/// main that prints the results of a few calls. Each function's first
/// parameter counts down: below 1 the function returns without calling,
/// so every call tree is finite, if possibly large.
fn program_from_bytes(bytes: &[u8]) -> String {
    let mut choices = Choices { bytes };
    let n_functions = 1 + choices.below(4);
    let arities = (0..n_functions).map(|_| 1 + choices.below(3)).collect();
    let mut g = Generator {
        choices,
        arities,
        scope: Vec::new(),
        lets: 0,
        calls: false,
        out: String::from("import __Builtin.Stdio\nimport __Builtin.Int\n"),
    };
    for f in 0..g.arities.len() {
        let params: Vec<String> = (0..g.arities[f]).map(|i| format!("p{i}")).collect();
        let typed = params
            .iter()
            .map(|p| format!("{p} : int"))
            .collect::<Vec<_>>()
            .join(", ");
        let keyword = if f == 0 { "fun" } else { "and" };
        // The result is annotated too: a function whose result comes only
        // from recursive calls would leave its operators unresolved.
        write!(g.out, "{keyword} f{f} ({typed}) = (if p0 < 1 then ").unwrap();
        g.scope = params;
        g.calls = false;
        g.expr(2);
        g.out.push_str(" else ");
        g.calls = true;
        g.expr(3);
        g.out.push_str(" : int)\n");
    }
    g.scope.clear();
    g.calls = false;
    for _ in 0..1 + g.choices.below(3) {
        g.out
            .push_str("val _ = BuiltinStdio.println (BuiltinInt.to_string ");
        let counter = g.choices.below(7).to_string();
        g.call(1, &counter);
        g.out.push_str(")\n");
    }
    g.out
}

/// Compile `src` (desugaring, type inference, code generation). `None`
/// when it does not compile.
pub(super) fn compile(src: &str) -> Option<CompiledProgram> {
    let tokens = Lexer::new(src, 0).tokenize().ok()?;
    let program = Parser::new(tokens).parse_program().ok()?;
    Some(Compiler::compile(program).ok()?.0)
}

/// A VM for a verified `program` with all builtins and captured output,
/// running `code`: the program's register code, or none for the stack VM
/// alone.
pub(super) fn vm(program: &CompiledProgram, code: &Arc<RegisterCode>) -> VM {
    let mut vm = VM::from_parts(program.clone(), None, Arc::clone(code));
    vm.register_builtins();
    vm.enable_output_capture();
    vm
}

/// No register code, and the register code for `program`, in the order
/// the checks compare them: stack VM first. `Err` when `program` does not
/// verify; only verified code is translated.
pub(super) fn both_codes(program: &CompiledProgram) -> Result<[Arc<RegisterCode>; 2], String> {
    verify_program(program).map_err(|e| format!("does not verify: {e}"))?;
    Ok([Arc::default(), Arc::new(RegisterCode::translate(program))])
}

/// `Err` naming `what` when the stack VM's result differs from the
/// register VM's.
fn same<T: PartialEq + std::fmt::Debug>(
    what: impl std::fmt::Display,
    [stack, register]: [T; 2],
) -> Result<(), String> {
    if stack == register {
        Ok(())
    } else {
        Err(format!(
            "{what}:\n stack:    {stack:?}\n register: {register:?}"
        ))
    }
}

/// Everything a run shows from outside: the output, the error, where the
/// error points, and how deep the call stack was when it stopped.
///
/// The span is compared only for errors other than fuel exhaustion. When
/// fuel runs out, both VMs stop in the same frame after the same work, but
/// not always at the same instruction: one register instruction stands for
/// several stack instructions, and the stack VM can stop between them, for
/// example after pushing one argument of a call. The register VM then
/// points at the call, the stack VM at the push.
#[derive(Debug, PartialEq)]
struct Outcome {
    output: Vec<String>,
    error: Option<String>,
    span: Option<hiko_syntax::span::Span>,
    frames: usize,
}

fn run(program: &CompiledProgram, code: &Arc<RegisterCode>, fuel: u64) -> Outcome {
    let mut vm = vm(program, code);
    vm.set_fuel(fuel);
    let error = vm.run().err();
    let span = match &error {
        Some(e) if !e.is_fuel_exhausted() => vm.error_span(),
        _ => None,
    };
    Outcome {
        output: vm.get_output().to_vec(),
        span,
        error: error.map(|e| e.message),
        frames: vm.frames.len(),
    }
}

/// Run in slices of `size` fuel until the program stops or `FUEL` is
/// spent; the slice count and the end state.
fn run_slices(
    program: &CompiledProgram,
    code: &Arc<RegisterCode>,
    size: u64,
) -> (u64, String, Vec<String>) {
    let mut vm = vm(program, code);
    let mut slices = 0;
    let end = loop {
        slices += 1;
        match vm.run_slice(size) {
            RunResult::Yielded if slices * size < FUEL => {}
            other => break format!("{other:?}"),
        }
    };
    (slices, end, vm.get_output().to_vec())
}

/// Check that `program` behaves the same on both VMs: with enough fuel to
/// finish or a fuel limit, at a small fuel limit taken from `seed` (where
/// it stops, and in which frame), and run in small scheduler slices.
fn check(program: &CompiledProgram, seed: u8) -> Result<(), String> {
    let codes = both_codes(program)?;
    for fuel in [FUEL, u64::from(seed) * 7] {
        same(
            format_args!("fuel {fuel}"),
            codes.each_ref().map(|c| run(program, c, fuel)),
        )?;
    }
    let size = 1 + u64::from(seed % 16);
    same(
        format_args!("slices of {size}"),
        codes.each_ref().map(|c| run_slices(program, c, size)),
    )
}

/// Generate a program from `bytes` and check it on both VMs. The error
/// shows the difference and the program.
pub fn check_bytes(bytes: &[u8]) -> Result<(), String> {
    // The seed for fuel limits and slice sizes is a byte the generator does
    // not read, so it varies independently of the program.
    let (seed, bytes) = bytes.split_last().unwrap_or((&0, &[]));
    let src = program_from_bytes(bytes);
    let program = compile(&src).ok_or_else(|| format!("does not compile:\n{src}"))?;
    check(&program, *seed).map_err(|diff| format!("{diff}\nprogram:\n{src}"))
}

// Generated programs print with `BuiltinStdio.println` and `BuiltinInt.to_string`.
#[cfg(all(test, feature = "builtin-stdio", feature = "builtin-convert"))]
mod tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(300))]

        #[test]
        fn both_vms_agree(bytes in proptest::collection::vec(any::<u8>(), 0..256)) {
            if let Err(e) = check_bytes(&bytes) {
                panic!("{e}");
            }
        }
    }

    #[test]
    fn generated_functions_translate() {
        // The check is only worth something if generated functions reach
        // the register VM. Stage 1 does not translate `/`, `mod`, `=`, `<>`
        // or `not`, so some fall back to the stack VM; 47% translate. Raise
        // the bound as the translator grows.
        let (mut total, mut translated) = (0, 0);
        for seed in 0u32..500 {
            let bytes: Vec<u8> = (0..256u32)
                .map(|i| {
                    (seed.wrapping_mul(2_654_435_761) ^ i.wrapping_mul(40_503)).to_le_bytes()[1]
                })
                .collect();
            let program = compile(&program_from_bytes(&bytes)).expect("compiles");
            let code = RegisterCode::translate(&program);
            total += program.functions.len();
            translated += (0..program.functions.len())
                .filter(|&i| code.get(i).is_some())
                .count();
        }
        eprintln!("{translated} of {total} generated functions translate");
        assert!(translated * 5 > total * 2, "{translated} of {total}");
    }
}
