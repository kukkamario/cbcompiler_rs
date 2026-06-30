# CoolBasic Syntax Reference

This document is the **authoritative reference for the CoolBasic dialect this compiler implements**.
When the lexer, parser, or semantic analyzer needs to know how a construct should behave, the answer goes here first — then the implementation follows.

## 1. Lexical structure

### 1.1 Source encoding and lines

Files are UTF-8. A UTF-8 BOM at the start of a file is permitted and discarded. Both `\n` (LF) and `\r\n` (CRLF) line endings are accepted; a bare `\r` is treated as a line ending too.

A line ending terminates the current statement. To continue a statement across lines, end the line with `\`. Any spaces or tabs are allowed between `\` and the line ending.

```cb
total = a + b + c + \
        d + e + f
```

A line that contains only whitespace and/or a comment does not terminate a statement (i.e. an empty/comment-only line is not a statement).

### 1.2 Whitespace and comments

Space and tab characters are whitespace.

**Inline comments** start with `'`, `REM`, or `//` and run to end of line:

```cb
x = 1 // assign one
y = 2 ' assign two
REM this is a comment
```

**Block comments** are delimited by `/*` and `*/` and nest: every `/*` must be matched by a `*/` before the comment is considered closed.

```cb
/* outer /* inner */ still in outer */ code resumes here
not comment /* comment /* comment */ comment */ not comment
```

Unterminated block comments are a compile error.

### 1.3 Identifiers

An identifier follows the Unicode UAX #31 definition (the same character set Rust accepts):

- The first character must be `XID_Start` (which includes ASCII letters and most Unicode letters) or `_`.
- Subsequent characters must be `XID_Continue` (letters, digits, and combining marks) or `_`.

Identifiers are **case-insensitive**, compared using Unicode simple case folding. `myVar`, `MYVAR`, and `MyVar` all refer to the same name.

An identifier that exactly matches a keyword (§1.5) is reserved; identifiers can also carry a trailing type sigil (§1.4), in which case the sigil is **not** part of the name (`x%` and `x#` both refer to `x`, but their declared type must match — see §1.4).

```cb
foo            // valid
_total         // valid
résumé2        // valid (non-ASCII letters allowed)
2cool          // ERROR: must not start with a digit
my-var         // ERROR: '-' is not an identifier character
```

### 1.4 Type sigils

Type sigils annotate an identifier with a built-in type at its declaration or first reference:

| Sigil | Type    |
| ----- | ------- |
| `%`   | Integer |
| `#`   | Float   |
| `$`   | String  |

Sigils exist **only** for these three common types. `Byte`, `Short`, `Long`, user-defined types, and arrays must use `As` syntax. `!` is a **reserved** symbol with no current meaning (formerly the `Bool` sigil); using it as a sigil or operator is a compile error.

A variable's type is fixed at its first reference and cannot change later. If a sigil and an `As Type` clause are both given, they must agree:

```cb
Dim count% As Integer    // OK: sigil and As agree
Dim count% As Float      // ERROR: % is Integer, As says Float
```

Once `x%` is declared, `x` and `x%` refer to the same variable; writing `x#` later is an error.

### 1.5 Keywords

Reserved words. Case-insensitive like all identifiers.

```
And, As
BinAnd, BinNot, BinOr, BinXor, Bool, Boolean, Break, Byte
Case, Const, Continue
Default, Dim
Each, Else, ElseIf, End, EndFunction, EndIf, EndSelect, EndStruct, EndType
False, Field, Float, For, Forever, Function
Global, Goto
If, Include, Int, Integer
Long
Mod
New, Next, Not, Null
Or
Redim, REM, Repeat, Return
Sar, Select, Shl, Short, Shr, Step, String, Struct
Then, To, True, Type
UInt, UInteger, ULong, Until
Wend, While
Xor
```

`Bool`, `Boolean`, `UInt`, `UInteger`, and `ULong` are **reserved but unsupported** type names — invalid in a type position (§3.1). They stay reserved so the names remain free for future use and so legacy code using them fails with a clear diagnostic instead of silently parsing them as identifiers.

Any `End <Keyword>` pair may also be written as a single token: `End If` == `EndIf`, `End Function` == `EndFunction`, etc. Parsers matching a block closer accept both forms interchangeably.
`ElseIf` can also be written as `Else If`.

`REM` is recognised as the start of a line comment (§1.2) rather than as a value-producing keyword.

### 1.5.1 Runtime command names

Names provided by the runtime library (commands such as `Box`, `Text`,
`LoadImage`; see [`cb_runtime.md`](cb_runtime.md)) are **not** keywords. They are
ordinary identifiers seeded into the global scope, and shadowing rules apply by
*kind*:

- A runtime **command** (a runtime function or overload set) may be **shadowed
  by an explicit declaration** of the same name. `Dim box As Int` — or
  `Global box`, or a user `Function box(...)` — reclaims the name: from that
  declaration onward `box` refers to the user's variable/function, not the
  command.
- An **implicit** declaration may **not** shadow a command. A bare assignment to
  a never-declared command name, e.g. `box = 5`, is an error (`E0328`); the
  diagnostic points the user to declare it explicitly with `Dim` if they intend
  to shadow it.
- Runtime-defined **constants** (e.g. `On`, `Off`, `Pi`, key scancodes) and
  runtime **types** (opaque handle types) are **reserved**: any colliding user
  declaration is an error (`E0303`), whether explicit or implicit.

Case-insensitivity applies throughout (§1.3): `Box`, `box`, and `BOX` are the
same name.

### 1.6 Literals

#### Integer literals

```
0           // decimal
12304       // decimal
5_342_100   // underscores allowed as digit separators
$2f4E4      // hexadecimal (classic CB syntax)
$dead_beef  // hexadecimal, underscores allowed
%1010       // binary
```

Hexadecimal digits and the `b` in `0b` are case-insensitive. Underscores may appear between digits but not adjacent to the prefix (`$_ff`, `0b_10`, `1__000` are all errors). An integer literal that overflows the inferred type is a compile error (see §3.4 for inference).

`%` introduces a binary literal only at the start of a number; after an identifier, `%` is the Integer sigil (§1.4). The lexer disambiguates by context: `x%` is `x` with the Integer sigil; `%10` is the binary literal `2`.

