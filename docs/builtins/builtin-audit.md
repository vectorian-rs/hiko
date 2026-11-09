# Builtin-to-module audit

The checked inventory for #185; runtime metadata remains authoritative in
[the metadata crate](../../crates/hiko-builtin-meta/src/lib.rs). This document
is mapping data, never a registry used by the compiler or VM. Tests require
one row per name across **all** families, even disabled optional features.

## Naming and availability

Public names use semantic nouns and module-owned `of_x`/`to_x` conversions.
The module column is the intended owner, not a claim that every wrapper exists:
math members, Fiber.wait_any and Time.epoch remain
proposed; no extra primitive or wrapper is authorized by an inventory row. Primitive means the existing Rust
or runtime operation is retained; wrappers may reorder arguments, handle effects
or compose cleanup. A new primitive requires irreducible behavior or measured
justification; no new primitive is introduced here.

#185 adds only Int/Float/Char/Word/Path/System thin modules. Other modules and
raw APIs keep their behavior. All six new modules defer builtin lookup
until invocation in non-strict runs: importing alone grants no capability.
`--strict` checks all imported bodies, including unused functions of the noun.
System requires both `builtin-system` and `builtin-testing`; strict use requires
both exit/panic leaves, but non-strict calls require only their own leaf. Existing
eager embedded modules (including Numeric, Time and Stdio) stay unchanged.

Current keys are exact runtime policy leaves; `-` means an internal operation
without a policy leaf. Module-only names and runtime aliases are not new public
raw APIs.
Proposed keys are **inactive audit data** for #186, not accepted policy aliases;
#186 alone activates the final naming/import/key migration. An existing leaf may
control a runtime-only companion (await_process_result), not a separate grant.
Features are unchanged; availability never implies runtime permission.

## Inventory

