//! Real named profiles load Std.Fiber through the policy-filtered VM.
mod common;

use std::path::Path;

use hiko_compile::chunk::CompiledProgram;
use hiko_compile::compiler::Compiler;
use hiko_vm::config::RunConfig;

const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
const PROFILES: &[&str] = &["benchmark", "software-developer-role"];

fn policy(profile: &str) -> String {
    std::fs::read_to_string(format!("{ROOT}/policies/{profile}.policy.toml")).unwrap()
}

fn compile_fiber_fixture() -> Result<CompiledProgram, String> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fiber_policy.hml");
    let source = std::fs::read_to_string(&path).unwrap();
    let tokens = hiko_syntax::lexer::Lexer::new(&source, 0)
        .tokenize()
        .map_err(|e| format!("{e:?}"))?;
    let program = hiko_syntax::parser::Parser::new(tokens)
        .parse_program()
        .map_err(|e| format!("{e:?}"))?;
    Compiler::compile_file_with_options(program, &path, common::hermetic_loader_options())
        .map(|(program, _)| program)
        .map_err(|e| format!("{e:?}"))
}

#[cfg(all(
    feature = "builtin-process",
    feature = "builtin-convert",
    feature = "builtin-testing"
))]
mod enabled {
    use super::*;
    use hiko_vm::threaded::ThreadedRuntime;

    fn run(profile: &str, program: CompiledProgram) -> Result<(), String> {
        let config = RunConfig::from_toml(&policy(profile))?;
        let runtime = ThreadedRuntime::new(1);
        let pid = runtime.spawn_root_vm(config.build_vm(program));
        runtime.run_to_completion()?;
        match runtime.failure(pid) {
            Some(error) => Err(error.to_string()),
            None => Ok(()),
        }
    }

    #[test]
    fn real_profiles_allow_fiber() {
        let program = compile_fiber_fixture().unwrap();
        for profile in PROFILES {
            run(profile, program.clone()).unwrap_or_else(|e| panic!("{profile}: {e}"));
        }
    }

    #[test]
    fn a_profile_without_process_permissions_still_denies_fiber() {
        let message = run("docs-writer", compile_fiber_fixture().unwrap()).unwrap_err();
        assert!(message.contains("undefined global: spawn"), "{message}");
    }
}

#[cfg(not(feature = "builtin-process"))]
#[test]
fn policy_grants_cannot_enable_an_omitted_cargo_feature() {
    for profile in PROFILES {
        let config = RunConfig::from_toml(&policy(profile)).unwrap();
        assert!(config.enabled_builtin_names().contains("spawn"));
    }
    let message = compile_fiber_fixture().unwrap_err();
    assert!(message.contains("not available in this build"), "{message}");
    assert!(message.contains("builtin-process"), "{message}");
}