#### Float literals

```
0.23
23.205421
12.4e23
1.0e-7
6.022e+23
1_000.5      // underscores allowed
```

A float literal must contain a decimal point or an exponent (or both). A leading or trailing dot is **not** allowed: write `0.5` and `5.0`, not `.5` or `5.`. The exponent letter `e` is case-insensitive; the exponent itself is a decimal integer with an optional `+`/`-` sign.

#### String literals

Single-line strings use `"…"` and are **verbatim**: a backslash is an ordinary
character and the first unescaped `"` always closes the literal. There is no
escape processing, so a Windows path writes naturally:

```cb
path$ = "C:\new"     // the six characters C : \ n e w — no newline
```

Because `"` always closes a verbatim string, a literal double quote inside a
string needs either the escaped form `$"\""` or a raw `"""…"""` string.

**Escaped strings** use the `$"…"` form, which processes C-style escapes. The
`$` is a **mode marker only — it is not interpolation**; nothing inside the
string is evaluated. The escape set is:

| Escape   | Meaning                          |
| -------- | -------------------------------- |
| `\\`     | backslash                        |
| `\"`     | double quote                     |
| `\n`     | newline (LF)                     |
| `\r`     | carriage return                  |
| `\t`     | tab                              |
| `\0`     | null character                   |
| `\xNN`   | code point U+0000–U+00FF (2 hex digits) |
| `\uNNNN` | Unicode code point (4 hex digits)|

```cb
msg$ = $"line one\nline two"   // an actual LF between the two halves
q$   = $"she said \"hi\""      // embeds two literal double quotes
```

> **String model — Unicode code points, not bytes.** CoolBasic-rs strings are
> sequences of **Unicode code points**: from CB code's point of view a string
> behaves like a UTF-32 array (one element per code point), while the
> implementation stores it as **valid UTF-8** internally. `Len`, `Mid`, `Left`,
> `Right`, `Asc`/`Chr`, indexing, and every other string operation count and
> address **code points**, never bytes. Consequently `\xNN` denotes the *code
> point* U+00NN (e.g. `\xFF` is U+00FF, "ÿ"), exactly like `\u00NN` — it is **not**
> a raw byte. This is a deliberate divergence from the original CoolBasic
> runtime, whose strings were 8-bit byte strings (there `Len(Chr(255))` was 1;
> here it is also 1, but the single element is the code point U+00FF, and the
> string serialises to two UTF-8 bytes). Choosing Unicode semantics up front
> avoids byte-vs-character ambiguity across the whole language and runtime.

A literal newline inside a single-line string (`"…"` or `$"…"`) is a compile error — use `$"\n"` for an escaped newline, or a multi-line `"""…"""` string.

**Multi-line strings** use triple double-quotes `"""…"""` and are **raw**: no escape processing, no interpolation. The closing delimiter must appear on its own line, and the common leading whitespace of the content lines is stripped:

```cb
msg$ = """
    Line one with a literal \n (not an escape)
    Line two with "quotes" inside
    """
// msg = "Line one with a literal \n (not an escape)\nLine two with \"quotes\" inside\n"
```

If a line in the content block has less indentation than the closing `"""`, that is a compile error.

#### Integer truth literals and Null

```
True      // the Integer 1
False     // the Integer 0
Null      // valid value for any reference-typed variable (arrays, Type, Function pointers)
```

There is no `Bool` type — `True` and `False` are simply the `Integer` constants `1` and `0` (§3.1).

### 1.7 Operators and punctuation

Arithmetic (binary): `+`, `-`, `*`, `/`, `^`, `Mod`
- `/` is division: **integer division when both operands are integers**, and floating-point division when either operand is `Float` (which promotes both to `Float`). There is no separate integer-division operator — `\` is the `Type` field accessor (see the postfix/access list below and §3.3), not arithmetic.
- `^` is exponentiation, right-associative. It **always yields `Float`** (operands are promoted to `Float` and the result type is `Float`), so `2 ^ 10` is `1024.0`, not an `Integer`. Wrap with `Int(...)` if an integer result is wanted.
- `Mod` is signed remainder; the sign of the result matches the dividend.

Bitwise (binary): `BinAnd`, `BinOr`, `BinXor`, `Shl`, `Shr`, `Sar`
- `Shl`/`Shr`/`Sar` shift by an integer count. `Shr` is logical right shift, `Sar` is arithmetic shift.

Comparison: `=`, `<>`, `<`, `>`, `<=`, `>=`
- Defined for all numeric types, `String` (lexicographic by Unicode code point), and reference types (`=`/`<>` only, by identity). A comparison yields `Integer` `1` (true) or `0` (false); there is no `Bool` type.

Logical: `And`, `Or`, `Xor`, `Not`
- `And` and `Or` short-circuit (§5.2). `Xor` evaluates both operands. Operands are tested as `<> 0`; the result is `Integer` `1`/`0`.

Unary: `+`, `-`, `Not`, `BinNot`
- Unary `+` is **absolute value**, identical to the `Abs` function: `+x` ≡ `Abs(x)` (e.g. `+(-5)` is `5`). It is **not** a no-op. The result keeps the operand's numeric type.

String concatenation uses `+`. Numeric operands on either side of a string `+` are implicitly converted to String (§3.4).

Postfix/access: `()` (call), `[]` (index), `\` and `.` (`Type` field access — interchangeable; `\` is the legacy CoolBasic form, `.` the dotted alias). See §3.3.

Assignment is `=` (§6.1). Statements are separated by the line ending (§1.1) or by `:`; a single line may chain multiple statements with `:`:

```cb
x = 1 : y = 2 : Print x + y
```

`:` after an identifier on a line by itself (`name:`) is a label, not a separator — see §6.4.

## 2. Program structure

### 2.1 Top-level form

All statements may appear at top level. Execution begins at the first statement of the main file passed to the compiler. Function and Type/Struct definitions placed at top level are hoisted: they are visible everywhere in the program regardless of textual position (§7.3).

### 2.2 Includes

`Include "path"` brings another source file into the program. It is a **top-level-only** statement; `Include` inside a function, block, or loop is a compile error.

- Paths are resolved **relative to the file doing the include**. Absolute paths are also accepted.
- Each file is included **at most once** per compilation. Repeated includes (direct or cyclic) are silently ignored after the first.

```cb
// main.cb
Include "utils.cb"
Include "graphics/sprite.cb"
```

### 2.3 Entry point

Implicit. Compilation starts from the main file given on the command line; execution starts at that file's first statement. There is no `Main` function.

## 3. Types

### 3.1 Primitive types

| Type             | Width        | Signedness | Sigil |
| ---------------- | ------------ | ---------- | ----- |
| `Byte`           | 8-bit        | unsigned   | —     |
| `Short`          | 16-bit       | unsigned   | —     |
| `Int`/`Integer`  | 32-bit       | signed     | `%`   |
| `Long`           | 64-bit       | signed     | —     |
| `Float`          | 64-bit IEEE  | —          | `#`   |
| `String`         | UTF-8 string | —          | `$`   |

