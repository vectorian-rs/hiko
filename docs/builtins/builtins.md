# Hiko Builtins Reference

All builtins are available as global functions. No imports needed.

Width-specific numeric APIs live in stdlib modules such as `Int32`, `Word32`,
and `Float32`; see [numerics.md](../language/numerics.md). Their raw runtime builtins are
internal aliases, not the public surface.

## Type Signature Contract

Builtin type signatures in `crates/hiko-builtin-meta/src/signatures.rs` are a
soundness-critical contract. The type checker trusts these strings as the source
of truth for builtin return types, so each signature must match the exact runtime
shape produced by the Rust implementation: scalar kind, tuple arity, list element
shape, option/data constructor layout, and opaque host-handle type.

When adding or changing a builtin, update the signature and add or update the
runtime shape sample in `crates/hiko-vm/src/shape_check.rs`. If a builtin cannot
be sampled in a unit test because it is runtime-only, non-returning, or requires
external I/O, it must have an explicit exemption there with a reason.

## Notation

Types use Hiko syntax. A builtin that takes several arguments takes one tuple,
written `a * b`: `substring : string * int * int -> string` is called as
`substring (s, start, len)`. The authoritative list is
`crates/hiko-builtin-meta/src/signatures.rs`; this page covers the common
builtins and is not exhaustive.

## I/O

| Builtin      | Type             | Description                         |
| ------------ | ---------------- | ----------------------------------- |
| `print`      | `string -> unit` | Print string to stdout (no newline) |
| `println`    | `string -> unit` | Print string to stdout with newline |
| `eprint`     | `string -> unit` | Print string to stderr (no newline) |
| `eprintln`   | `string -> unit` | Print string to stderr with newline |
| `read_line`  | `unit -> string` | Read a line from stdin              |
| `read_stdin` | `unit -> string` | Read all remaining stdin            |

## Type Conversion

| Builtin           | Type              | Description                                                             |
| ----------------- | ----------------- | ----------------------------------------------------------------------- |
| `int_to_string`   | `int -> string`   | Integer to decimal string                                               |
| `float_to_string` | `float -> string` | float to string                                                         |
| `string_to_int`   | `string -> int`   | Parse decimal string to integer                                         |
| `char_to_int`     | `char -> int`     | Character to Unicode codepoint                                          |
| `int_to_char`     | `int -> char`     | Unicode codepoint to character; errors on invalid or out-of-range input |
| `int_to_float`    | `int -> float`    | Integer to float                                                        |
| `word_to_int`     | `word -> int`     | Word to integer; errors if value is above `i64::MAX`                    |
| `int_to_word`     | `int -> word`     | Integer to word; errors if value is negative                            |
| `word_to_string`  | `word -> string`  | Word to decimal string                                                  |
| `string_to_word`  | `string -> word`  | Parse decimal string to word; errors on invalid input or overflow       |

## string Operations

| Builtin           | Type                                 | Description                            |
| ----------------- | ------------------------------------ | -------------------------------------- |
| `string_length`   | `string -> int`                      | Number of characters                   |
| `substring`       | `string * int * int -> string`       | Substring from start index with length |
| `string_contains` | `string * string -> bool`            | Does haystack contain needle           |
| `trim`            | `string -> string`                   | Strip leading/trailing whitespace      |
| `split`           | `string * string -> string list`     | Split string by delimiter              |
| `string_replace`  | `string * string * string -> string` | Replace all occurrences                |
| `string_join`     | `string list * string -> string`     | Join list with separator               |
| `starts_with`     | `string * string -> bool`            | Prefix test                            |
| `ends_with`       | `string * string -> bool`            | Suffix test                            |
| `to_upper`        | `string -> string`                   | Uppercase                              |
| `to_lower`        | `string -> string`                   | Lowercase                              |

## Math

| Builtin     | Type             | Description              |
| ----------- | ---------------- | ------------------------ |
| `sqrt`      | `float -> float` | Square root              |
| `abs_int`   | `int -> int`     | Absolute value (integer) |
| `abs_float` | `float -> float` | Absolute value (float)   |
| `floor`     | `float -> int`   | Floor to integer         |
| `ceil`      | `float -> int`   | Ceiling to integer       |

