//! A project's lockfile stays in its directory, for every command that reads
//! the manifest, and the script's own project is the one checked.
#![cfg(feature = "cli-lock")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SCRIPT: &str = "val x = 1\n";

/// An empty directory for test `name`, by its real path.
fn empty_dir(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("project_boundary")
        .join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::canonicalize(dir).unwrap()
}

/// `<dir>/project`, holding a manifest with `defaults`, a policy and `a.hml`.
fn project_in(dir: &Path, defaults: &str) -> PathBuf {
    let project = dir.join("project");
    fs::create_dir_all(&project).unwrap();
    fs::write(
        project.join("hiko.toml"),
        format!("[defaults]\npolicy = \"p\"\n{defaults}\n[policies.p]\npath = \"p.toml\"\n"),
    )
    .unwrap();
    fs::write(project.join("p.toml"), "").unwrap();
    fs::write(project.join("a.hml"), SCRIPT).unwrap();
    project
}

fn hiko(cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_hiko-cli"))
        .current_dir(cwd)
        .env("NO_COLOR", "1")
        .args(args)
        .output()
        .expect("hiko-cli should start")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn outside_message(project: &Path, lockfile: &str) -> String {
    format!(
        "project manifest '{}' sets lockfile to '{lockfile}', which is outside its directory; use a path inside it",
        project.join("hiko.toml").display()
    )
}

/// Every command that reads the manifest, for a script with no import.
fn assert_every_command_rejects(project: &Path, lockfile: &str) {
    let commands: [&[&str]; 6] = [
        &["run", "a.hml"],
        &["check", "a.hml"],
        &["inspect-work", "a.hml"],
        &["install"],
        &["install", "--check"],
        &["lock", "verify"],
    ];
    for args in commands {
        let output = hiko(project, args);
        assert!(!output.status.success(), "{args:?} passed");
        let message = stderr(&output);
        assert!(
            message.contains(&outside_message(project, lockfile)),
            "{args:?}: {message}"
        );
    }
}

#[test]
fn a_lockfile_outside_the_project_is_rejected_by_every_command() {
    let dir = empty_dir("rejected");
    let absolute = dir.join("elsewhere.toml");
    fs::write(&absolute, "keep this line\n").unwrap();
    for lockfile in ["../x", "new/../../planted", absolute.to_str().unwrap()] {
        let project = project_in(&dir, &format!("lockfile = \"{lockfile}\""));
        assert_every_command_rejects(&project, lockfile);
    }
    // `hiko install` wrote nothing, here or at the paths it was given.
    assert_eq!(fs::read_to_string(&absolute).unwrap(), "keep this line\n");
    assert!(!dir.join("x").exists());
    assert!(!dir.join("planted").exists());
    assert!(!dir.join("project/new").exists());
}

#[cfg(unix)]
#[test]
fn a_final_symlink_out_of_the_project_is_rejected_by_every_command() {
    use std::os::unix::fs::symlink;

    let dir = empty_dir("symlink");
    fs::write(dir.join("outside.toml"), "keep this line\n").unwrap();

    let project = project_in(&dir, "lockfile = \"link.toml\"");
    symlink("../outside.toml", project.join("link.toml")).unwrap();
    assert_every_command_rejects(&project, "link.toml");

    // The default name is bounded too.
    let dir = empty_dir("symlink-default");
    fs::write(dir.join("outside.toml"), "keep this line\n").unwrap();
    let project = project_in(&dir, "");
    symlink("../outside.toml", project.join("hiko.lock.toml")).unwrap();
    assert_every_command_rejects(&project, "hiko.lock.toml");
    assert_eq!(
        fs::read_to_string(dir.join("outside.toml")).unwrap(),
        "keep this line\n"
    );
}

#[test]
fn build_vm_lock_prints_the_rejection_as_text() {
    let dir = empty_dir("build-vm");
    let project = project_in(&dir, "lockfile = \"../x\"");
    fs::write(project.join("hiko.lock.toml"), "schema_version = 1\n").unwrap();
    let output = hiko(
        &project,
        &["build-vm", "p.toml", "--lock", "hiko.lock.toml"],
    );
    assert!(!output.status.success());
    let message = stderr(&output);
    assert!(
        message.contains(&outside_message(&project, "../x")),
        "got: {message}"
    );
    assert!(!message.contains("Codegen("), "got: {message}");
}

#[test]
fn the_scripts_own_project_is_the_one_checked() {
    let dir = empty_dir("own-project");
    let policy = project_in(&dir.join("policy"), "lockfile = \"../x\"");
    let config = policy.join("hiko.toml");
    let config = config.to_str().unwrap();

    // The policy's manifest is not the script's project, so its lockfile is
    // not used and not checked.
    let elsewhere = dir.join("no-project");
    fs::create_dir(&elsewhere).unwrap();
    fs::write(elsewhere.join("a.hml"), SCRIPT).unwrap();
    assert!(
        hiko(&elsewhere, &["check", "--config", config, "a.hml"])
            .status
            .success()
    );

    // The script's project is checked, whichever manifest gives the policy.
    let scripts = project_in(&dir.join("scripts"), "lockfile = \"../y\"");
    let output = hiko(&scripts, &["check", "--config", config, "a.hml"]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains(&outside_message(&scripts, "../y")));
}

#[test]
fn an_invalid_manifest_beside_the_script_fails_with_no_import() {
    let dir = empty_dir("invalid-manifest");
    let policy = project_in(&dir.join("policy"), "");
    let config = policy.join("hiko.toml");
    let scripts = project_in(&dir.join("scripts"), "");
    fs::write(scripts.join("hiko.toml"), "[defaults\n").unwrap();
    let output = hiko(
        &scripts,
        &["check", "--config", config.to_str().unwrap(), "a.hml"],
    );
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("Invalid project manifest"),
        "got: {}",
        stderr(&output)
    );
}