`Int` and `Integer` are exact aliases. `Byte` and `Short` are **storage-only**: they widen to `Int` for all arithmetic, and a value is narrowed back only when stored into a `Byte`/`Short` location (§3.4). There is **no 32-bit float type** — `Float` is always 64-bit. A future `Single` type can be added without changing this.

There is **no `Bool` type**: comparison and logical operators yield `Integer` `1`/`0`, and `True`/`False` are the `Integer` constants `1`/`0` (§1.6). The names `Bool`, `Boolean`, `UInt`, `UInteger`, and `ULong` are **reserved but unsupported** — using any of them in a type position is a compile error (§1.5).

### 3.2 Arrays

Arrays are reference-typed and 0-indexed. Their element type goes before `[]`; one comma per additional dimension:

```cb
Dim a As Integer[]       // 1-D, currently 0 elements (Null-like)
Dim b As Float[,]        // 2-D, currently 0 elements
Dim c As String[,,]      // 3-D, currently 0 elements

a = New Integer[10]      // 1-D, length 10, indices 0..9
b = New Float[4, 8]      // 2-D, 4*8 = 32 elements
```

A newly declared array has **length 0** until assigned. A `New T[…]` array's elements are zero-initialised (numerics = 0, String = "", reference types = Null).

Indexing uses one bracketed expression with comma-separated indices per dimension:

```cb
a[3] = 42
b[i, j] = 1.5
```

An index outside the valid range traps at runtime (§9.2).

Assignment between array variables copies the **reference**, not the contents:

```cb
Dim arr As Float[] = New Float[10]
Dim arr2 As Float[]
arr2 = arr       // arr2 now references the same array as arr
arr2[0] = 1.0    // arr[0] is now 1.0 as well
```

To resize an array variable (replacing whatever it referenced), use `Redim`:

```cb
Redim arr2 As Float[100]   // arr2 now references a fresh 100-element array
```

`Redim` replaces the variable with a **fresh, zero-initialised** array — it does **not** preserve the previous contents. (This matches the interpreter, the differential-test oracle; an earlier note here claimed Redim preserves, which was wrong.)

Functions taking or returning arrays do not pin a length; the size is part of the runtime value:

```cb
Function sum#(arr As Float[]) As Float
    Dim total As Float = 0.0
    For i = 0 To Len(arr) - 1
        total = total + arr[i]
    Next i
    Return total
EndFunction
```

`Len(arr)` returns the length of dimension 0; `Len(arr, n)` returns the length of dimension `n` (0-indexed). These are compiler intrinsics.

### 3.3 User-defined types

There are two user-defined types, with different memory models:

#### `Type … EndType` — heap-allocated linked-list node

A `Type` defines a record that is always allocated on the heap, accessed through a reference, and **automatically threaded into a global linked list** of all live instances of that type. Type variables are reference-typed; assigning between them copies the reference, not the fields. The literal `Null` is a valid value of any `Type`.

```cb
Type MyType
    Field field1 As Integer
    Field asd$
    Field something As MyType
EndType

Dim var As MyType = New MyType    // appended to the MyType list
var.field1 = 23
var.asd = "hello"

Dim second As MyType = New MyType  // also appended; comes after var
```

**Field access uses `\` or `.`**, which are fully interchangeable: `\` is the original CoolBasic accessor (`var\field1`) and `.` is an accepted alias (`var.field1`). Both bind as left-associative postfix operators (§5.1) and may be mixed freely in a chain — `a\b.c\d` and `a.b.c.d` parse identically. The examples in this document use `.` for readability.

Each `Field` declares exactly one name; comma-separated forms like `Field x, y As Integer` are not accepted — write one `Field` line per name. The same rule applies to `Field` declarations inside `Struct` (below).

**Built-ins for `Type` linked lists:**

| Built-in            | Returns                                       |
| ------------------- | --------------------------------------------- |
| `First(MyType)`     | First live instance, or `Null` if list empty  |
| `Last(MyType)`      | Last live instance, or `Null` if list empty   |
| `Next(node)`        | Next instance, or `Null` at the end           |
| `Previous(node)`    | Previous instance, or `Null` at the start     |
| `Delete node`       | Removes `node` from its list and frees it     |

The name-vs-instance distinction is load-bearing for the compiler. `First`/`Last`, `New`, and `Each` (below) take a **`Type` name** — always written as a plain identifier, never a more complex expression. `Next`/`Previous`/`Delete` instead take a **`Type` instance value** (typically a variable). A bare `Type` name is therefore legal *only* in those name positions; used anywhere else as a value (`Return MyType`, `a = MyType`, `Print MyType`) it is a compile error (sema `E0311`).

Iteration with `For Each` (§6.3) is the idiomatic loop:

```cb
For n = Each MyType
    Print n.field1
Next n
```

Two `Type` references compare by **identity** (same node = equal); fields are not compared:

```cb
If var = second Then
    // True only if var and second reference the same node
