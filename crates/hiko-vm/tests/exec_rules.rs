//! `[[capabilities.exec.run.allow]]`: commands allowed only with given
//! arguments, and the ways a script might try to get around them.
#![cfg(all(unix, feature = "builtin-exec"))]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use hiko_vm::builder::VMBuilder;
use hiko_vm::config::RunConfig;
use hiko_vm::threaded::ThreadedRuntime;
use hiko_vm::vm::OutputSink;

fn compile(source: &str) -> hiko_compile::chunk::CompiledProgram {
    let tokens = hiko_syntax::lexer::Lexer::new(source, 0)
        .tokenize()
        .expect("lex");
    let program = hiko_syntax::parser::Parser::new(tokens)
        .parse_program()
        .expect("parse");
    hiko_compile::compiler::Compiler::compile(program)
        .expect("compile")
        .0
}

fn temp_dir(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "hiko-exec-rules-{name}-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// An executable at `dir/name` that prints its arguments, one per line.
fn tool(dir: &Path, name: &str) -> String {
    let path = dir.join(name);
    std::fs::write(
        &path,
        "#!/bin/sh\nfor a in \"$@\"; do echo \"[$a]\"; done\n",
    )
    .unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path.to_string_lossy().into_owned()
}

/// `["a", "b"]`, which TOML and Hiko both read as a list of strings.
fn quoted_list(items: &[&str]) -> String {
    let items: Vec<String> = items.iter().map(|s| format!("{s:?}")).collect();
    format!("[{}]", items.join(", "))
}

/// A config with one rule for `argv`, and `rest` if given.
fn config(argv: &[&str], rest: Option<&str>) -> RunConfig {
    let rest = rest.map(|r| format!("rest = {r:?}\n")).unwrap_or_default();
    RunConfig::from_toml(&format!(
        "[capabilities.stdio.print]\nenabled = true\n\
         [capabilities.stdio.println]\nenabled = true\n\
         [capabilities.stdio.eprint]\nenabled = true\n\
         [capabilities.stdio.eprintln]\nenabled = true\n\
         [capabilities.stdio.read_line]\nenabled = true\n\
         [capabilities.stdio.read_stdin]\nenabled = true\n\
         [capabilities.exec.run]\nenabled = true\n\
         [[capabilities.exec.run.allow]]\nargv = {}\n{rest}",
        quoted_list(argv)
    ))
    .expect("config")
}

/// Run `exec (command, args)` under `config`: the stdout, or the error.
fn run(config: &RunConfig, command: &str, args: &[&str]) -> Result<String, String> {
    let program = compile(&format!(
        "import __Builtin.Exec\nimport __Builtin.Stdio\nval (code, out, err) = BuiltinExec.exec ({command:?}, {})\nval _ = BuiltinStdio.print out",
        quoted_list(args)
    ));
    let mut vm = config.apply_to_builder(VMBuilder::new(program)).build();
    vm.enable_output_capture();
    vm.run().map_err(|e| e.message)?;
    Ok(vm.get_output().concat())
}

fn assert_refused(result: Result<String, String>) {
    let err = result.expect_err("must be refused");
    assert!(err.contains("matches no exec rule"), "{err}");
}

#[test]
fn a_refusal_does_not_show_the_arguments() {
    let dir = temp_dir("secret");
    let tool = tool(&dir, "tool");
    let config = config(&[&tool, "deploy"], None);

    let err = run(&config, &tool, &["deploy", "--token", "s3cr3t-value"]).expect_err("refused");
    assert!(!err.contains("s3cr3t-value"), "{err}");
    assert!(
        err.contains("with 3 argument(s) matches no exec rule"),
        "{err}"
    );
}

#[test]
fn a_changed_file_is_reported_only_when_a_rule_allows_the_arguments() {
    use std::io::Write as _;

    let dir = temp_dir("changed-args");
    let tool = tool(&dir, "tool");
    let config = config(&[&tool, "ok"], None);
    let program = compile(&format!(
        "import __Builtin.Exec\nimport __Builtin.Stdio\nval (code, out, err) = BuiltinExec.exec ({tool:?}, {})\nval _ = BuiltinStdio.print out",
        quoted_list(&["no"])
    ));
    let mut vm = config.apply_to_builder(VMBuilder::new(program)).build();
    // Written in place after the policy was applied: same inode, new size.
    std::fs::OpenOptions::new()
        .append(true)
        .open(&tool)
        .unwrap()
        .write_all(b"# more\n")
        .unwrap();
    let err = vm.run().expect_err("refused").message;
    // `no` matches no rule, so that is the reason, not the change.
    assert!(err.contains("matches no exec rule"), "{err}");
    assert!(!err.contains("changed or was replaced"), "{err}");
}

#[test]
fn the_program_sees_the_command_it_was_run_as() {
    // On Linux the pinned file runs through /proc/self/fd; argv[0] is still
    // the command the script ran.
    let sh = std::fs::canonicalize("/bin/sh").unwrap();
    let sh = sh.to_string_lossy().into_owned();
    let config = config(&[&sh, "-c", "echo \"$0\""], None);
    let out = run(&config, &sh, &["-c", "echo \"$0\""]).unwrap();
    assert_eq!(out.trim_end(), sh, "{out}");
}

#[test]
fn exact_rule_allows_only_its_arguments() {
    let dir = temp_dir("exact");
    let tool = tool(&dir, "tool");
    let config = config(&[&tool, "run", "check"], None);

    assert_eq!(
        run(&config, &tool, &["run", "check"]).unwrap(),
        "[run]\n[check]\n"
    );
    assert_refused(run(&config, &tool, &[]));
    assert_refused(run(&config, &tool, &["run"]));
    assert_refused(run(&config, &tool, &["run", "check", "extra"]));
    assert_refused(run(&config, &tool, &["run", "check", ""]));
    assert_refused(run(&config, &tool, &["", "run", "check"]));
    assert_refused(run(&config, &tool, &["run check"]));
    assert_refused(run(&config, &tool, &["run", "check "]));
    assert_refused(run(&config, &tool, &["check", "run"]));
    assert_refused(run(&config, &tool, &["run", "check;", "rm"]));
}

#[test]
fn prefix_rule_allows_any_rest_after_the_prefix() {
    let dir = temp_dir("prefix");
    let tool = tool(&dir, "tool");
    let config = config(&[&tool, "logs", "--"], Some("any"));

    assert_eq!(
        run(&config, &tool, &["logs", "--"]).unwrap(),
        "[logs]\n[--]\n"
    );
    assert_eq!(
        run(&config, &tool, &["logs", "--", "--since", "1h"]).unwrap(),
        "[logs]\n[--]\n[--since]\n[1h]\n"
    );
    assert_refused(run(&config, &tool, &["logs"]));
    assert_refused(run(&config, &tool, &["logs", "--x"]));
    assert_refused(run(&config, &tool, &["--", "logs"]));
    assert_refused(run(&config, &tool, &["logs-all", "--"]));
}

#[test]
fn the_command_matches_by_file_not_by_name() {
    let dir = temp_dir("file");
    let allowed = tool(&dir, "tool");
    let other_dir = dir.join("other");
    std::fs::create_dir_all(&other_dir).unwrap();
    let impostor = tool(&other_dir, "tool");
    let config = config(&[&allowed, "run"], None);

    // Another file with the same name and the same arguments.
    assert_refused(run(&config, &impostor, &["run"]));
    // A symlink to another file.
    let link = dir.join("link");
    std::os::unix::fs::symlink(&impostor, &link).unwrap();
    assert_refused(run(&config, &link.to_string_lossy(), &["run"]));
    // Spellings of the allowed file resolve to it.
    let dotted = format!("{}/./tool", dir.display());
    assert_eq!(run(&config, &dotted, &["run"]).unwrap(), "[run]\n");
}

#[test]
fn nul_in_an_argument_is_refused() {
    let dir = temp_dir("nul");
    let tool = tool(&dir, "tool");
    let config = config(&[&tool], Some("any"));
    let err = run(&config, &tool, &["a\u{0}b"]).unwrap_err();
    assert!(err.contains("NUL"), "{err}");
}

#[test]
fn allowed_commands_still_allow_any_arguments() {
    let dir = temp_dir("commands");
    let tool = tool(&dir, "tool");
    let config = RunConfig::from_toml(&format!(
        "[capabilities.stdio.print]\nenabled = true\n\
         [capabilities.exec.run]\nenabled = true\nallowed_commands = [{tool:?}]\n"
    ))
    .unwrap();
    assert_eq!(run(&config, &tool, &["x", "y"]).unwrap(), "[x]\n[y]\n");
}

#[test]
fn bad_rules_are_config_errors() {
    for (rule, expected) in [
        ("argv = []\n", "argv must not be empty"),
        ("argv = [\"\"]\n", "argv must not be empty"),
        ("argv = [\"ls\", \"a\\u0000\"]\n", "NUL"),
        ("argv = [\"ls\"]\nrest = \"some\"\n", "unknown variant"),
        ("argv = [\"ls\"]\nprefix = true\n", "unknown field"),
        ("rest = \"any\"\n", "missing field `argv`"),
    ] {
        let err = RunConfig::from_toml(&format!(
            "[capabilities.exec.run]\nenabled = true\n[[capabilities.exec.run.allow]]\n{rule}"
        ))
        .unwrap_err();
        assert!(err.contains(expected), "{rule}: {err}");
    }
}

#[test]
fn build_vm_source_carries_the_rules() {
    let config = config(&["mise", "run", "//cell:check"], Some("any"));
    let source = config.to_rust_source().expect("generated source");
    assert!(
        source.contains(
            r#"hiko_vm::builder::ExecArgvRule { argv: vec!["mise".into(), "run".into(), "//cell:check".into()], rest_any: true }"#
        ),
        "{source}"
    );
}

#[derive(Default)]
struct Captured(Mutex<String>);

impl OutputSink for Captured {
    fn write(&self, text: &str) -> std::io::Result<()> {
        self.0.lock().unwrap().push_str(text);
        Ok(())
    }
}

#[test]
fn a_spawned_child_has_the_same_rules() {
    let dir = temp_dir("child");
    let tool = tool(&dir, "tool");
    let config = RunConfig::from_toml(&format!(
        "[capabilities.exec.run]\nenabled = true\n\
         [[capabilities.exec.run.allow]]\nargv = [{tool:?}, \"ok\"]\n\
         [capabilities.fiber.spawn]\nenabled = true\n\
         [capabilities.fiber.join_or_fail]\nenabled = true\n[capabilities.fiber.join]\nenabled = true\n\
         [capabilities.stdio.println]\nenabled = true\n"
    ))
    .unwrap();
    let program = compile(&format!(
        "import __Builtin.Exec\nimport __Builtin.Fiber\nimport __Builtin.Stdio\n\
         val allowed = BuiltinFiber.spawn (fn () => let val (_, out, _) = BuiltinExec.exec ({tool:?}, [\"ok\"]) in out end)\n\
         val _ = BuiltinStdio.println (BuiltinFiber.await allowed)\n\
         val refused = BuiltinFiber.spawn (fn () => let val (_, out, _) = BuiltinExec.exec ({tool:?}, [\"no\"]) in out end)\n\
         val _ = BuiltinStdio.println (BuiltinFiber.await refused)"
    ));
    let mut vm = config.apply_to_builder(VMBuilder::new(program)).build();
    let sink = Arc::new(Captured::default());
    vm.set_output_sink(sink.clone());
    let runtime = ThreadedRuntime::new(2);
    let pid = runtime.spawn_root_vm(vm);
    let _ = runtime.run_to_completion();
    // The first child ran (`println` adds a newline after the tool's). The
    // second was refused, which fails the root at its await.
    assert_eq!(*sink.0.lock().unwrap(), "[ok]\n\n");
    let failure = format!("{:?}", runtime.failure(pid));
    assert!(
        failure.contains("with 1 argument(s) matches no exec rule"),
        "{failure}"
    );
}
