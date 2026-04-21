# Hiko Builtins Reference

All builtins are available as global functions. No imports needed.

## I/O

| Builtin     | Type             | Description                         |
| ----------- | ---------------- | ----------------------------------- |
| `print`      | `string -> unit` | Print string to stdout (no newline) |
| `println`    | `string -> unit` | Print string to stdout with newline |
| `read_line`  | `unit -> string` | Read a line from stdin              |
| `read_stdin` | `unit -> string` | Read all remaining stdin            |

## Type Conversion

| Builtin           | Type              | Description                     |
| ----------------- | ----------------- | ------------------------------- |
| `int_to_string`   | `int -> string`   | Integer to decimal string       |
| `float_to_string` | `float -> string` | float to string                 |
| `string_to_int`   | `string -> int`   | Parse decimal string to integer |
| `char_to_int`     | `char -> int`     | Character to Unicode codepoint  |
| `int_to_char`     | `int -> char`     | Unicode codepoint to character  |
| `int_to_float`    | `int -> float`    | Integer to float                |

## string Operations

| Builtin           | Type                                 | Description                            |
| ----------------- | ------------------------------------ | -------------------------------------- |
| `string_length`   | `string -> int`                      | Number of characters                   |
| `substring`       | `(string, int, int) -> string`       | Substring from start index with length |
| `string_contains` | `(string, string) -> bool`           | Does haystack contain needle           |
| `trim`            | `string -> string`                   | Strip leading/trailing whitespace      |
| `split`           | `(string, string) -> string list`    | Split string by delimiter              |
| `string_replace`  | `(string, string, string) -> string` | Replace all occurrences                |
| `string_join`     | `(string list, string) -> string`    | Join list with separator               |
| `starts_with`     | `(string, string) -> bool`           | Prefix test                            |
| `ends_with`       | `(string, string) -> bool`           | Suffix test                            |
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

| Builtin           | Type                       | Description                               |
| ----------------- | -------------------------- | ----------------------------------------- |
| `read_file`       | `string -> string`         | Read entire file as string                |
| `read_file_bytes` | `string -> bytes`          | Read entire file as raw bytes             |
| `write_file`      | `(string, string) -> unit` | Write string to file (creates/overwrites) |
| `file_exists`     | `string -> bool`           | Does path exist                           |
| `remove_file`     | `string -> unit`           | Delete a file                             |
| `create_dir`      | `string -> unit`           | Create directory (recursive)              |
| `is_dir`          | `string -> bool`           | Is path a directory                       |
| `is_file`         | `string -> bool`           | Is path a regular file                    |
| `list_dir`        | `string -> string list`    | List directory entries                    |
| `glob`            | `string -> string list`    | Glob pattern matching                     |
| `walk_dir`        | `string -> string list`    | Recursive directory walk                  |

## Path

| Builtin     | Type                         | Description                                              |
| ----------- | ---------------------------- | -------------------------------------------------------- |
| `path_join` | `(string, string) -> string` | Join two path components without touching the filesystem |

## Hashline (content-addressed editing)

| Builtin            | Type                           | Description                       |
| ------------------ | ------------------------------ | --------------------------------- |
| `read_file_tagged` | `(string, int, int) -> string` | Read file with FNV-1a line hashes |
| `edit_file_tagged` | `(string, string) -> string`   | Apply hashline-based edit         |

## HTTP

| Builtin        | Type                                                                                        | Description                                                        |
| -------------- | ------------------------------------------------------------------------------------------- | ------------------------------------------------------------------ |
| `http_get`     | `string -> (int, (string * string) list, string)`                                           | GET url, returns (status, headers, body)                           |
| `http`         | `(string, string, (string * string) list, string) -> (int, (string * string) list, string)` | Full HTTP: (method, url, headers, body) -> (status, headers, body) |
| `http_json`    | `(string, string, (string * string) list, string) -> (int, (string * string) list, 'a)`     | HTTP with JSON-parsed body                                         |
| `http_msgpack` | `(string, string, (string * string) list, string) -> (int, (string * string) list, 'a)`     | HTTP with msgpack-parsed body                                      |
| `http_bytes`   | `(string, string, (string * string) list, string) -> (int, (string * string) list, bytes)`  | HTTP with raw bytes body                                           |

In the threaded runtime, `http_get` and `read_file` suspend the process instead of blocking the worker thread.

## JSON