EndIf
```

#### `Delete` semantics

`Delete` removes a node from its `Type`'s linked list and frees it. The exact behaviour depends on whether the operand is a plain **variable** (an lvalue delete, with rewind) or any other expression — including a field access (`n.link`) or an array element (`arr[0]`) — which is an **rvalue** delete (free only, no rewind). Only a bare variable has a slot the rewind/mark step can update; a field or element operand is treated exactly like `Delete First(MyType)` (see the rvalue case below).

**`Delete v` where `v` is an lvalue:**

1. If `v` is `Null` or `v` is already in the deleted state (defined below), trap (§9.2).
2. Capture `prev = v.prev` — the node immediately before `v` in the list, or the internal list-head sentinel if `v` was first.
3. Unlink the node from the list, release its fields, and free its memory.
4. Reassign `v` to `prev` and mark `v`'s variable slot as **deleted**.

The deleted mark on a variable slot:

- is cleared by any subsequent assignment to that variable (`v = Next(v)`, `v = someOther`, `v = Null`, …);
- causes any field access through `v` (e.g. `v.field1`) to trap (§9.2);
- causes a second `Delete v` to trap as double-delete (§9.2);
- is **transparent** to `Next(v)` and `Previous(v)`: they walk from `v`'s underlying pointer — which is now the previous node (or the sentinel) — so `Next(v)` returns the live node that *was* after the deleted one (or `Null` if there was none), and `Previous(v)` returns the live node before that.

**`Delete e` where `e` is an rvalue expression** (e.g. `Delete First(MyType)`, `Delete n.something`):

Steps 1 and 3 only; no rewind, no mark (there is no variable slot to update). Any CB variable or field still holding the freed reference now dangles — see "Aliasing and dangling references" below.

**The head sentinel.** Each `Type` maintains an internal head sentinel so that deleting the *first* real node has a well-defined rewind target. The sentinel is never returned to CB code: `First(MyType)` returns its successor (or `Null` for an empty list), and `Previous` on the first real node returns `Null`.

**`Next(Null)` and `Previous(Null)`** both return `Null`. This makes `While n <> Null … Wend` walk the list to completion without an explicit guard.

**`For Each` desugaring.** A `For Each` loop over a `Type` is exactly:

```cb
n = First(MyType)
While n <> Null
    // body — may Delete n
    n = Next(n)           // if Delete rewound n, Next yields the post-deleted-position node
Wend
```

No special compiler support for `Delete` inside `For Each` is needed — the lvalue-rewind rule makes the loop pattern work uniformly.

**Worked examples.**

```cb
// Canonical loop pattern — works:
For n = Each MyType
    If n.dead Then Delete n
Next n

// Delete outside a loop — Next on the same variable is still defined:
Dim x As MyType = First(MyType)
Delete x
Dim after As MyType = Next(x)     // = the node that was after x, or Null

// Field access after Delete on the same variable — traps:
Dim y As MyType = First(MyType)
Delete y
Print y.field1                    // RUNTIME TRAP (variable in deleted state)

// Double-delete via the same variable — traps:
Dim z As MyType = First(MyType)
Delete z
Delete z                          // RUNTIME TRAP

// Delete on an rvalue — no rewind possible:
Delete First(MyType)              // first real node freed; any aliases dangle
```

**Aliasing and dangling references.**

Only the variable named in `Delete v` is rewound and marked. Other variables, parameters, or fields that held the same reference still hold the *freed* pointer:

```cb
Dim a As MyType = First(MyType)
Dim b As MyType = a               // b aliases a's node
Delete a                          // a is rewound and marked; b is untouched
Print b.field1                    // UNDEFINED — b is dangling

Function kill(t As MyType)
    Delete t                      // rewinds t (this function's local)
EndFunction
Dim n As MyType = First(MyType)
kill(n)                           // caller's n is NOT rewound
Print Next(n)                     // UNDEFINED — caller's n is dangling
```

The language treats these reads as undefined behaviour. The interpreter backend (`cb-backend-interp`), as the reference implementation, traps on use of a known-freed reference with a clear diagnostic. The LLVM backend may exhibit any behaviour here, especially under optimisation.

#### `Struct … EndStruct` — statically allocated value type

A `Struct` is a value type with a static layout, similar to a C struct. It is **copied** on assignment and on parameter passing. Structs are not threaded into any list and have no `New`/`Delete`; declaring one allocates it in place.

```cb
Struct Vec2
    Field x As Float
    Field y As Float
EndStruct

Dim p As Vec2          // zero-initialised in place: {0.0, 0.0}
p.x = 1.5
p.y = -3.0

Dim q As Vec2 = p      // full copy; mutating q does not affect p
q.x = 99.0
Print p.x              // 1.5
```

Structs may contain other Structs by value, arrays by reference, and Type references. A Struct cannot contain itself by value (that would be infinite size), but it can contain a `Type` reference to itself.

### 3.4 Type conversions

#### Implicit conversions

- `Byte`/`Short` are **storage-only**: an operand of either widens to `Int` before any arithmetic, bitwise, shift, or comparison. The integer result is `Long` if an operand is `Long`, otherwise `Int`; a value narrows back to `Byte`/`Short` only on assignment to such a variable.
- Numeric widening between integer types: always implicit (`Byte` → `Short` → `Int` → `Long`).
- **An integer literal assigned/coerced to a narrower integer type is range-checked at compile time.** If the literal value does not fit the target type's range it is a hard error (e.g. `Dim b As Byte : b = 300`, or `b = -1`). An in-range literal converts silently — because the value is a known-safe constant, it does *not* produce the narrowing warning that a runtime value would.
- Integer → Float: implicit.
- Float → Integer: implicit, **rounds to the nearest integer with ties away from zero** — matching the `Int()` runtime function, **not** a straight truncation toward zero (so `10.5 → 11`, `-1.5 → -2`, `-2.5 → -3`; see `cb_runtime.md` §Math). Both implicit conversions and explicit `Int(x)` use this same rule. The compiler emits a **narrowing-conversion warning** at the implicit conversion site; suppress by writing `Int(x)` explicitly.
- Long → Int, Int → Byte/Short, etc.: implicit but **warned** as a narrowing conversion.
- Any numeric → String: implicit when used as a `+` operand on a String. Float → String uses the shortest decimal representation that round-trips, switching to scientific notation outside a sensible range. Integer → String uses decimal.
- Comparison and logical operators yield `Integer` `1`/`0`; any numeric is truthy when `<> 0` (used by `If`/`While`/`Until` conditions). There is no `Bool` type to convert to or from.

`Null` is implicitly assignable to any reference type.

#### Explicit conversions

The compiler-intrinsic conversion functions are:

```cb
Int(val)         // to Integer (32-bit signed)
Float(val)       // to Float
Str(val)         // to String
```

`Int("123")` returns 123. `Float("1.5e2")` returns 150.0. A `Str`-to-numeric conversion that fails to parse returns 0 (or 0.0) — it does not throw. To distinguish "0" from "parse failed", parse and check explicitly via runtime-library helpers.

### 3.5 Runtime-defined opaque types

Runtime libraries can define **opaque handle types** — named types whose internal representation is hidden from user code. These are used for external resources such as files, images, sounds, or network connections.

```cb
Dim img As Image           // declared like any named type
img = LoadImage("hero.png") // created by a runtime function
DrawImage(img, 100, 200)   // passed to runtime functions
```

Opaque type variables default to `Null` and behave like references in that respect:

```cb
Dim snd As Sound           // defaults to Null
If snd = Null Then
    Print "no sound loaded"
