# Formatting and comment placement

Hiko source is formatted through the AST formatter. Comments are preserved when they follow the formatter's public-library convention.

## Comment placement convention

Use comments as documentation for declarations:

```sml
(* Parse a JSON string into Json.json. *)
fun parse text =
  ...
```

Short trailing declaration comments are also allowed:

```sml
val timeout_ms = 5000  (* default network timeout *)
```

Avoid comments inside expressions, patterns, and types. Move the explanation to a declaration-leading comment or extract the expression into a named helper declaration.

Avoid this:

```sml
val result =
  input
  |> normalize
  (* Validate after normalization. *)
  |> validate
```

Prefer this:

```sml
(* Validate after normalization. *)
val result =
  input
  |> normalize
  |> validate
```

For case branches, prefer named helpers or a declaration-level comment until branch-comment support is implemented:

```sml
(* Render process/fiber errors for users. *)
fun render_error err =
  case err of
      RuntimeError msg => msg
    | Cancelled => "cancelled"
```

## Implementation policy

`hiko fmt` enforces this convention. If it finds an expression-level or dangling comment that it cannot safely preserve, it rejects formatting with a diagnostic instead of silently dropping or moving the comment.

Supported today:

- comments before top-level declarations;
- comments before declarations inside `structure`, `local`, and similar declaration lists;
- short comments after a declaration on the same line;
- comment-only files.

Future formatter work may add structured support for comments attached to case branches, pipeline stages, let bodies, patterns, and type expressions.
