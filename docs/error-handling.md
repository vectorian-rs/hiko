# Hiko Error Handling

## Overview

Hiko uses `Std.Result` for recoverable failure.

```sml
datatype ('a, 'e) result =
    Ok of 'a
  | Err of 'e
```

This is the standard error-handling model for library and application code.
Hiko does not use exceptions as the primary recoverable-error mechanism.

The intended rule is:

- recoverable failures return `('a, error) Result.result`
- each library defines its own `datatype error = ...`
- higher-level libraries wrap lower-level errors with more context
- boundary code decides how errors are rendered to users

## Library-Owned Error Types

Each library should define its own error type close to the API that returns it.

```sml
structure Json = struct
  datatype error =
      InvalidSyntax of string
    | InvalidUtf8

  val parse : string -> (value, error) Result.result
end
```

This keeps failure modes explicit in types and avoids one giant global error type.

## Wrapping Lower-Level Errors

Higher-level libraries should wrap errors from lower-level libraries instead of
erasing them to strings.

```sml
structure Config = struct
  datatype error =
      ReadFile of string * Filesystem.error
    | ParseFile of string * Json.error

  fun load path =
    path
    |> Filesystem.read_text
    |> Result.map_err (fn err => ReadFile (path, err))
    |> Result.and_then Json.parse
    |> Result.map_err (fn err => ParseFile (path, err))
end
```

This is the standard layering pattern:

- leaf libraries define local errors
- callers add operation context
- apps widen to umbrella error types only when they need to

## Context Without Printing

Libraries should usually add context, but they should not usually print.

These are different concerns:

- adding context is pure data construction
- rendering or printing is I/O

For example, this is good:

```sml
datatype error =
    ReadFile of string * Filesystem.error
  | ParseFile of string * Json.error
```

This is not a good library boundary:

```sml
datatype error =
    Error of string
```

unless the library is already at the outermost user-facing boundary.

The goal is to preserve enough structure for the caller to decide how much detail
to show.

## Rendering Errors

Most libraries should not print or log their own errors. They should return
structured errors and let callers decide how to present them.

The usual pattern is:

```sml
fun render_config_error err =
  case err of
    Config.ReadFile (path, Filesystem.PermissionDenied _) =>
      "permission denied: could not open " ^ path
  | Config.ReadFile (path, Filesystem.NotFound _) =>
      "config file not found: " ^ path
  | Config.ReadFile (path, _) =>
      "could not read config file: " ^ path
  | Config.ParseFile (path, Json.InvalidSyntax msg) =>
      "invalid config syntax in " ^ path ^ ": " ^ msg
  | Config.ParseFile (path, Json.InvalidUtf8) =>
      "config file is not valid UTF-8: " ^ path
```

Then the boundary code decides what to do:

```sml
case Config.load "config.json" of
    Result.Ok cfg => run cfg
  | Result.Err err => println (render_config_error err)
```

A library may provide a pure helper like `to_string : error -> string`, but it
should still avoid printing as a side effect.

## Result Combinators

`Std.Result` is designed to work naturally with `|>`.

The most important helpers are:

- `Result.map`
- `Result.map_err`
- `Result.and_then`
- `Result.flatten`
- `Result.fold`
- `Result.or_else`

Typical pipeline style:

```sml
fun load path =
  path
  |> Filesystem.read_text
  |> Result.map_err (fn err => ReadFile (path, err))
  |> Result.and_then Json.parse
  |> Result.map_err (fn err => ParseFile (path, err))
```

## Top-Level Error Types

Applications may define one umbrella error type when they need to combine
failures from several subsystems.

```sml
structure App = struct
  datatype error =
      ConfigError of Config.error
    | HttpError of Http.error
    | DomainError of string
end
```

This is an application boundary concern, not a reason to use a single language-
wide error type.

## What Is Not A Result

Not every failure should be represented as a `Result`.

Examples of failures that are not ordinary HML-level recoverable errors:

- real host allocator or OS out-of-memory conditions
- internal runtime corruption
- fatal host process aborts

These are host/runtime failures, not ordinary library outcomes.

## Fiber Joins

`Std.Fiber.join` follows the same rule: child/process outcomes are recoverable
values, not parent process failure.

```sml
val join : 'a Fiber.t -> ('a, Fiber.error) Result.result
```

This lets callers distinguish:

- child success
- child/domain failure returned by the child itself
- runtime/process failure such as cancellation or fuel exhaustion

That means joins often produce nested `Result` values:

```sml
val child : (summary, App.error) Result.result Fiber.t = ...
val joined : ((summary, App.error) Result.result, Fiber.error) Result.result =
  Fiber.join child
```

The usual pattern is:

1. lift `Fiber.error` into your application error type
2. flatten the two `Result` layers

```sml
structure App = struct
  datatype error =
      Runtime of Fiber.error
    | S3 of string * Cloud.S3.error
    | Parquet of string * Data.Parquet.error
    | Stats of Stats.error
end

val summary =
  Fiber.spawn (fn () => App.compute_summary bucket key)
  |> Fiber.join
  |> Result.map_err App.Runtime
  |> Result.flatten
```

`Fiber.render_error : Fiber.error -> string` provides the default human-readable
rendering for process-level failures.

## Fiber Cancellation

Current `Std.Fiber` cancellation rules are:

- `Fiber.cancel fiber` is fire-and-forget
- the child observes cancellation at the next suspension point
- `Fiber.cancel` on an already-finished child is a no-op
- after cancellation, `Fiber.join fiber` returns `Result.Err ...`

For fail-fast scripts, `Fiber.join_or_fail` unwraps `Fiber.join` and panics with
the rendered fiber error.