EndIf
```

**Allowed operations:**

| Operation | Example | Notes |
|-----------|---------|-------|
| Assignment | `img = LoadImage("x.png")` | From runtime function return or another variable |
| Null comparison | `img = Null`, `img <> Null` | Equality/inequality only |
| Identity comparison | `img1 = img2` | Same opaque type only; checks if both refer to the same handle |
| Pass to runtime function | `DrawImage(img, 0, 0)` | Type-checked against the function signature |

**Disallowed operations** (compile error):

- Arithmetic (`img + 1`, `img - img2`)
- Ordering (`img < img2`, `img > img2`)
- Bitwise, logical, unary operators
- Field access (`img.width` — unless the runtime provides a function like `GetImageWidth(img)`)
- `New`, `Delete`
- `First`, `Last`, `Next`, `Previous` (these are for `Type` linked lists only)
- Implicit conversion to or from any other type

Opaque types are distinct from user-defined `Type … EndType` records: they have no fields, no linked-list threading, and cannot be constructed with `New`. The runtime library is solely responsible for creating and destroying handles.

## 4. Variables and scope

### 4.1 Declaration

**Explicit declaration with `Dim`:**

```cb
Dim x As Integer
Dim name$ As String           // sigil + As must agree
Dim total# = 0.0              // explicit type via sigil, with initialiser
Dim point As Vec2             // value-type Struct, zero-initialised in place
Dim node As MyType = Null     // reference-type Type, starts Null
```

**Multi-name declarations.** `Dim` accepts a comma-separated name list that shares one trailing `As Type` clause:

```cb
Dim a, b, c As Integer    // three Integer variables, all of type Integer
```

All names share the declared type. The multi-name form does not accept an initialiser — use single-name `Dim` if you need one (`Dim x As Integer = 0`). `Global` (§4.3) accepts the same form; `Field` (§3.3) and `Const` (§4.4) do not.

**Implicit declaration** at the first assignment:

```cb
x = 213                       // x is Integer (inferred from the Int value)
obj = LoadObject("hero.png")  // obj is Object (inferred from the value)
p# = 23.04                    // p is Float (via sigil)
z As String = "asd"           // z is String (via As, no sigil needed)
```

If the first reference has neither a sigil nor an `As` clause, the variable's type is **inferred from the assigned value**: `x = 213` makes `x` an `Integer`, `x = 3.14` makes it a `Float`, `s = "hi"` a `String`, and `obj = LoadObject(...)` an `Object`. A sigil or `As` clause still pins the type explicitly (and the value is coerced to it).

The value must have a concrete type for inference to succeed:

- Assigning `Null` is an error (**E0331**) — `Null` has no type of its own; declare the variable explicitly with `As` (e.g. `Dim node As MyType = Null`).
- A self-referential first assignment such as `x = x + 1` before any declaration of `x` is a use-before-declaration error (**E0300**): the right-hand side reads `x` before it exists. Declare `x` first if a running total is intended.

### 4.2 Scope rules

Variables are **function-scoped**. The whole top-level forms one main scope; each `Function` body is its own scope. There is no block scoping inside `If`, `For`, `While`, `Repeat`, or `Select` — a variable introduced inside a block is visible until the end of the enclosing function (or end of file at top level).

```cb
If condition Then
    Dim temp = 10
EndIf
Print temp     // OK: temp lives until end of the enclosing scope
```

Within a function, only `Global`-declared variables from the main scope are visible; ordinary main-scope variables are not.

### 4.3 Globals

`Global` makes a variable available in every function:

```cb
Global score As Integer = 0

Function addScore(n As Integer)
    score = score + n          // visible because of Global
EndFunction
```

`Global` accepts the same multi-name form as `Dim` (§4.1):

```cb
Global a, b As Float
```

`Global` may appear only at top level.

### 4.4 Constants

`Const` introduces a name bound to a constant expression of a built-in type:

```cb
Const Pi# = 3.14159
Const MaxItems = 100
Global Const Version$ = "1.0.0"
```

The value must be set at the declaration and cannot be reassigned. Constant expressions may use literals, other constants, and the operators in §5 — they're evaluated at compile time.

`Const` is legal at top level (with or without `Global`) and inside function bodies. A function-local `Const` is scoped to its enclosing function, like an ordinary `Dim`. Each `Const` declares exactly one name; comma-list forms like `Const A = 1, B = 2` are not accepted.

## 5. Expressions

### 5.1 Precedence and associativity

Listed highest precedence (binds tightest) at the top.

| Level | Operators                              | Associativity |
| ----- | -------------------------------------- | ------------- |
|  1    | `()` `[]` `\` `.` (call, index, field) | left          |
|  2    | unary `+` `-` `Not` `BinNot`           | right         |
|  3    | `^`                                    | **right**     |
|  4    | `*` `/` `Mod`                          | left          |
|  5    | `+` `-`                            | left          |
|  6    | `Shl` `Shr` `Sar`                  | left          |
|  7    | `BinAnd`                           | left          |
|  8    | `BinXor`                           | left          |
|  9    | `BinOr`                            | left          |
| 10    | `=` `<>` `<` `>` `<=` `>=`         | left, non-chaining |
| 11    | `And`                              | left, short-circuit |
| 12    | `Xor`                              | left          |
| 13    | `Or`                               | left, short-circuit |

Examples:

```cb
2 ^ 3 ^ 2            // = 2 ^ (3 ^ 2) = 512          (^ is right-assoc)
-2 ^ 2               // = -(2 ^ 2) = -4              (unary tighter than ^)
a + b BinAnd mask    // = (a + b) BinAnd mask        (bitwise below arithmetic)
a = b And c = d      // = (a = b) And (c = d)        (comparison tighter than And)
```

Comparison operators do **not chain**: `1 < x < 10` parses as `(1 < x) < 10` — the `1 < x` yields `Integer` `0` or `1`, which is then compared to `10`, almost certainly a bug. Use `x > 1 And x < 10` instead.

### 5.2 Short-circuit evaluation

`And` and `Or` short-circuit: the right-hand operand is only evaluated when the left does not already determine the result.

```cb
If p <> Null And p.field > 0 Then        // safe: p.field never accessed when p is Null
    ...