#[test]
fn a_missing_lockfile_inside_the_project_passes_the_bound() {
    let dir = empty_dir("missing");
    let project = project_in(&dir, "lockfile = \"new/project.lock.toml\"");
    assert!(hiko(&project, &["check", "a.hml"]).status.success());

    // A program that needs it keeps the existing error.
    fs::write(project.join("b.hml"), "import Std.List\n").unwrap();
    let output = hiko(&project, &["check", "b.hml"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("references missing module lockfile"),
        "got: {}",
        stderr(&output)
    );
}

#[test]
fn an_absolute_lockfile_inside_the_project_passes_the_bound() {
    let dir = empty_dir("absolute-inside");
    let lockfile = dir.join("project/project.lock.toml");
    let defaults = format!("lockfile = \"{}\"", lockfile.display());
    let project = project_in(&dir, &defaults);
    assert!(hiko(&project, &["check", "a.hml"]).status.success());
}

#[test]
fn config_naming_a_manifest_in_the_working_directory_passes_the_bound() {
    let dir = empty_dir("bare-config");
    let project = project_in(&dir, "");
    fs::write(project.join("hiko.lock.toml"), "schema_version = 1\n").unwrap();
    for args in [
        &["lock", "verify", "--config", "hiko.toml"][..],
        &["install", "--check", "--config", "hiko.toml"],
    ] {
        let output = hiko(&project, args);
        assert!(output.status.success(), "{args:?}: {}", stderr(&output));
    }
}

#[cfg(unix)]
#[test]
fn a_project_in_a_symlinked_directory_passes_the_bound() {
    let dir = empty_dir("symlinked-dir");
    let project = project_in(&dir, "");
    fs::write(project.join("hiko.lock.toml"), "schema_version = 1\n").unwrap();
    std::os::unix::fs::symlink(&project, dir.join("link")).unwrap();
    let output = hiko(&dir, &["lock", "verify", "--config", "link/hiko.toml"]);
    assert!(output.status.success(), "{}", stderr(&output));
}

const NOT_TOML: &str = "schema_version = 1\nexport API_TOKEN=FAKEVALUE3\n";

/// `output` names `path` and the position, and prints no source line.
fn assert_position_and_no_source_line(output: &Output, path: &Path, label: &str) {
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        stderr(output)
    );
    let position = format!("'{}': line 2, column 8: expected", path.display());
    assert!(text.contains(&position), "{label}: {text}");
    assert!(!text.contains("API_TOKEN"), "{label}: {text}");
    assert!(!text.contains("FAKEVALUE3"), "{label}: {text}");
}

