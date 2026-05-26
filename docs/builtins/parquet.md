# Parquet in Hiko

## Goal

The first Parquet feature in Hiko should make it easy to:

- open a Parquet file
- inspect its schema
- preview a small number of rows
- print a readable summary

It should **not** try to expose the entire file as an eager Hiko value.

## Design Principles

### 1. Native capability, simple Hiko surface

Parquet parsing should live in Rust as a host capability. Hiko should see a small, inspectable API.

### 2. Opaque table handle

A Parquet file can be large and columnar. Converting the whole thing to a Hiko list of rows would be slow, memory-heavy, and the wrong default.

So the main data object should be an opaque handle:

```sml
Parquet.table
```

### 3. Materialize only small views

Hiko should materialize:

- schema information
- small row previews
- formatted text output

That is enough for inspection, debugging, and scripting workflows.

## Proposed User-Facing API

```sml
signature PARQUET = sig
  type table
  type schema
  type row = (string * value) list

  datatype value =
      Null
    | Bool of bool
    | Int of int
    | Float of float
    | String of string
    | Bytes of bytes
    | List of value list
    | Struct of (string * value) list
    | Decimal of string
    | Date of string
    | Timestamp of string

  val read : string -> table
  val schema : table -> schema
  val columns : table -> string list
  val num_rows : table -> int
  val head : table -> int -> row list
  val show : table -> int -> string
end
```

Example:

```sml
val t = Parquet.read "./data/users.parquet"

println (Parquet.show t 10)
println (int_to_string (Parquet.num_rows t))
```

Or:

```sml
val rows = Parquet.head t 5
```

## Why `row = (string * value) list`

The first Parquet surface uses association-list rows because records are not
available yet, not because records would be the wrong abstraction here.

An association-list row:

```sml
type row = (string * value) list
```

is enough for:

- previewing data
- writing formatters
- simple row-oriented scripting
- nested struct rendering

This keeps the first design compatible with Hiko's current datatype-oriented style.

Once records land, they should become the preferred surface for structured row
inspection. Records now sit ahead of mutable state on the roadmap precisely
because APIs like this are already paying tuple/alist ceremony costs.

## Schema Representation

The public `schema` type can start opaque. That keeps the first implementation simple:

```sml
type schema
```

Useful initial functions:

```sml
val schema : table -> schema
val show_schema : schema -> string
```

If Hiko later needs more introspection, the schema can become a public datatype like:

```sml
datatype logical_type =
    LBool
  | LInt
  | LFloat
  | LString
  | LBytes
  | LDecimal
  | LDate
  | LTimestamp
  | LList of logical_type
  | LStruct of field list

and field = Field of {
  name : string,
  nullable : bool,
  ty : logical_type
}
```

But that should be a later step. For the first version, `show_schema` is probably enough.

## Value Mapping

The first version should map Parquet values conservatively:

- boolean -> `Bool`
- integer -> `Int`
- floating-point -> `Float`
- UTF-8 text -> `String`
- binary -> `Bytes`
- list -> `List`
- struct -> `Struct`
- null -> `Null`

Logical types that need careful formatting should start as strings:

- decimal -> `Decimal of string`
- date -> `Date of string`
- timestamp -> `Timestamp of string`

That avoids premature commitment to Hiko-native decimal/time types.

## Display Surface

`show` should be a first-class convenience API.

Example:

```sml
println (Parquet.show t 10)
```

The output should be human-readable and table-like, suitable for:

- quick CLI inspection
- logs
- harness tools

It should truncate wide values and nested structures sensibly.

## Capability Model

Parquet is a filesystem-backed native capability, so it should be controlled explicitly by run config.

Proposed config shape:

```toml
[capabilities.parquet.read]
enabled = true
folders = ["./data", "./fixtures"]
```

Semantics:

- `Parquet.read` is only available when `capabilities.parquet.read.enabled = true`
- the target file must live under one of the allowed `folders`

This keeps Parquet aligned with the existing capability model instead of inventing a special rule.

## Implementation Shape

Public API:

```sml
structure Parquet :> PARQUET = struct
  val read = __builtin_parquet_read
  val schema = __builtin_parquet_schema
  val columns = __builtin_parquet_columns
  val num_rows = __builtin_parquet_num_rows
  val head = __builtin_parquet_head
  val show = __builtin_parquet_show
end
```

The builtin names above are internal only. The stable user-facing surface is the `Parquet` module.

Internally:

- `table` is a native opaque handle
- the VM does not eagerly copy the whole file into Hiko values
- `head` materializes only a bounded row slice
- `show` can format directly from native data without materializing more than necessary

## Why Not Return `row list` from `read`

That would be the wrong default:

- large allocations
- slow startup
- loss of columnar advantages
- unclear behavior on big files

So `read : string -> table` is the correct baseline.

## Streaming and Performance

Parquet is columnar, so the efficient execution model is not “decode one row at a time into Hiko values”.

The efficient model is:

1. open the file
2. inspect schema and metadata
3. select only the needed columns
4. iterate row groups or native batches
5. materialize rows only when Hiko actually needs them

That means Hiko should optimize around **batches first, rows second**.

### Important optimizations

- **Column projection**
  - only decode the requested columns
- **Row-group pruning**
  - skip row groups when metadata makes that possible
- **Limit pushdown**
  - `head 10` should stop as soon as it has enough rows
- **Batch decoding**
  - decode in chunks instead of row-by-row
- **Lazy materialization**
  - keep data in native columnar form until rows are explicitly requested
- **Buffer reuse**
  - reuse decode buffers across batches
- **Optional parallel row-group reads**
  - a later Rust-side optimization for large files

### Why batch-first matters

If Hiko starts with a row-at-a-time API as the primary abstraction, it will bias the implementation toward the wrong shape:

- too many allocations
- too much per-row dispatch overhead
- loss of Parquet's natural columnar advantages

So even if Hiko eventually exposes generator-like access, the implementation should still pull data in batches internally.

## Streaming Surface

The first Parquet slice does not need streaming yet, but the next layer should be a scanner over native batches.

Proposed extension:

```sml
signature PARQUET = sig
  type table
  type schema
  type scanner
  type batch
  type row = (string * value) list

  datatype value =
      Null
    | Bool of bool
    | Int of int
    | Float of float
    | String of string
    | Bytes of bytes
    | List of value list
    | Struct of (string * value) list
    | Decimal of string
    | Date of string
    | Timestamp of string

  val read : string -> table
  val schema : table -> schema
  val columns : table -> string list
  val num_rows : table -> int
  val head : table -> int -> row list
  val show : table -> int -> string

  val scan : table -> scanner
  val next_batch : scanner -> batch option
  val batch_size : batch -> int
  val rows : batch -> row list
end
```

This gives Hiko:

- a simple inspection path now
- a scalable streaming path later
- no pressure to expose Parquet as an eager `row list`

## Generators

Yes, generator-style access is possible, but it should not be the primary implementation model.

A future row-oriented convenience layer might look like:

```sml
type row_stream
val next_row : row_stream -> (row option * row_stream)
```

or, with hidden internal mutation:

```sml
val next_row : row_stream -> row option
```

But that row stream should still be implemented on top of batch scanning, not on top of repeated single-row decoding.

So the right long-term answer is:

- **native batch scanner underneath**
- **optional generator-like row API on top**

That preserves both usability and performance.

## Likely Later Extensions

Once the basic inspection workflow exists, the next useful additions are:

- `show_schema : schema -> string`
- `select : table -> string list -> table`
- `take : table -> int -> table`
- `count : table -> int`
- `column : table -> string -> value list`
- batched or Arrow-style access for larger scans
- scanner-based streaming access

But those should come after `read`, `schema`, `head`, and `show`.

## Recommended First Slice

The first implementation should aim for:

1. `Parquet.read`
2. `Parquet.schema`
3. `Parquet.num_rows`
4. `Parquet.columns`
5. `Parquet.head`
6. `Parquet.show`

That is enough to make Parquet observable and useful in Hiko without overdesigning the data model.