EndIf
```

`Xor` does **not** short-circuit (it can't — both operands are needed). `Not` is a unary operator and the question doesn't arise.

### 5.3 String operations

- Concatenation with `+`. Numeric operands on either side are implicitly converted to `String`:

  ```cb
  msg$ = "Score: " + score      // score is Integer, auto-converted
  ```

- Comparison: `=`, `<>`, `<`, `>`, `<=`, `>=` compare lexicographically by Unicode code point.

- Indexing strings with `[]` is **not** part of the language; use runtime-library functions (`Mid`, `Left`, `Right`, etc.) provided by the runtime.

### 5.4 Type expressions

Several constructs take a **type expression** — a piece of syntax that names a type. Type expressions appear in:

- After `As` in `Dim`, `Global`, `Field`, `Const`, function parameters, and function return types.
- After `New` (§3.2, §3.3) to allocate an array or a `Type` node.
- After `Redim` (§3.2) to retype an array variable.
- Inside `Function(...)` parameter types and the `As ReturnType` clause of a function-pointer type (§7.4).

A type expression is one of:

- **A primitive type** — `Byte`, `Short`, `Int`/`Integer`, `Long`, `Float`, `String` (§3.1).
- **A user-defined type name** — any identifier that resolves at sema time to a `Type` or `Struct` (§3.3), e.g. `MyType`, `Vec2`.
- **An array of T with N dimensions** — the element type followed by `[]` (1-D), `[,]` (2-D), `[,,]` (3-D), and so on: `Integer[]`, `Float[,]`, `String[,,]`. The element type may itself be any non-array type expression.
- **A function-pointer type** — `Function(<param-types>)` with an optional `As <return-type>`. Parameters use the same syntax as a function declaration (§7.2): each parameter is `name As Type` or `name<sigil>`, with names optional in a type position.
- **A parenthesised type** — `(T)` is the same type as `T`. Parentheses are for grouping and disambiguation only; they have no effect on meaning. They are valid in every position a type expression is, not just after `As`.

```cb
Dim x       As Integer                                        // primitive
Dim p       As Vec2                                           // user-defined
Dim arr     As Float[,]                                       // 2-D array of Float
Dim fn      As Function(Integer, Float) As String             // function-pointer
Dim fnNamed As Function(text As String, length As Float) As String
Dim grouped As (Integer)                                      // same as As Integer
```

**Nested function-pointer types.** When a `Function(...)`'s return type is itself a function-pointer, the unparenthesised form parses **right-associatively**: each `As <type>` is consumed as the return type of the most recently opened `Function(...)`, recursively. Parentheses let you write the same type explicitly, or override the default grouping.

```cb
// Right-assoc default — outer takes Integer, returns (Function(Float) As String):
Dim fn As Function(Integer) As Function(Float) As String
// Same type, parenthesised for clarity:
Dim fn As Function(Integer) As (Function(Float) As String)

// Array of function-pointers requires parens — otherwise [] binds to the return type:
Dim handlers     As (Function(Integer) As Float)[]    // array of fn-pointers
Dim returnsArray As Function(Integer) As Float[]      // single fn-pointer returning Float[]
```

**`New` expression grammar.**

- `New T` — allocates a fresh `Type` node and threads it into `T`'s linked list (§3.3). `T` must be a user-defined `Type`.
- `New T[dim1, dim2, ...]` — allocates an array. The bracketed dimensions give each axis's size; the number of dimensions must match the array variable's declared rank. `T` is the element type (any non-array type expression).

```cb
Dim node As MyType    = New MyType
Dim a    As Float[]   = New Float[10]         // 1-D, length 10
Dim b    As Float[,]  = New Float[4, 8]       // 2-D, 4 × 8 elements
```

**`Function` keyword disambiguation.** `Function` starts a declaration at statement position (top level — §7.1) and a function-pointer **type** in a type-expression position (after `As`, or inside another `Function(...)` parameter type). Same keyword, two roles; the parser distinguishes by context.

## 6. Statements

### 6.1 Assignment

Assignment uses `=`:

```cb
x = 10
arr[i, j] = compute()
node.field = value
```

Because comparison also uses a single `=`, an expression like `a = b = c` parses as `a = (b = c)` — assigning to `a` the `Integer` (`0`/`1`) result of comparing `b` and `c`. **Assignment is not chainable**:

```cb
a = b = 5            // a := (b = 5), i.e. a is Integer (0 or 1), b is unchanged — NOT a chain
```

There are no compound-assignment operators (`+=`, `-=`, …); write the operation out:

```cb
total = total + delta
```

### 6.2 Conditionals

Block `If`:

```cb
If x > 0 Then
    Print "positive"
ElseIf x = 0 Then
    Print "zero"
Else
    Print "negative"
EndIf
```

Single-line `If`:

```cb
If x > 0 Then Print "positive"
If ready Then start() Else stop()      // single-line Else is allowed
```

**Single-line vs. block disambiguation.** After consuming `Then`, the parser peeks one token: if it is a line ending, the `If` is the block form (closed with `EndIf`); otherwise the next token starts a single-line `If` statement.

```cb
If x > 0 Then              // newline follows Then → block form
    Print "positive"