## File System

| Builtin           | Type                      | Description                               |
| ----------------- | ------------------------- | ----------------------------------------- |
| `read_file`       | `string -> string`        | Read entire file as string                |
| `read_file_bytes` | `string -> bytes`         | Read entire file as raw bytes             |
| `write_file`      | `string * string -> unit` | Write string to file (creates/overwrites) |
| `file_exists`     | `string -> bool`          | Does path exist                           |
| `remove_file`     | `string -> unit`          | Delete a file                             |
| `create_dir`      | `string -> unit`          | Create directory (recursive)              |
| `is_dir`          | `string -> bool`          | Is path a directory                       |
| `is_file`         | `string -> bool`          | Is path a regular file                    |
| `list_dir`        | `string -> string list`   | List directory entries                    |
| `glob`            | `string -> string list`   | Glob pattern matching                     |
| `walk_dir`        | `string -> string list`   | Recursive directory walk                  |

## Path

| Builtin     | Type                        | Description                                              |
| ----------- | --------------------------- | -------------------------------------------------------- |
| `path_join` | `string * string -> string` | Join two path components without touching the filesystem |

## HTTP

| Builtin        | Type                                                                                         | Description                                                        |
| -------------- | -------------------------------------------------------------------------------------------- | ------------------------------------------------------------------ |
| `http_get`     | `string -> int * (string * string) list * string`                                            | GET url, returns (status, headers, body)                           |
| `http`         | `string * string * (string * string) list * string -> int * (string * string) list * string` | Full HTTP: (method, url, headers, body) -> (status, headers, body) |
| `http_json`    | `string * string * (string * string) list * string -> int * (string * string) list * 'a`     | HTTP with JSON-parsed body                                         |
| `http_msgpack` | `string * string * (string * string) list * string -> int * (string * string) list * 'a`     | HTTP with msgpack-parsed body                                      |
| `http_bytes`   | `string * string * (string * string) list * string -> int * (string * string) list * bytes`  | HTTP with raw bytes body                                           |

In the threaded runtime, `http_get` and `read_file` suspend the process instead of blocking the worker thread.

## JSON

Use `import Std.Json` for the typed JSON ADT (`JNull | JBool | JInt | JFloat | JStr | JArray | JObject`).

| Builtin          | Type                        | Description                           |
| ---------------- | --------------------------- | ------------------------------------- |
| `Json.parse`     | `string -> Json.json`       | Parse text into a JSON value          |
| `Json.to_string` | `Json.json -> string`       | Serialize a JSON value                |
| `json_get`       | `string * string -> string` | Get field from JSON string by key     |
| `json_keys`      | `string -> string list`     | Get keys from JSON object string      |
| `json_length`    | `string -> int`             | Length of JSON array/object string    |

The JSON datatype is owned by the embedded `BuiltinJson` module. `Std.Json`
re-exports its type and constructors, so values built with `Json.JInt`,
`Json.JArray` and the other constructors can be matched and serialized directly.
Parsing as an unrelated Hiko type or serializing a function is a type error.

The host converters `json_parse` and `json_to_string` are available only while
the embedded `__Builtin.Json` module is compiled. Direct calls report the
replacement and `import Std.Json`; source cannot redefine `BuiltinJson.json`.
The existing `capabilities.json.json_parse` and
`capabilities.json.json_to_string` policy keys still control these operations.

## bytes

| Builtin           | Type                         | Description                    |
| ----------------- | ---------------------------- | ------------------------------ |
| `bytes_length`    | `bytes -> int`               | Number of bytes                |
| `bytes_to_string` | `bytes -> string`            | Decode UTF-8                   |
| `string_to_bytes` | `string -> bytes`            | Encode UTF-8                   |
| `bytes_get`       | `bytes * int -> int`         | Get byte at index (0-255)      |
| `bytes_slice`     | `bytes * int * int -> bytes` | Slice from start with length   |
| `random_bytes`    | `int -> bytes`               | Cryptographically random bytes |

## Hash

| Builtin  | Type              | Description                            |
| -------- | ----------------- | -------------------------------------- |
| `blake3` | `bytes -> string` | BLAKE3 digest as 64-char lowercase hex |

## Random Number Generation