| Name | Feature | Current key | Module function | Proposed key | Rationale |
| --- | --- | --- | --- | --- | --- |
| abs_float | builtin-math | capabilities.math.abs_float | Std.Float.abs | capabilities.float.abs | primitive: existing scalar/codec operation |
| abs_int | builtin-math | capabilities.math.abs_int | Std.Int.abs | capabilities.int.abs | primitive: existing scalar/codec operation |
| assert | builtin-testing | capabilities.testing.assert | Std.Testing.check | capabilities.testing.check | primitive: failure/assertion boundary |
| assert_eq | builtin-testing | capabilities.testing.assert_eq | Std.Testing.equal | capabilities.testing.equal | primitive: failure/assertion boundary |
| await_process | builtin-process | capabilities.process.await_process | Std.Fiber.join_or_fail | capabilities.fiber.join_or_fail | primitive: runtime lifecycle; wrappers compose cleanup |
| await_process_result | builtin-process | capabilities.process.await_process | Std.Fiber.join | capabilities.fiber.join | primitive: runtime lifecycle; wrappers compose cleanup |
| blake3 | builtin-hash | capabilities.hash.blake3 | Hash.Blake3.hash | capabilities.hash.blake3.hash | primitive: existing scalar/codec operation |
| bytes_get | builtin-bytes | capabilities.bytes.bytes_get | Std.Bytes.get | capabilities.bytes.get | primitive: existing scalar/codec operation |
| bytes_length | builtin-bytes | capabilities.bytes.bytes_length | Std.Bytes.length | capabilities.bytes.length | primitive: existing scalar/codec operation |
| bytes_slice | builtin-bytes | capabilities.bytes.bytes_slice | Std.Bytes.slice | capabilities.bytes.slice | primitive: existing scalar/codec operation |
| bytes_to_string | builtin-bytes | capabilities.bytes.bytes_to_string | Std.Bytes.to_string | capabilities.bytes.to_string | primitive: existing scalar/codec operation |
| cancel | builtin-process | capabilities.process.cancel | Std.Fiber.cancel | capabilities.fiber.cancel | primitive: runtime lifecycle; wrappers compose cleanup |
| ceil | builtin-math | capabilities.math.ceil | Std.Float.ceil | capabilities.float.ceil | primitive: existing scalar/codec operation |
| char_to_int | builtin-convert | capabilities.convert.char_to_int | Std.Char.to_int | capabilities.char.to_int | primitive: existing scalar/codec operation |
| create_dir | builtin-filesystem | capabilities.filesystem.create_dir | Std.Filesystem.create_directory | capabilities.filesystem.create_directory | primitive: host I/O; effect wrapper composes handling |
| date_day | builtin-date | - | Std.Date.day | - | primitive: timezone/calendar representation |
| date_fixed_offset | builtin-date | - | Std.Date.fixed_offset | - | primitive: timezone/calendar representation |
| date_format | builtin-date | - | Std.Date.format | - | primitive: timezone/calendar representation |
| date_from_instant | builtin-date | - | Std.Date.from_instant | - | primitive: timezone/calendar representation |
| date_hour | builtin-date | - | Std.Date.hour | - | primitive: timezone/calendar representation |
| date_in_timezone | builtin-date | - | Std.Date.in_timezone | - | primitive: timezone/calendar representation |
| date_local_tz | builtin-date | - | Std.Date.local_timezone | - | primitive: timezone/calendar representation |
| date_millisecond | builtin-date | - | Std.Date.millisecond | - | primitive: timezone/calendar representation |
| date_minute | builtin-date | - | Std.Date.minute | - | primitive: timezone/calendar representation |
| date_month | builtin-date | - | Std.Date.month | - | primitive: timezone/calendar representation |
| date_now_in | builtin-date | - | Std.Date.now_in | - | primitive: timezone/calendar representation |
| date_parse_rfc3339 | builtin-date | - | Std.Date.parse_rfc3339 | - | primitive: timezone/calendar representation |
| date_parse_rfc9557 | builtin-date | - | Std.Date.parse_rfc9557 | - | primitive: timezone/calendar representation |
| date_second | builtin-date | - | Std.Date.second | - | primitive: timezone/calendar representation |
| date_timezone_of | builtin-date | - | Std.Date.timezone_of | - | primitive: timezone/calendar representation |
| date_to_epoch_ms | builtin-date | - | Std.Date.to_instant | - | primitive: timezone/calendar representation |
| date_to_rfc2822 | builtin-date | - | Std.Date.to_rfc2822 | - | primitive: timezone/calendar representation |
| date_to_rfc3339 | builtin-date | - | Std.Date.to_rfc3339 | - | primitive: timezone/calendar representation |
| date_to_timezone | builtin-date | - | Std.Date.to_timezone | - | primitive: timezone/calendar representation |
| date_utc_now | builtin-date | - | Std.Date.utc_now | - | primitive: timezone/calendar representation |
| date_utc_tz | builtin-date | - | Std.Date.utc | - | primitive: timezone/calendar representation |
| date_weekday | builtin-date | - | Std.Date.weekday | - | primitive: timezone/calendar representation |
| date_year | builtin-date | - | Std.Date.year | - | primitive: timezone/calendar representation |
| ends_with | builtin-string | capabilities.string.ends_with | Std.String.ends_with | capabilities.string.ends_with | primitive: existing scalar/codec operation |
| epoch | builtin-time | capabilities.time.epoch | Std.Time.epoch | capabilities.time.epoch | primitive: host clock/scheduler boundary |
| epoch_ms | builtin-time | capabilities.time.epoch_ms | Std.Time.now | capabilities.time.now | primitive: host clock/scheduler boundary |
| eprint | builtin-stdio | capabilities.stdio.eprint | Std.Stdio.eprint | capabilities.stdio.eprint | primitive: host stream boundary |
| eprintln | builtin-stdio | capabilities.stdio.eprintln | Std.Stdio.eprintln | capabilities.stdio.eprintln | primitive: host stream boundary |
| exec | builtin-exec | capabilities.exec.exec | Std.Exec.run | capabilities.exec.run | primitive: pinned subprocess boundary |
| exit | builtin-system | capabilities.system.exit | Std.System.exit | capabilities.system.exit | primitive: host exit boundary |
| file_exists | builtin-filesystem | capabilities.filesystem.file_exists | Std.Filesystem.exists | capabilities.filesystem.exists | primitive: host I/O; effect wrapper composes handling |
| float_to_string | builtin-convert | capabilities.convert.float_to_string | Std.Float.to_string | capabilities.float.to_string | primitive: existing scalar/codec operation |
| floor | builtin-math | capabilities.math.floor | Std.Float.floor | capabilities.float.floor | primitive: existing scalar/codec operation |
| getenv | builtin-env | capabilities.env.getenv | Std.Env.get | capabilities.env.get | primitive: host environment read |
| glob | builtin-filesystem | capabilities.filesystem.glob | Std.Filesystem.glob_matches | capabilities.filesystem.glob_matches | primitive: host I/O; effect wrapper composes handling |
| http | builtin-http | capabilities.http.http | Http.Client.request | capabilities.http.client.request | primitive: host protocol/codec boundary |
| http_bytes | builtin-http | capabilities.http.http_bytes | Http.Client.request_bytes | capabilities.http.client.request_bytes | primitive: host protocol/codec boundary |
| http_get | builtin-http | capabilities.http.http_get | Http.Client.get | capabilities.http.client.get | primitive: host protocol/codec boundary |
| http_json | builtin-http | capabilities.http.http_json | Http.Client.request_json | capabilities.http.client.request_json | primitive: host protocol/codec boundary |
| http_msgpack | builtin-http | capabilities.http.http_msgpack | Http.Client.request_msgpack | capabilities.http.client.request_msgpack | primitive: host protocol/codec boundary |
| int_to_char | builtin-convert | capabilities.convert.int_to_char | Std.Char.of_int | capabilities.char.of_int | primitive: existing scalar/codec operation |
| int_to_float | builtin-convert | capabilities.convert.int_to_float | Std.Float.of_int | capabilities.float.of_int | primitive: existing scalar/codec operation |
| int_to_string | builtin-convert | capabilities.convert.int_to_string | Std.Int.to_string | capabilities.int.to_string | primitive: existing scalar/codec operation |
| int_to_word | builtin-convert | capabilities.convert.int_to_word | Std.Word.of_int | capabilities.word.of_int | primitive: existing scalar/codec operation |
| is_dir | builtin-filesystem | capabilities.filesystem.is_dir | Std.Filesystem.is_directory | capabilities.filesystem.is_directory | primitive: host I/O; effect wrapper composes handling |
| is_file | builtin-filesystem | capabilities.filesystem.is_file | Std.Filesystem.is_regular_file | capabilities.filesystem.is_regular_file | primitive: host I/O; effect wrapper composes handling |
| json_get | builtin-json | capabilities.json.json_get | Std.Json.get | capabilities.json.get | primitive: existing scalar/codec operation |
| json_keys | builtin-json | capabilities.json.json_keys | Std.Json.keys | capabilities.json.keys | primitive: existing scalar/codec operation |
| json_length | builtin-json | capabilities.json.json_length | Std.Json.length | capabilities.json.length | primitive: existing scalar/codec operation |
| json_parse | builtin-json | capabilities.json.json_parse | Std.Json.parse | capabilities.json.parse | primitive: existing scalar/codec operation |
| json_to_string | builtin-json | capabilities.json.json_to_string | Std.Json.to_string | capabilities.json.to_string | primitive: existing scalar/codec operation |
| list_dir | builtin-filesystem | capabilities.filesystem.list_dir | Std.Filesystem.list_entries | capabilities.filesystem.list_entries | primitive: host I/O; effect wrapper composes handling |
| monotonic_ms | builtin-time | capabilities.time.monotonic_ms | Std.Time.monotonic_now | capabilities.time.monotonic_now | primitive: host clock/scheduler boundary |
| numeric_float32_add | builtin-convert | - | Std.Float32.add | - | primitive: checked/width-specific representation |
| numeric_float32_div | builtin-convert | - | Std.Float32.div | - | primitive: checked/width-specific representation |
| numeric_float32_mul | builtin-convert | - | Std.Float32.mul | - | primitive: checked/width-specific representation |
| numeric_float32_neg | builtin-convert | - | Std.Float32.neg | - | primitive: checked/width-specific representation |
| numeric_float32_of_float | builtin-convert | - | Std.Float32.of_float | - | primitive: checked/width-specific representation |
| numeric_float32_sub | builtin-convert | - | Std.Float32.sub | - | primitive: checked/width-specific representation |
| numeric_float32_to_float | builtin-convert | - | Std.Float32.to_float | - | primitive: checked/width-specific representation |
| numeric_int32_add | builtin-convert | - | Std.Int32.add | - | primitive: checked/width-specific representation |
| numeric_int32_checked_add | builtin-convert | - | Std.Int32.checked_add | - | primitive: checked/width-specific representation |
| numeric_int32_checked_of_int | builtin-convert | - | Std.Int32.checked_of_int | - | primitive: checked/width-specific representation |
| numeric_int32_div | builtin-convert | - | Std.Int32.div | - | primitive: checked/width-specific representation |
| numeric_int32_max_value | builtin-convert | - | Std.Int32.max_value | - | primitive: checked/width-specific representation |
| numeric_int32_min_value | builtin-convert | - | Std.Int32.min_value | - | primitive: checked/width-specific representation |
| numeric_int32_mul | builtin-convert | - | Std.Int32.mul | - | primitive: checked/width-specific representation |
| numeric_int32_neg | builtin-convert | - | Std.Int32.neg | - | primitive: checked/width-specific representation |
| numeric_int32_of_int | builtin-convert | - | Std.Int32.of_int | - | primitive: checked/width-specific representation |
| numeric_int32_rem | builtin-convert | - | Std.Int32.rem | - | primitive: checked/width-specific representation |
| numeric_int32_saturating_add | builtin-convert | - | Std.Int32.saturating_add | - | primitive: checked/width-specific representation |
| numeric_int32_sub | builtin-convert | - | Std.Int32.sub | - | primitive: checked/width-specific representation |
| numeric_int32_to_int | builtin-convert | - | Std.Int32.to_int | - | primitive: checked/width-specific representation |
| numeric_int32_wrapping_add | builtin-convert | - | Std.Int32.wrapping_add | - | primitive: checked/width-specific representation |
| numeric_word32_add | builtin-convert | - | Std.Word32.add | - | primitive: checked/width-specific representation |
| numeric_word32_checked_add | builtin-convert | - | Std.Word32.checked_add | - | primitive: checked/width-specific representation |
| numeric_word32_checked_of_int | builtin-convert | - | Std.Word32.checked_of_int | - | primitive: checked/width-specific representation |
| numeric_word32_checked_of_word | builtin-convert | - | Std.Word32.checked_of_word | - | primitive: checked/width-specific representation |
| numeric_word32_div | builtin-convert | - | Std.Word32.div | - | primitive: checked/width-specific representation |
| numeric_word32_max_value | builtin-convert | - | Std.Word32.max_value | - | primitive: checked/width-specific representation |
| numeric_word32_min_value | builtin-convert | - | Std.Word32.min_value | - | primitive: checked/width-specific representation |
| numeric_word32_mul | builtin-convert | - | Std.Word32.mul | - | primitive: checked/width-specific representation |
| numeric_word32_of_int | builtin-convert | - | Std.Word32.of_int | - | primitive: checked/width-specific representation |
| numeric_word32_of_word | builtin-convert | - | Std.Word32.of_word | - | primitive: checked/width-specific representation |
| numeric_word32_rem | builtin-convert | - | Std.Word32.rem | - | primitive: checked/width-specific representation |
| numeric_word32_saturating_add | builtin-convert | - | Std.Word32.saturating_add | - | primitive: checked/width-specific representation |
| numeric_word32_sub | builtin-convert | - | Std.Word32.sub | - | primitive: checked/width-specific representation |
| numeric_word32_to_int | builtin-convert | - | Std.Word32.to_int | - | primitive: checked/width-specific representation |
| numeric_word32_to_word | builtin-convert | - | Std.Word32.to_word | - | primitive: checked/width-specific representation |
| panic | builtin-testing | capabilities.testing.panic | Std.System.panic | capabilities.system.panic | primitive: failure/assertion boundary |
| path_join | builtin-path | capabilities.path.path_join | Std.Path.join | capabilities.path.join | primitive: platform path semantics |
| print | builtin-stdio | capabilities.stdio.print | Std.Stdio.print | capabilities.stdio.print | primitive: host stream boundary |
| println | builtin-stdio | capabilities.stdio.println | Std.Stdio.println | capabilities.stdio.println | primitive: host stream boundary |
| random_bytes | builtin-random | capabilities.random.random_bytes | Std.Random.entropy | capabilities.random.entropy | primitive: entropy or opaque RNG state |
| read_file | builtin-filesystem | capabilities.filesystem.read_file | Std.Filesystem.read_text | capabilities.filesystem.read_text | primitive: host I/O; effect wrapper composes handling |
| read_file_bytes | builtin-filesystem | capabilities.filesystem.read_file_bytes | Std.Filesystem.read_bytes | capabilities.filesystem.read_bytes | primitive: host I/O; effect wrapper composes handling |
| read_line | builtin-stdio | capabilities.stdio.read_line | Std.Stdio.read_line | capabilities.stdio.read_line | primitive: host stream boundary |
| read_stdin | builtin-stdio | capabilities.stdio.read_stdin | Std.Stdio.read_stdin | capabilities.stdio.read_stdin | primitive: host stream boundary |
| regex_match | builtin-regex | capabilities.regex.regex_match | Std.Regex.matches | capabilities.regex.matches | primitive: existing scalar/codec operation |
| regex_replace | builtin-regex | capabilities.regex.regex_replace | Std.Regex.replace | capabilities.regex.replace | primitive: existing scalar/codec operation |
| remove_file | builtin-filesystem | capabilities.filesystem.remove_file | Std.Filesystem.remove | capabilities.filesystem.remove | primitive: host I/O; effect wrapper composes handling |
| rng_bytes | builtin-random | capabilities.random.rng_bytes | Std.Random.bytes | capabilities.random.bytes | primitive: entropy or opaque RNG state |
| rng_int | builtin-random | capabilities.random.rng_int | Std.Random.int_between | capabilities.random.int_between | primitive: entropy or opaque RNG state |
| rng_seed | builtin-random | capabilities.random.rng_seed | Std.Random.seed | capabilities.random.seed | primitive: entropy or opaque RNG state |
| sleep | builtin-time | capabilities.time.sleep | Std.Time.sleep | capabilities.time.sleep | primitive: host clock/scheduler boundary |
| spawn | builtin-process | capabilities.process.spawn | Std.Fiber.spawn | capabilities.fiber.spawn | primitive: runtime lifecycle; wrappers compose cleanup |
| split | builtin-string | capabilities.string.split | Std.String.split | capabilities.string.split | primitive: existing scalar/codec operation |
| sqrt | builtin-math | capabilities.math.sqrt | Std.Float.sqrt | capabilities.float.sqrt | primitive: existing scalar/codec operation |
| starts_with | builtin-string | capabilities.string.starts_with | Std.String.starts_with | capabilities.string.starts_with | primitive: existing scalar/codec operation |
| string_contains | builtin-string | capabilities.string.string_contains | Std.String.contains | capabilities.string.contains | primitive: existing scalar/codec operation |
| string_join | builtin-string | capabilities.string.string_join | Std.String.join | capabilities.string.join | primitive: existing scalar/codec operation |
| string_length | builtin-string | capabilities.string.string_length | Std.String.length | capabilities.string.length | primitive: existing scalar/codec operation |
| string_replace | builtin-string | capabilities.string.string_replace | Std.String.replace | capabilities.string.replace | primitive: existing scalar/codec operation |
| string_to_bytes | builtin-bytes | capabilities.bytes.string_to_bytes | Std.Bytes.of_string | capabilities.bytes.of_string | primitive: existing scalar/codec operation |
| string_to_int | builtin-convert | capabilities.convert.string_to_int | Std.Int.of_string | capabilities.int.of_string | primitive: existing scalar/codec operation |
| string_to_word | builtin-convert | capabilities.convert.string_to_word | Std.Word.of_string | capabilities.word.of_string | primitive: existing scalar/codec operation |
| substring | builtin-string | capabilities.string.substring | Std.String.substring | capabilities.string.substring | primitive: existing scalar/codec operation |
| to_lower | builtin-string | capabilities.string.to_lower | Std.String.lower | capabilities.string.lower | primitive: existing scalar/codec operation |
| to_upper | builtin-string | capabilities.string.to_upper | Std.String.upper | capabilities.string.upper | primitive: existing scalar/codec operation |
| trim | builtin-string | capabilities.string.trim | Std.String.trim | capabilities.string.trim | primitive: existing scalar/codec operation |
| wait_any | builtin-process | capabilities.process.wait_any | Std.Fiber.wait_any | capabilities.fiber.wait_any | primitive: runtime lifecycle; wrappers compose cleanup |
| walk_dir | builtin-filesystem | capabilities.filesystem.walk_dir | Std.Filesystem.walk | capabilities.filesystem.walk | primitive: host I/O; effect wrapper composes handling |
| word_to_int | builtin-convert | capabilities.convert.word_to_int | Std.Word.to_int | capabilities.word.to_int | primitive: existing scalar/codec operation |
| word_to_string | builtin-convert | capabilities.convert.word_to_string | Std.Word.to_string | capabilities.word.to_string | primitive: existing scalar/codec operation |
| write_file | builtin-filesystem | capabilities.filesystem.write_file | Std.Filesystem.write_text | capabilities.filesystem.write_text | primitive: host I/O; effect wrapper composes handling |