EndIf

If x > 0 Then Print "positive"        // statement follows Then → single-line
```

The `Then` and `Else` branches of a single-line `If` may chain multiple statements with `:` (§1.7):

```cb
If x > 0 Then a = 1 : b = 2 Else c = 3 : d = 4
```

A single-line `If` ends at the end of the line; it cannot contain `ElseIf` and cannot span multiple lines.

#### `Select`

`Select` supports all built-in types. Every `Case` value must be a constant expression and implicitly convertible to the type of the `Select` value. Cases **do not fall through** by default; use `Continue` inside a case body to fall through to the next case.

```cb
Select val
    Case 10
        Print "ten"

    Case 30
        Print "thirty"
        Continue              // explicit fall-through into Case 40
    Case 40
        Print "thirty or forty"

    Default
        Print "something else"
EndSelect
```

`Default` is optional and may appear in any position; when present it matches when no `Case` does.

### 6.3 Loops

#### Forever loop

Runs until `Break` or `Return` exits it.

```cb
Repeat
    line$ = readLine()
    If line = "" Then Break
    process(line)
Forever
```

`Continue` jumps to the top of the body.

#### Repeat-While (condition at the end)

```cb
Repeat
    work()
While moreToDo()              // condition checked here; Continue jumps to this check
```

The body always runs at least once. `Continue` jumps to the condition check.

#### Repeat-Until (condition at the end, inverted)

```cb
Repeat
    work()
Until done()                 // condition checked here; Continue jumps to this check
```

The post-test dual of `Repeat … While`: the body always runs at least once, then the loop continues *while the condition is falsy* and exits the moment it becomes truthy. `Continue` jumps to the condition check. There is no pre-test `Until` form — use `While … Wend` for that.

#### While-Wend (condition at the start)

```cb
While moreToDo()              // condition checked here; Continue jumps to this check
    work()
Wend
```

The body may run zero times. `Continue` jumps to the condition check.

#### Iterative For

Inclusive on both ends. `Step` is optional and defaults to `1`. If `Step` is positive the loop runs while `i <= To`; if negative, while `i >= To`. `Step` may be a Float when `i` is a Float variable.

```cb
For i = 0 To 10 Step 2
    Print i              // 0, 2, 4, 6, 8, 10
Next i

For i = 10 To 0 Step -1
    Print i
Next
```

The variable name after `Next` is optional, but if given it must match the loop variable. `Continue` jumps to the end-of-iteration step (increment + condition check).

An **implicitly declared** loop variable (no prior `Dim`, no sigil) takes its type from the bounds, following the value-inference rule of §4.1: the type is the numeric promotion of `From`/`To`/`Step` (floored at `Integer`, so `Byte`/`Short` bounds give an `Integer` variable). Thus `For i = 1 To 10` makes `i` an `Integer`, while `For i = 0.0 To 1.0 Step 0.1` makes it a `Float`. A sigil (`For i# = ...`) or a prior `Dim` pins the type instead.

#### For Each (over an array or Type list)

```cb
For val = Each scores         // scores is Float[] or Float[,] etc.
    Print val
Next val

For node = Each MyType        // iterates the global MyType linked list
    Print node.field1
Next node
```

The parser distinguishes iterative `For` from `For Each` by the token immediately after `=`: if it is the `Each` keyword, this is a For-Each loop; otherwise it is an iterative `For/To/Step` loop.

`Continue` advances to the next element — the next array slot, or `Next(<var>)` in the `Type`-list desugar.

Multi-dimensional arrays iterate in **row-major order** (last index varies fastest).

`For Each <var> = Each <Type>` is safe to combine with `Delete <var>` in the body — the `Delete`-rewind rule (§3.3) ensures the loop's subsequent `Next(<var>)` step yields the correct next live node.

#### Break

`Break` exits the innermost enclosing loop. `Break n` (where `n` is a constant positive integer literal) exits `n` nested loops:

```cb
For i = 0 To 9
    For j = 0 To 9
        If grid[i, j] = target Then Break 2     // exits both loops
    Next j
Next i
```

`Break` requires an enclosing loop (and `Break n` requires `n` of them); `Continue` requires an enclosing loop or `Select`. A `Break`/`Continue` with no such enclosing construct is an error (E0332). A `Break` is never satisfied by a `Select` — only loops count toward its `n`.

### 6.4 Goto and labels

A label is `name:` on a line by itself (any indentation):

```cb
    Goto cleanup

cleanup:
    closeFile(f)
```

`Goto` may jump **only within the current function** (or within top-level code, treated as one scope). Crossing function boundaries is a compile error. Jumping into the middle of a `For` block from outside is also a compile error, since the loop variable would not be initialised.

### 6.5 End

A bare `End` statement terminates the whole program immediately (exit code 0), from anywhere — including inside nested functions or loops:

```cb
If fatalCondition Then
    Print "stopping"
    End                  // program halts here; nothing after runs
EndIf
```

`End` (the statement) is distinct from the block closers `End If` / `EndFunction` / `EndType` / `EndStruct` / `EndSelect`, which close their respective blocks. A standalone `End` is only the terminate-program statement when it is **not** immediately followed by one of those block keywords.

Compiler note: `End` lowers to an IR `Halt` terminator (it is not a runtime function). The related runtime function `MakeError(msg$)` writes `msg` to stderr and terminates with exit code 1.

## 7. Subroutines and functions

### 7.1 Function

Functions are introduced with `Function`:

```cb
Function myFunc(a, b, c) As Integer
    Return a + b + c
EndFunction
```

If a function does not declare a return type, it is a **subroutine** and may be called without parentheses (statement form). A function with a return type must be called with parentheses (expression form).

```cb
Function MySub(a#, b$, c As String)
    Print a, b, c
EndFunction

MySub 0.42, "Hello", "World"         // statement call, no parens
MySub(0.42, "Hello", "World")        // also OK
```

Recursion is allowed.

### 7.2 Parameters and return type

Parameters use the same declaration syntax as variables — sigil or `As`, optionally with a default:

```cb
Function f(a As Integer, b As Float, c As String) As String
Function step(distance#, count = 1) As Float    // count defaults to 1
```