Pure functional RNG (PCG-XSH-RR). State is threaded explicitly — no mutation.

| Builtin     | Type                       | Description                                |
| ----------- | -------------------------- | ------------------------------------------ |
| `rng_seed`  | `bytes -> rng`             | Create RNG from seed bytes                 |
| `rng_bytes` | `rng * int -> bytes * rng` | Generate N random bytes, return new state  |
| `rng_int`   | `rng * int -> int * rng`   | Random int in [0, bound), return new state |

## Regex

| Builtin         | Type                                 | Description                                    |
| --------------- | ------------------------------------ | ---------------------------------------------- |
| `regex_match`   | `string * string -> bool`            | Does string match pattern                      |
| `regex_replace` | `string * string * string -> string` | Replace matches: (input, pattern, replacement) |

## Time

| Builtin        | Type          | Description                                                                                                                                      |
| -------------- | ------------- | ------------------------------------------------------------------------------------------------------------------------------------------------ |
| `epoch`        | `unit -> int` | Unix timestamp in seconds (wall clock)                                                                                                           |
| `epoch_ms`     | `unit -> int` | Unix timestamp in milliseconds (wall clock)                                                                                                      |
| `monotonic_ms` | `unit -> int` | Monotonic clock in milliseconds since first call. Uses `Instant::now()` — never goes backward, not affected by NTP. Use for measuring durations. |
| `sleep`        | `int -> unit` | Pause for N milliseconds. In threaded runtime, suspends the process instead of blocking.                                                         |

## Environment & System

| Builtin  | Type                                            | Description                                                 |
| -------- | ----------------------------------------------- | ----------------------------------------------------------- |
| `getenv` | `string -> string`                              | Get environment variable (empty string if unset)            |
| `exec`   | `string * string list -> int * string * string` | Run command: (program, args) -> (exit_code, stdout, stderr) |
| `exit`   | `int -> unit`                                   | Exit process with status code                               |
| `panic`  | `string -> 'a`                                  | Abort with error message                                    |

### Exec policy

`exec` runs a program directly, with no shell, and only as the run config
allows. There are two kinds of permission, and a call needs one of them:

```toml
[capabilities.exec.exec]
enabled = true
# These commands may run with any arguments.
allowed_commands = ["git"]

# This command may run only with exactly these arguments.
[[capabilities.exec.exec.allow]]
argv = ["mise", "run", "//cell:sre-check"]

# With rest = "any", these arguments are a fixed prefix: more may follow.
[[capabilities.exec.exec.allow]]
argv = ["mise", "run", "//cell:logs", "--"]
rest = "any"   # default "none"
```

- **The command matches by file, not by name.** When the VM is built, each
  configured command (an `allowed_commands` entry, or `argv[0]` of a rule) is
  resolved to a file: a path is used as written, and a bare name is looked
  up in `PATH`. A call's program is resolved the same way when it runs, and
  must be the same file (device and inode on Unix). So `./mise` or
  `/opt/other/mise` is refused unless it is the configured file, and a
  symlink is followed to its target.
- **A replaced file is refused.** The VM holds each allowed executable
  open while it runs, so the file's inode cannot be freed and reused. A
  file created at the same path, by deleting and recreating it or by
  renaming over it, always has a different inode, and `exec` refuses it.
  This does not depend on timestamps (#160). An execute-only executable
  (no read permission bits at all), which the VM cannot open, is not held
  open; it is checked by device, inode, change time and size only. Any other
  error opening it (no file descriptors left, or a permission denied by an
  ACL or a security module) leaves the command out of the policy, and a
  refusal lists the error.
- **A refusal does not show the arguments**, only how many there were:
  arguments can hold secrets, and refusals end up in logs.
- **Arguments match as strings, exactly.** There is no globbing, trimming or
  splitting: `"run check"` is one argument and does not match `"run",
  "check"`; an empty string is an argument. With `rest = "any"`, only
  arguments after the prefix are free.
- A NUL in the program or an argument is refused. A rule with an empty
  `argv`, or an empty command, is a config error.
- Child processes have the same permissions as their parent.
- The program's `argv[0]` is the command the script ran (`git`, or
  `/bin/ls`), as a shell would pass it, not the resolved file or
  `/proc/self/fd/N`: busybox-style tools dispatch on it.

