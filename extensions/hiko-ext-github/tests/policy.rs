//! The repository allowlist and the module, without `gh` or the network:
//! every call here is refused before `gh` would run.

use hiko_compile::chunk::CompiledProgram;
use hiko_compile::compiler::{Compiler, ModuleLoaderOptions};
use hiko_ext_github::GITHUB;
use hiko_vm::config::RunConfig;
use hiko_vm::extension::Extension;

const LINKED: &[&Extension] = &[&GITHUB];

fn compile(source: &str, config: &RunConfig) -> CompiledProgram {
    let tokens = hiko_syntax::lexer::Lexer::new(source, 0)
        .tokenize()
        .expect("lex");
    let program = hiko_syntax::parser::Parser::new(tokens)
        .parse_program()
        .expect("parse");
    let configured = config.configured_extensions(LINKED).expect("linked");
    let options = ModuleLoaderOptions {
        extensions: Extension::compile_surface(&configured),
        ..Default::default()
    };
    Compiler::compile_with_options(program, options)
        .expect("compile")
        .0
}

fn run_error(source: &str, policy: &str) -> String {
    let config = RunConfig::from_toml(policy).expect("config");
    let program = compile(source, &config);
    let mut vm = config
        .build_vm_with_extensions(program, LINKED)
        .expect("build");
    vm.run().expect_err("the call must be refused").message
}

#[test]
fn a_repo_outside_the_allowlist_is_refused() {
    let err = run_error(
        r#"val _ = github_issue_create ("evil-org/nope", "title", "body")"#,
        "[extensions.github]\ncreate = [\"vectorian-rs/hiko\"]\n",
    );
    assert!(
        err.contains("repo 'evil-org/nope' is not in the allowed repos"),
        "{err}"
    );
}

#[test]
fn an_operation_without_a_list_may_use_no_repo() {
    let err = run_error(
        r#"import Github.Issue
val _ = Issue.update "vectorian-rs/hiko" 1 (Issue.update_spec [Issue.Close])"#,
        "[extensions.github]\nview = [\"vectorian-rs/hiko\"]\n",
    );
    assert!(
        err.contains("github_issue_update: repo 'vectorian-rs/hiko'"),
        "{err}"
    );
}

#[test]
fn unknown_policy_keys_are_rejected() {
    let config = RunConfig::from_toml("[extensions.github]\ndelete = [\"a/b\"]\n").expect("config");
    let program = compile("val x = 1", &config);
    let err = config
        .build_vm_with_extensions(program, LINKED)
        .err()
        .expect("unknown key must fail");
    assert!(
        err.contains("extensions.github: unknown field `delete`"),
        "{err}"
    );
}