Return types can use either sigil or `As`:

```cb
Function area#(r As Float)
Function name() As String
```

**Default values** are allowed only on trailing parameters. A call may omit defaulted arguments from the right:

```cb
step(2.5)        // count = 1
step(2.5, 4)     // count = 4
```

**Function overloading.** A name may have several definitions as long as their
signatures are distinguishable, and a call selects the matching one by its
argument types:

```cb
Function area(s As Int) As Int        : Return s * s        : EndFunction
Function area(w As Int, h As Int) As Int : Return w * h     : EndFunction
area(5)       // 25  — one-argument overload
area(3, 4)    // 12  — two-argument overload
```

Resolution rules:

- Overloads are distinguished by their **parameter types** (and count), not by
  parameter names or defaults. Two definitions with the *same* parameter types
  **and** the same return type are a redefinition error.
- A call ranks candidates by argument fit: an **exact** type match is preferred,
  then a **widening** implicit conversion, then a **narrowing** one (§3.4). If no
  candidate accepts the arguments it is an error; if two remain equally good the
  call is **ambiguous** and is rejected.
- Overloads **may differ only by return type**, and a **sub** (no return type)
  may share a name with a **function**. Because a sub is a statement and a
  function call yields a value, the call context picks between them: a
  statement-position call selects the sub, an expression-position call selects
  the function. Any tie a context cannot break is ambiguous (see also §7.4 for
  taking the address of such a name).

**Parameter passing rules:**

| Argument type            | Default mode |
| ------------------------ | ------------ |
| Primitives (Int, Float, …)| by value     |
| `Struct`                 | by value (full copy) |
| Arrays                   | by reference |
| `Type` references        | by reference (it's a reference type) |
| `String`                 | by value (logically); implementation may copy-on-write |

Mutating a parameter that was passed by value does not affect the caller. Mutating an array element or a Type's field through a parameter does affect the caller, because the parameter is a reference to the same array/node.

### 7.3 Forward declarations and ordering

Functions, `Type`s, and `Struct`s have global scope and are visible everywhere regardless of definition order — there are no forward declarations and none are needed.

### 7.4 First-class functions

Function pointer types use `Function(paramTypes) [As ReturnType]`:

```cb
Dim fnPointer As Function(Integer, Float, String)
Dim fnPointer2 As Function(text As String, length As Float) As String
```

To take the address of a function, use its bare name in a value context:

```cb
fnPointer = MySub               // takes address of MySub
fnPointer(0.42, "Hello", "x")   // calls through the pointer
```

The compiler decides "address-of" vs "call" by the expression's expected type
and the absence/presence of `()`.

**Address-of an overloaded name.** When a name has several overloads (§7.2), a
bare reference to it cannot be resolved on its own — the **destination's declared
function-pointer type** selects the overload, by an **exact** signature match
(equal parameter types *and* the same return-ness). The presence or absence of
`As <ReturnType>` in that type discriminates a function from a sub of the same
parameters:

```cb
Function handle(x As Integer)              // sub
EndFunction
Function handle(x As Integer) As Integer   // function
    Return x
EndFunction

Dim asSub  As Function(Integer)              // no `As` → the sub
Dim asFunc As Function(Integer) As Integer   // `As Integer` → the function
asSub  = handle    // unambiguous
asFunc = handle    // unambiguous
```

The destination type can come from a variable, a function-pointer parameter, or
a function's return type. If the type is **not stated** — e.g. a bare
`x = handle` whose `x` would be inferred — the reference is an error: type
inference does not apply to an overloaded name. A type that matches no overload,
or matches more than one, is likewise an error.

A name with a **single** definition needs no destination type; `MySub` above
always resolves to that one function.

Functions cannot capture non-global variables, so a function pointer is always a plain typed pointer — never a closure.

A null function pointer call traps at runtime (§9.2).

## 8. Standard library surface

**The compiler itself ships only the language**, including these compiler-known intrinsics:

- Conversion: `Int`, `Float`, `Str`
- Array operations: `Len`, `New`, `Redim`
- `Type` linked-list operations: `First`, `Last`, `Next`, `Previous`, `New`, `Delete`

Everything else — `Print`, math functions, string manipulation, file I/O, graphics, input, audio — comes from a **separately built runtime crate** that the program links against. The runtime is intentionally pluggable: a different runtime can be supplied without changing the compiler.

This means a "language-only" program built without a runtime can still compile and run, but it has no way to perform I/O. The interpreter backend (`cb-backend-interp`) and the LLVM backend (`cb-backend-llvm`) both load the same runtime interface; if the two disagree on a runtime call's behaviour, the interpreter is the reference.

## 9. Error model

### 9.1 Compile-time diagnostics

Diagnostics start with `file:line:col`, an error code, and a one-line summary, then expand with context and pointers to related locations:

```
code_file.cb:32:4 error(E3202): Function cannot be defined inside a function

Functions can only be defined at top level.
Function definition is inside another Function definition that starts at code_file.cb:10:0.
```

Warnings use `warning(Wxxxx)` instead of `error(Exxxx)`. Narrowing implicit conversions (§3.4) are warnings, not errors.

### 9.2 Runtime errors

The interpreter traps and clearly reports all of the following with `file:line:col`:

- Integer division by zero (`/` between integer types, or `Mod` with a zero divisor).
- Float division by zero produces ±∞ / NaN per IEEE 754; not a trap.
- Null `Type` dereference: field access or method call on a `Null` Type reference.
- Field access through a variable in the deleted state — the variable was the operand of a `Delete` and has not been reassigned since (§3.3).
- `Delete` on a `Null` Type reference.
- `Delete` on a variable already in the deleted state (double-delete — §3.3).
- Null function pointer call.
- Array index out of bounds (any dimension).
- Failed runtime `Type` cast (where applicable to future features that introduce dynamic type checks).

The LLVM backend produces equivalent traps. A future CLI flag may opt out of bounds checks for release builds; this would explicitly diverge from the interpreter's behaviour and would not be the default.

## 10. Open questions

A scratch list of language questions that have come up but aren't answered yet. Resolve and move into the relevant section above as they're decided.

- _(none yet)_