Requires `use "libraries/Std-v0.1.0/modules/Json.hml"` for typed JSON ADT (`JNull | JBool | JInt | JFloat | JStr | JArray | JObject`).

| Builtin          | Type                         | Description                           |
| ---------------- | ---------------------------- | ------------------------------------- |
| `json_parse`     | `string -> 'a`               | Parse JSON string to typed JSON value |
| `json_to_string` | `'a -> string`               | Serialize value to JSON string        |
| `json_get`       | `(string, string) -> string` | Get field from JSON string by key     |
| `json_keys`      | `string -> string list`      | Get keys from JSON object string      |
| `json_length`    | `string -> int`              | Length of JSON array/object string    |

## bytes

| Builtin           | Type                         | Description                    |
| ----------------- | ---------------------------- | ------------------------------ |
| `bytes_length`    | `bytes -> int`               | Number of bytes                |
| `bytes_to_string` | `bytes -> string`            | Decode UTF-8                   |
| `string_to_bytes` | `string -> bytes`            | Encode UTF-8                   |
| `bytes_get`       | `(bytes, int) -> int`        | Get byte at index (0-255)      |
| `bytes_slice`     | `(bytes, int, int) -> bytes` | Slice from start with length   |
| `random_bytes`    | `int -> bytes`               | Cryptographically random bytes |

## Random Number Generation

Pure functional RNG (PCG-XSH-RR). State is threaded explicitly — no mutation.

| Builtin     | Type                         | Description                                |
| ----------- | ---------------------------- | ------------------------------------------ |
| `rng_seed`  | `bytes -> rng`               | Create RNG from seed bytes                 |
| `rng_bytes` | `(rng, int) -> (bytes, rng)` | Generate N random bytes, return new state  |
| `rng_int`   | `(rng, int) -> (int, rng)`   | Random int in [0, bound), return new state |

## Regex

| Builtin         | Type                                 | Description                                    |
| --------------- | ------------------------------------ | ---------------------------------------------- |
| `regex_match`   | `(string, string) -> bool`           | Does string match pattern                      |
| `regex_replace` | `(string, string, string) -> string` | Replace matches: (input, pattern, replacement) |

## Time

| Builtin        | Type          | Description                                                                                                                                      |
| -------------- | ------------- | ------------------------------------------------------------------------------------------------------------------------------------------------ |
| `epoch`        | `unit -> int` | Unix timestamp in seconds (wall clock)                                                                                                           |
| `epoch_ms`     | `unit -> int` | Unix timestamp in milliseconds (wall clock)                                                                                                      |
| `monotonic_ms` | `unit -> int` | Monotonic clock in milliseconds since first call. Uses `Instant::now()` — never goes backward, not affected by NTP. Use for measuring durations. |
| `sleep`        | `int -> unit` | Pause for N milliseconds. In threaded runtime, suspends the process instead of blocking.                                                         |

## Environment & System

| Builtin  | Type                                             | Description                                                 |
| -------- | ------------------------------------------------ | ----------------------------------------------------------- |
| `getenv` | `string -> string`                               | Get environment variable (empty string if unset)            |
| `exec`   | `(string, string list) -> (int, string, string)` | Run command: (program, args) -> (exit_code, stdout, stderr) |
| `exit`   | `int -> unit`                                    | Exit process with status code                               |
| `panic`  | `string -> 'a`                                   | Abort with error message                                    |

## Concurrency (threaded runtime)

| Builtin         | Type                    | Description                                          |
| --------------- | ----------------------- | ---------------------------------------------------- |
| `spawn`         | `(unit -> 'a) -> pid`   | Spawn an isolated process                            |
| `await_process` | `pid -> 'a`             | Block until a child process completes                |
| `cancel`        | `pid -> unit`           | Cooperatively cancel a child process                 |
| `wait_any`      | `pid list -> pid`       | Wait until any child in the set finishes             |

The raw builtins are intended to sit under `Std.Fiber`, which provides the user-facing structured-concurrency surface (`Fiber.spawn`, `Fiber.join`, `Fiber.first`, `Fiber.any`). `Fiber.join` is `Result`-typed, so child/runtime failure is handled as a value rather than propagating failure to the parent.

## Testing

| Builtin     | Type                       | Description                   |
| ----------- | -------------------------- | ----------------------------- |
| `assert`    | `(bool, string) -> unit`   | Assert condition with message |
| `assert_eq` | `('a, 'a, string) -> unit` | Assert equality with message  |