What the allowlist does **not** guarantee:

- **Writes to the allowed file itself.** The check compares the file's
  change time and size, so a write that keeps the size and lands within one
  timestamp tick of the policy can go unnoticed. The other way round, any
  change to the file's change time (a write, `chmod`, `touch`, a new hard
  link, an extended attribute) makes `exec` refuse it until the VM is
  started again, with an error that says the file changed. Keep allowed executables
  in directories, and as files, that the script's user cannot write.
- **A replacement between the check and the spawn, except for ELF binaries
  on Linux.** On Linux the VM runs an allowed ELF binary through the file it
  holds open (`/proc/self/fd/N`), so a file swapped in at the path does not
  run. Everything else runs by path: the VM checks the path just before it
  spawns and the OS then opens it again, so a file swapped in during that
  short window runs. On Linux that is a `#!` script or a binfmt_misc binary
  (their interpreter opens the path itself), an execute-only file (not held
  open), or a system without `/proc`. `hiko run` prints an `info:` line for
  each such command when it starts, so the weaker protection is never
  silent. macOS and other systems always run by path: macOS cannot run a
  program through an open descriptor (`execve` of `/dev/fd/N` fails with
  `EACCES`). The defence is the same: the script's user must not be able to
  write to the allowed executable or its directory.
- **A write in place after the check.** The descriptor names the same inode
  as the path, so on Linux too, a write to the allowed file itself between
  the check and the spawn runs the new content.
- **What the program does.** Arguments are fixed, but the program itself
  may read files, environment variables or configuration that the script can
  change.
- `build-vm` writes the rules into the generated binary.

## Concurrency (threaded runtime)

| Builtin         | Type                     | Description                              |
| --------------- | ------------------------ | ---------------------------------------- |
| `spawn`         | `(unit -> 'a) -> 'a pid` | Spawn an isolated process                |
| `await_process` | `'a pid -> 'a`           | Block until a child process completes    |
| `cancel`        | `'a pid -> unit`         | Cooperatively cancel a child process     |
| `wait_any`      | `'a pid list -> 'a pid`  | Wait until any child in the set finishes |

A pid records what its child returns: `spawn (fn () => 42)` is an `int pid`,
and awaiting it gives an `int`. Annotate a pid with its result type (`int pid`),
or use `'a pid` in a polymorphic wrapper. Pids with the same result type compare
with `=`; a heterogeneous `wait_any` list reports the conflicting pid types.

`BuiltinProcess.await_result` and `BuiltinProcess.error` are runtime-owned
nominal types, reserved even before `__Builtin.Process` is imported. Source
cannot redefine them with a datatype, type alias or signature ascription.
Only the embedded internal module defines their constructors; its constructor
order matches the runtime tags. Import it to match a typed await result.

`wait_any [p1, p2, ..., pn]` is deterministic: it blocks until at least one listed child has completed, then returns the leftmost PID in the original input list whose child is complete. If multiple children complete concurrently, input list order breaks ties. A child completion is only a wakeup signal; it is not automatically the winner.

The raw builtins are intended to sit under `Std.Fiber`, which provides the user-facing structured-concurrency surface (`Fiber.spawn`, `Fiber.join`, `Fiber.both`, `Fiber.all`, `Fiber.first`, `Fiber.any`). `Fiber.join` is `Result`-typed, so child/runtime failure is handled as a value rather than propagating failure to the parent. `Fiber.both` and `Fiber.all` return the first failure as soon as it happens, and cancel the other fibers.

Loading `Std.Fiber` binds all four process operations, so its policy needs
`capabilities.process.spawn`, `await_process`, `cancel`, and `wait_any` enabled.
The `benchmark` and `software-developer-role` profiles grant these explicitly.
Profiles without process permissions still cannot load the module. The host must
also compile the `builtin-process` Cargo feature; a policy cannot enable it.

## Testing

| Builtin     | Type                       | Description                   |
| ----------- | -------------------------- | ----------------------------- |
| `assert`    | `bool * string -> unit`    | Assert condition with message |
| `assert_eq` | `'a * 'a * string -> unit` | Assert equality with message  |
