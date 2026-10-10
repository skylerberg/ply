# 15. Derivation

Most of the code you would write by hand for a new type is the same every time:
compare two values, order them, encode one as JSON, print one. `derive` writes it
for you, and the result is an ordinary value you can pass around.

## `derive`

```ply
import std.json

pub type Line = { sku: String, qty: Int, unit_price: Decimal }
derive json for Line
```

That one line produces a function `line_json` of type `std.json.JsonCodec<Line>`,
which is a record pairing an encoder with a decoder:

```ply
test "a derived json codec round-trips" {
  let c = line_json();
  let text = json::encode_string({ sku: "a", qty: 2, unit_price: 1.50m }, c);
  assert_eq(json::decode_string(text, c), Ok({ sku: "a", qty: 2, unit_price: 1.50m }))
}
```

A codec is a plain value, so it composes: `json::decode_bytes(body, order_json())`.
The encoding is canonical — a record's fields are written in sorted order — so the
text is stable:

```console
$ ply run
   "{\"qty\":2,\"sku\":\"a\",\"unit_price\":1.50}"
```

`Decimal` keeps its scale through the round trip, which is why JSON numbers are
`Decimal` rather than `Float`.

## The derivers

There are seven, and no others (`E0207`):

| deriver | generates | type |
| --- | --- | --- |
| `json` | `<snake_case(T)>_json` | `std.json.JsonCodec<T>` |
| `eq` | `<snake_case(T)>_eq` | `{eq: (T, T) -> Bool}` |
| `ord` | `<snake_case(T)>_ord` | `{compare: (T, T) -> Ordering}` |
| `bin` | `<snake_case(T)>_bin` | `std.bin.BinCodec<T>` |
| `show` | `<snake_case(T)>_show` | `{show: (T) -> String}` |
| `hash` | `<snake_case(T)>_hash` | `{hash: (T) -> Bytes}` |
| `row` | `<snake_case(T)>_row` | `std.sql.RowCodec<T>` |

The name is `std.text`'s `snake_case`, so a collision (`HTTPRequest` and
`HttpRequest` both give `http_request`) is `E0105`. A `derive` must be in the
module declaring its type:

```text
Error[E0208]: `T` is not a type this module declares
  --> f.ply:2:15
   | derive eq for T
   |               ^ declared in another module, or not at all
   = a `derive` may only name a type its own module declares, so that one type has one canonical encoding rather than one per module that thought of it
```

```ply
pub type Point = { x: Int, y: Int }
derive eq for Point
derive ord for Point

test "derived dictionaries" {
  assert(point_eq().eq({ x: 1, y: 2 }, { x: 1, y: 2 }));
  assert_eq(point_ord().compare({ x: 1, y: 2 }, { x: 1, y: 3 }), Less)
}
```

`json` and `bin` need their module imported (`import std.json`, `import std.bin`)
or the derivation is `E0206`; `show` imports `std.show` itself.

## Parameterized types

A parameterized type's function takes one dictionary per parameter:

```ply
pub type Box<a> = { label: String, inner: a }
derive json for Box
// box_json : <a>(JsonCodec<a>) -> JsonCodec<Box<a>>
```

```ply
test "a parameterized codec takes a dictionary per parameter" {
  let c = box_json(json::int_json());
  assert_eq(
    json::decode_string(json::encode_string({ label: "n", inner: 1 }, c), c),
    Ok({ label: "n", inner: 1 }),
  )
}
```

The parameter is used through a `where derivable(json, a)` constraint
(chapter 7), so a `Box<Float>` needs a codec for `Float` and gets one.

A `new` record derives as an alias of its fields does: the same documents and the
same shape.

## What cannot be derived

`E0206` names the field that blocks a derivation:

| blocked | derivers |
| --- | --- |
| function types, `Cell`, `Task`, `Chan` | all |
| `Float` | `ord`, `hash` |
| `Secret` | `json`, `ord`, `bin`, `show`, `hash` |
| `Option<Unit>`, `Option<Option<a>>` | `json` |
| a type that binds a resource label, or a field given one | all |

```text
Error[E0206]: `ord` cannot be derived for `Reading`
  --> e.ply:1:18
   | type Reading = { value: Float }
   |                  ^^^^^^^^^^^^ `Float` is not ordered: `NaN != NaN`, so `Float` has no derivation
   = `derive ord for Reading` requires every field to be derivable
   = remove the field from `Reading`, or write the dictionary by hand
```

The reason is always the same shape: a derivation asks the field's type for its
own dictionary, and this type has none to give. A `Float` has no total order
because `NaN != NaN`; a `Secret` has no `json` because encoding one would defeat
it; a label exists only in a function's row.

A type that states a `key` derives `eq`, `ord` and `hash` from what the key
answers, and one that states a `show` derives `show` from that function. So the
usual fix for an un-derivable field is to give the field's own type a `key`, a
`show` or a `gen` (chapter 4), or to write the codec by hand.

The dictionary names are ordinary definitions, so `==`, `compare`, `digest` and
`show` do not use them unless you pass them explicitly; they are for APIs that
take a dictionary as a parameter.

## `row`: a record as a SQL row

`row` writes a codec for a record's row in a SQL table, one column per field,
read and written in declaration order:

```ply
import std.sql
import std.json

pub type Line = { sku: String, qty: Int }
pub type Order = { id: Int, customer: String, note: Option<String>, lines: List<Line> }

derive json for Line
derive row for Order
// order_row : () -> sql::RowCodec<Order>
```

A `String`, `Int`, `Bool`, `Float`, `Decimal` or `Bytes` field is the column of
that type; an `Option` one that may be null; a `List` of those an array; a
`json::Json` the document it holds; and any other field a `jsonb` document read
and written through its type's own `json` codec, which the module must import
`std.json` for and derive. The derivation needs `import std.sql`. A sum, a type
with parameters and an empty record have no row, and `row` is not a constraint a
`where` can name.

The codec's `fields` are what `std.db`'s `fetch` holds a statement's columns to
(chapter 18), so a `derive row` is also what makes a query's result type checked
against its `select` list.

## Recursion

A `json` or `bin` codec whose type reaches itself, directly or through the other
types the module derives, is two definitions: `<T>_json()` starts
`<T>_json_at(json::max_depth())`, and each level hands the next one less, so the
recursion is seen to end. Past the bound a decode is an error and an encode
panics. `json`'s bound is 128 levels, as deep as `parse` reads, and `bin`'s is
10,000, as deep as calls nest.

## Summary

- `derive D for T` writes a dictionary value, named `<snake_case(T)>_D`, in the
  module that declares `T`.
- Seven derivers: `json`, `eq`, `ord`, `bin`, `show`, `hash`, `row`.
- A parameterized type's dictionary takes one dictionary per parameter, used
  through `derivable`.
- `E0206` names the field that blocks a derivation; `E0208` is a `derive` in the
  wrong module. A field's type needs its own `key`, `show` or `gen` (or a
  hand-written codec).
- `row` maps a record to a SQL row's columns, and its `fields` check `fetch`.

Next: turning one file into a package other packages can depend on.
