
# Hiko Builtins Reference

All builtins are available as global functions. No imports needed.

## I/O

| Builtin | Type | Description |
|---------|------|-------------|
| `print` | `String -> Unit` | Print string to stdout (no newline) |
| `println` | `String -> Unit` | Print string to stdout with newline |
| `read_line` | `Unit -> String` | Read a line from stdin |

## Type Conversion

| Builtin | Type | Description |
|---------|------|-------------|
| `int_to_string` | `Int -> String` | Integer to decimal string |
| `float_to_string` | `Float -> String` | Float to string |
| `string_to_int` | `String -> Int` | Parse decimal string to integer |
| `char_to_int` | `Char -> Int` | Character to Unicode codepoint |
| `int_to_char` | `Int -> Char` | Unicode codepoint to character |
| `int_to_float` | `Int -> Float` | Integer to float |

## String Operations

| Builtin | Type | Description |
|---------|------|-------------|
| `string_length` | `String -> Int` | Number of characters |
| `substring` | `(String, Int, Int) -> String` | Substring from start index with length |
| `string_contains` | `(String, String) -> Bool` | Does haystack contain needle |
| `trim` | `String -> String` | Strip leading/trailing whitespace |
| `split` | `(String, String) -> String list` | Split string by delimiter |
| `string_replace` | `(String, String, String) -> String` | Replace all occurrences |
| `string_join` | `(String list, String) -> String` | Join list with separator |
| `starts_with` | `(String, String) -> Bool` | Prefix test |
| `ends_with` | `(String, String) -> Bool` | Suffix test |
| `to_upper` | `String -> String` | Uppercase |
| `to_lower` | `String -> String` | Lowercase |

## Math

| Builtin | Type | Description |
|---------|------|-------------|
| `sqrt` | `Float -> Float` | Square root |
| `abs_int` | `Int -> Int` | Absolute value (integer) |
| `abs_float` | `Float -> Float` | Absolute value (float) |
| `floor` | `Float -> Int` | Floor to integer |
| `ceil` | `Float -> Int` | Ceiling to integer |

## File System

| Builtin | Type | Description |
|---------|------|-------------|
| `read_file` | `String -> String` | Read entire file as string |
| `write_file` | `(String, String) -> Unit` | Write string to file (creates/overwrites) |
| `file_exists` | `String -> Bool` | Does path exist |
| `remove_file` | `String -> Unit` | Delete a file |
| `create_dir` | `String -> Unit` | Create directory (recursive) |
| `is_dir` | `String -> Bool` | Is path a directory |
| `is_file` | `String -> Bool` | Is path a regular file |
| `path_join` | `(String, String) -> String` | Join two path components |
| `list_dir` | `String -> String list` | List directory entries |
| `glob` | `String -> String list` | Glob pattern matching |
| `walk_dir` | `String -> String list` | Recursive directory walk |

## Hashline (content-addressed editing)

| Builtin | Type | Description |
|---------|------|-------------|
| `read_file_tagged` | `(String, Int, Int) -> String` | Read file with FNV-1a line hashes |
| `edit_file_tagged` | `(String, String) -> String` | Apply hashline-based edit |

## HTTP

| Builtin | Type | Description |
|---------|------|-------------|
| `http_get` | `String -> (Int, (String * String) list, String)` | GET url, returns (status, headers, body) |
| `http` | `(String, String, (String * String) list, String) -> (Int, (String * String) list, String)` | Full HTTP: (method, url, headers, body) -> (status, headers, body) |
| `http_json` | `(String, String, (String * String) list, String) -> (Int, (String * String) list, 'a)` | HTTP with JSON-parsed body |
| `http_msgpack` | `(String, String, (String * String) list, String) -> (Int, (String * String) list, 'a)` | HTTP with msgpack-parsed body |
| `http_bytes` | `(String, String, (String * String) list, String) -> (Int, (String * String) list, Bytes)` | HTTP with raw bytes body |

In the threaded runtime, `http_get` and `read_file` suspend the process instead of blocking the worker thread.

## JSON

Requires `use "stdlib/json.hml"` for typed JSON ADT (`JNull | JBool | JInt | JFloat | JStr | JArray | JObject`).

| Builtin | Type | Description |
|---------|------|-------------|
| `json_parse` | `String -> 'a` | Parse JSON string to typed JSON value |
| `json_to_string` | `'a -> String` | Serialize value to JSON string |
| `json_get` | `(String, String) -> String` | Get field from JSON string by key |
| `json_keys` | `String -> String list` | Get keys from JSON object string |
| `json_length` | `String -> Int` | Length of JSON array/object string |

## Bytes

| Builtin | Type | Description |
|---------|------|-------------|
| `bytes_length` | `Bytes -> Int` | Number of bytes |
| `bytes_to_string` | `Bytes -> String` | Decode UTF-8 |
| `string_to_bytes` | `String -> Bytes` | Encode UTF-8 |
| `bytes_get` | `(Bytes, Int) -> Int` | Get byte at index (0-255) |
| `bytes_slice` | `(Bytes, Int, Int) -> Bytes` | Slice from start with length |
| `random_bytes` | `Int -> Bytes` | Cryptographically random bytes |

## Random Number Generation

Pure functional RNG (PCG-XSH-RR). State is threaded explicitly — no mutation.

| Builtin | Type | Description |
|---------|------|-------------|
| `rng_seed` | `Bytes -> Rng` | Create RNG from seed bytes |
| `rng_bytes` | `(Rng, Int) -> (Bytes, Rng)` | Generate N random bytes, return new state |
| `rng_int` | `(Rng, Int) -> (Int, Rng)` | Random int in [0, bound), return new state |

## Regex

| Builtin | Type | Description |
|---------|------|-------------|
| `regex_match` | `(String, String) -> Bool` | Does string match pattern |
| `regex_replace` | `(String, String, String) -> String` | Replace matches: (input, pattern, replacement) |

## Time

| Builtin | Type | Description |
|---------|------|-------------|
| `epoch` | `Unit -> Int` | Unix timestamp in seconds (wall clock) |
| `epoch_ms` | `Unit -> Int` | Unix timestamp in milliseconds (wall clock) |
| `monotonic_ms` | `Unit -> Int` | Monotonic clock in milliseconds since first call. Uses `Instant::now()` — never goes backward, not affected by NTP. Use for measuring durations. |
| `sleep` | `Int -> Unit` | Pause for N milliseconds. In threaded runtime, suspends the process instead of blocking. |

## Environment & System

| Builtin | Type | Description |
|---------|------|-------------|
| `getenv` | `String -> String` | Get environment variable (empty string if unset) |
| `exec` | `(String, String list) -> (Int, String, String)` | Run command: (program, args) -> (exit_code, stdout, stderr) |
| `exit` | `Int -> Unit` | Exit process with status code |
| `panic` | `String -> 'a` | Abort with error message |

## Concurrency (threaded runtime)

| Builtin | Type | Description |
|---------|------|-------------|
| `spawn` | `(Unit -> 'a) -> Int` | Spawn isolated process, returns pid |
| `await_process` | `Int -> 'a` | Block until process completes, returns result |
| `send_message` | `(Int, 'a) -> Unit` | Send value to process mailbox |
| `receive_message` | `Unit -> 'a` | Receive from own mailbox (blocks if empty) |

## Testing

| Builtin | Type | Description |
|---------|------|-------------|
| `assert` | `(Bool, String) -> Unit` | Assert condition with message |
| `assert_eq` | `('a, 'a, String) -> Unit` | Assert equality with message |
