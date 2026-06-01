# R&D: Agentic Hiko Scripting

This note sketches how Hiko can be used as a typed, policy-limited scripting
layer for coding agents. It is not a Pi-specific runtime design.

## Model

```text
agent or human
  -> chooses/writes a Hiko script
  -> runs it with a least-privilege config
  -> reviews stdout, artifacts, exit status, and diffs
```

Hiko is the execution boundary, not the planner. Agents may plan the work, but
Hiko should do small deterministic tasks with explicit capabilities.

## Basic workflow

```text
1. State the task.
2. Choose or write a Hiko script.
3. Choose or write a task-specific run config.
4. Type-check the script.
5. Run it.
6. Review outputs and diffs.
7. Tighten the config if it was broader than needed.
```

Useful commands:

```bash
hiko check scripts/task.hml
hiko run --config configs/task.toml scripts/task.hml
```

For repeatable distribution:

```bash
hiko build-vm configs/task.toml
cargo build --release --manifest-path hiko-vm-task/Cargo.toml
./hiko-vm-task/target/release/hiko-vm-task scripts/task.hml
```

## Patterns

### Read-only audit

```text
read repo/docs -> run analysis script -> emit report -> review
```

Config: stdout plus read-only filesystem access to selected paths. Disable
writes, exec, and network unless explicitly needed.

### Generate report

```text
read inputs -> compute summary -> write one report file
```

Config: read selected input folders, write selected output folder, and set tight
I/O and memory limits.

### Fetch and normalize

```text
call approved HTTP host -> parse response -> write normalized output
```

Config: allow only required hosts, bound response bytes with `max_io_bytes`, and
write only the intended output path.

### Controlled command wrapper

```text
validate inputs -> run one allowlisted command -> parse stdout/stderr
```

Config: allow only the exact command path, set an exec timeout, and bound output
with `max_io_bytes`.

## Rule of thumb

Treat the script and config as one unit. A safe script with an overly broad
config is not least-privilege.

Do not use Hiko as a hidden general shell. If a task needs broad filesystem,
network, and command access, the policy boundary is probably too weak to be
useful.