#[test]
fn a_lockfile_that_does_not_parse_prints_no_source_line() {
    let dir = empty_dir("lockfile-parse");
    let project = project_in(&dir, "");
    let lockfile = project.join("hiko.lock.toml");
    fs::write(project.join("b.hml"), "import Std.List\n").unwrap();
    let commands: [&[&str]; 5] = [
        &["check", "b.hml"],
        &["lock", "verify"],
        &["build-vm", "p.toml", "--lock", "hiko.lock.toml"],
        &["install", "--check"],
        &["install"],
    ];
    for args in commands {
        fs::write(&lockfile, NOT_TOML).unwrap();
        let output = hiko(&project, args);
        assert_position_and_no_source_line(&output, &lockfile, &format!("{args:?}"));
    }
}

#[test]
fn a_manifest_that_does_not_parse_prints_no_source_line() {
    let dir = empty_dir("manifest-parse");
    let project = project_in(&dir, "");
    fs::write(project.join("hiko.lock.toml"), "schema_version = 1\n").unwrap();
    let manifest = project.join("hiko.toml");
    fs::write(&manifest, NOT_TOML).unwrap();
    let commands: [&[&str]; 4] = [
        &["check", "a.hml"],
        &["lock", "verify"],
        &["install"],
        // The compiler's own reader: nothing in `main.rs` reads this manifest.
        &["build-vm", "p.toml", "--lock", "hiko.lock.toml"],
    ];
    for args in commands {
        let output = hiko(&project, args);
        assert!(!output.status.success(), "{args:?} passed");
        assert_position_and_no_source_line(&output, &manifest, &format!("{args:?}"));
    }
}

#[test]
fn build_vm_checks_the_project_of_its_script() {
    let dir = empty_dir("build-vm-script");
    let good = project_in(&dir.join("good"), "");
    fs::write(good.join("hiko.lock.toml"), "schema_version = 1\n").unwrap();
    let lock = good.join("hiko.lock.toml");
    let out = dir.join("out");
    let build = |script: &Path| {
        hiko(
            &good,
            &[
                "build-vm",
                "p.toml",
                "--lock",
                lock.to_str().unwrap(),
                "--script",
                script.to_str().unwrap(),
                "--out",
                out.to_str().unwrap(),
            ],
        )
    };

    let other = project_in(&dir.join("other"), "lockfile = \"../x\"");
    let output = build(&other.join("a.hml"));
    assert!(!output.status.success());
    let message = stderr(&output);
    assert!(
        message.contains(&outside_message(&other, "../x")),
        "got: {message}"
    );

    fs::write(other.join("hiko.toml"), "[defaults\n").unwrap();
    let output = build(&other.join("a.hml"));
    assert!(!output.status.success());
    let message = stderr(&output);
    assert!(
        message.contains("Invalid project manifest"),
        "got: {message}"
    );
}

#[cfg(unix)]
#[test]
fn build_vm_lock_does_not_follow_a_link_to_another_project() {
    let dir = empty_dir("build-vm-link");
    let parent = project_in(&dir, "");
    fs::write(parent.join("hiko.lock.toml"), "schema_version = 1\n").unwrap();
    let sub = project_in(&parent.join("nested"), "");
    std::os::unix::fs::symlink("../../hiko.lock.toml", sub.join("hiko.lock.toml")).unwrap();
    let out = dir.join("out");
    let output = hiko(
        &sub,
        &[
            "build-vm",
            "p.toml",
            "--lock",
            "hiko.lock.toml",
            "--out",
            out.to_str().unwrap(),
        ],
    );
    assert!(!output.status.success());
    let message = stderr(&output);
    assert!(
        message.contains(&outside_message(&sub, "hiko.lock.toml")),
        "got: {message}"
    );
}

#[test]
fn build_vm_reports_a_missing_script_before_its_project() {
    let dir = empty_dir("build-vm-missing-script");
    let project = project_in(&dir, "");
    fs::write(project.join("hiko.toml"), "[defaults\n").unwrap();
    let out = dir.join("out");
    let output = hiko(
        &project,
        &[
            "build-vm",
            "p.toml",
            "--script",
            "nope.hml",
            "--out",
            out.to_str().unwrap(),
        ],
    );
    assert!(!output.status.success());
    let message = stderr(&output);
    assert!(message.contains("cannot read 'nope.hml'"), "got: {message}");
}
