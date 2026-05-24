# CoolBasic Runtime Library Reference

Reference of all runtime functions from the original CBCompiler implementation.
Each function was exposed via `CBF_` prefix with C linkage (`extern "C"`).

---

## Types

### Value Types

| Type | Description |
|------|-------------|
| `Integer` | Signed 32-bit integer |
| `Float` | Single-precision 32-bit floating point |
| `String` | Unicode string |

### Handle Types

| Type | Description |
|------|-------------|
| `Image` | Bitmap/texture handle (drawable surface) |
| `File` | Open file handle |
| `Memblock` | Raw memory block for byte-level access |

### Composite Types

| Type | Description |
|------|-------------|
| User-defined (`Type...EndType`) | Struct-like type; instances form a linked list per type |
| Arrays (`Dim`) | Multi-dimensional arrays of any value/handle type |

---

## Graphics

### Screen Management

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `Screen` | `w: Int, h: Int` | — | Opens a window with given dimensions (windowed mode) |
| `Screen` | `w: Int, h: Int, mode: Int` | — | Opens a window; mode: 0=FullScreen, 1=Windowed, 2=Resizable |
| `DrawScreen` | — | — | Flips the back buffer to display the rendered frame |
| `Cls` | — | — | Clears the screen to the current background color |
| `ClsColor` | `r: Int, g: Int, b: Int` | — | Sets the background clear color (RGB 0-255) |
| `ClsColor` | `r: Int, g: Int, b: Int, a: Int` | — | Sets the background clear color (RGBA 0-255) |
| `DrawToScreen` | — | — | Sets the screen as the active render target |
| `Lock` | — | — | Locks current render target for pixel access (read/write) |
| `Lock` | `img: Image` | — | Locks an image for pixel access |
| `Lock` | `state: Int` | — | Locks current target; 0=read/write, 1=read-only, 2=write-only |
| `Lock` | `img: Image, state: Int` | — | Locks image with specified access mode |
| `Unlock` | — | — | Unlocks current render target |
| `Unlock` | `img: Image` | — | Unlocks an image |
| `FPS` | — | `Int` | Returns current frames-per-second |

### Drawing Primitives

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `Color` | `r: Int, g: Int, b: Int` | — | Sets draw color (RGB 0-255) |
| `Color` | `r: Int, g: Int, b: Int, a: Int` | — | Sets draw color (RGBA 0-255) |
| `Line` | `x1: Float, y1: Float, x2: Float, y2: Float` | — | Draws a line between two points |
| `Circle` | `x: Float, y: Float, d: Float` | — | Draws circle outline at (x,y) with diameter d |
| `Circle` | `x: Float, y: Float, d: Float, fill: Int` | — | Draws circle; fill=1 for filled |
| `Box` | `x: Float, y: Float, w: Float, h: Float` | — | Draws rectangle outline |
| `Box` | `x: Float, y: Float, w: Float, h: Float, fill: Int` | — | Draws rectangle; fill=1 for filled |
| `Dot` | `x: Float, y: Float` | — | Draws a single pixel |
| `Text` | `x: Float, y: Float, s: String` | — | Draws text string at position |

### Pixel Operations

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `PutPixel` | `x: Int, y: Int, r: Int, g: Int, b: Int` | — | Sets pixel color (RGB 0-255, requires Lock) |
| `PutPixel` | `x: Int, y: Int, r: Int, g: Int, b: Int, a: Int` | — | Sets pixel color (RGBA 0-255) |
| `PutPixel` | `x: Int, y: Int, argb: Int` | — | Sets pixel from packed 32-bit ARGB |
| `GetPixel` | `img: Image, x: Int, y: Int` | `Int` | Reads pixel as packed 32-bit ARGB (requires Lock) |

---

## Images

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `MakeImage` | `w: Int, h: Int` | `Image` | Creates a blank image of given dimensions |
| `LoadImage` | `path: String` | `Image` | Loads image from file; returns handle (error on failure) |
| `DrawImage` | `img: Image, x: Float, y: Float` | — | Draws image at position on current render target |
| `MaskImage` | `img: Image, r: Int, g: Int, b: Int` | — | Sets transparent color for image (RGB) |
| `MaskImage` | `img: Image, r: Int, g: Int, b: Int, a: Int` | — | Sets transparent color for image (RGBA) |
| `DrawToImage` | `img: Image` | — | Sets image as active render target |
| `ImageWidth` | `img: Image` | `Int` | Returns image width in pixels |
| `ImageHeight` | `img: Image` | `Int` | Returns image height in pixels |

---

## Input

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `KeyDown` | `scancode: Int` | `Int` | Returns non-zero if key is currently held down |
| `KeyUp` | `scancode: Int` | `Int` | Returns non-zero if key is currently released |
| `KeyHit` | `scancode: Int` | `Int` | Returns non-zero if key was just pressed (edge trigger) |
| `EscapeKey` | — | `Int` | Returns non-zero if Escape is pressed |

---

## Math

All trig functions work in **degrees** (not radians).

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `Sin` | `angle: Float` | `Float` | Sine (degrees) |
| `Cos` | `angle: Float` | `Float` | Cosine (degrees) |
| `Tan` | `angle: Float` | `Float` | Tangent (degrees) |
| `ASin` | `value: Float` | `Float` | Arc sine, result in degrees |
| `ACos` | `value: Float` | `Float` | Arc cosine, result in degrees |
| `ATan` | `value: Float` | `Float` | Arc tangent, result in degrees |
| `Abs` | `f: Float` | `Float` | Absolute value (float) |
| `Abs` | `i: Int` | `Int` | Absolute value (integer) |
| `Sqrt` | `f: Float` | `Float` | Square root |
| `Log` | `f: Float` | `Float` | Natural logarithm (base e) |
| `Log10` | `f: Float` | `Float` | Base-10 logarithm |
| `Distance` | `x1: Float, y1: Float, x2: Float, y2: Float` | `Float` | Euclidean distance between two points |
| `GetAngle` | `x1: Float, y1: Float, x2: Float, y2: Float` | `Float` | Angle from point 1 to point 2 (degrees) |
| `WrapAngle` | `angle: Float` | `Float` | Wraps angle to [0, 360) range |
| `Max` | `a: Float, b: Float` | `Float` | Returns larger value |
| `Max` | `a: Int, b: Int` | `Int` | Returns larger value |
| `Min` | `a: Float, b: Float` | `Float` | Returns smaller value |
| `Min` | `a: Int, b: Int` | `Int` | Returns smaller value |
| `Rnd` | `max: Float` | `Float` | Random float in [0, max) |
| `Rnd` | `min: Float, max: Float` | `Float` | Random float in [min, max) |
| `Rand` | `max: Int` | `Int` | Random integer in [0, max) |
| `Rand` | `min: Int, max: Int` | `Int` | Random integer in [min, max) |
| `Randomize` | `seed: Int` | — | Seeds the random number generator |
| `RoundUp` | `f: Float` | `Int` | Ceiling (rounds toward +infinity) |
| `RoundDown` | `f: Float` | `Int` | Floor (rounds toward -infinity) |

---

## Strings

String indices are **1-based** in CoolBasic.

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `Str` | `i: Int` | `String` | Converts integer to string |
| `Str` | `f: Float` | `String` | Converts float to string |
| `Hex` | `i: Int` | `String` | Converts integer to uppercase hex string (zero-padded to 8 chars) |
| `Chr` | `code: Int` | `String` | Returns single character from Unicode code point |
| `Left` | `s: String, n: Int` | `String` | Returns leftmost n characters |
| `Right` | `s: String, n: Int` | `String` | Returns rightmost n characters |
| `InStr` | `s: String, find: String` | `Int` | Finds substring; returns 1-based index or -1 if not found |
| `InStr` | `s: String, find: String, start: Int` | `Int` | Finds substring from start position (1-based) |
| `StrRemove` | `s: String, pos: Int, len: Int` | `String` | Removes len characters starting at pos (1-based) |
| `Trim` | `s: String` | `String` | Removes leading and trailing whitespace |
| `Lower` | `s: String` | `String` | Converts to lowercase |
| `Upper` | `s: String` | `String` | Converts to uppercase |
| `Len` | `s: String` | `Int` | Returns string length in characters |

---

## File I/O

### File Open/Close

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `OpenToRead` | `path: String` | `File` | Opens file for reading |
| `OpenToWrite` | `path: String` | `File` | Opens file for writing (creates/truncates) |
| `OpenToEdit` | `path: String` | `File` | Opens file for read-write (append) |
| `CloseFile` | `f: File` | — | Closes an open file |
| `SeekFile` | `f: File, pos: Int` | — | Seeks to byte position |
| `FileOffset` | `f: File` | `Int` | Returns current byte position |
| `EOF` | `f: File` | `Bool` | Returns true if at end of file |

### Binary Read/Write

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `ReadByte` | `f: File` | `Int` | Reads unsigned 8-bit value |
| `ReadShort` | `f: File` | `Int` | Reads unsigned 16-bit value |
| `ReadInt` | `f: File` | `Int` | Reads signed 32-bit integer |
| `ReadFloat` | `f: File` | `Float` | Reads 32-bit float |
| `ReadString` | `f: File` | `String` | Reads a string |
| `ReadLine` | `f: File` | `String` | Reads until newline |
| `WriteByte` | `f: File, v: Int` | — | Writes unsigned 8-bit value |
| `WriteShort` | `f: File, v: Int` | — | Writes unsigned 16-bit value |
| `WriteInt` | `f: File, v: Int` | — | Writes signed 32-bit integer |
| `WriteFloat` | `f: File, v: Float` | — | Writes 32-bit float |
| `WriteString` | `f: File, s: String` | — | Writes string (no newline) |
| `WriteLine` | `f: File, s: String` | — | Writes string followed by newline |

### Filesystem Operations

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `FileExists` | `path: String` | `Bool` | Returns true if path exists |
| `IsDirectory` | `path: String` | `Bool` | Returns true if path is a directory |
| `FileSize` | `path: String` | `Int` | Returns file size in bytes |
| `CopyFile` | `src: String, dest: String` | — | Copies a file |
| `DeleteFile` | `path: String` | — | Deletes a file |
| `MakeDir` | `path: String` | — | Creates a directory |
| `ChDir` | `path: String` | — | Changes working directory |
| `CurrentDir` | — | `String` | Returns current working directory |
| `Execute` | `cmd: String` | — | Executes an external program |

### Directory Search

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `StartSearch` | — | — | Begins directory iteration |
| `FindFile` | — | `String` | Returns next filename in search |
| `EndSearch` | — | — | Ends directory iteration |

---

## Memory Blocks

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `MakeMemblock` | `size: Int` | `Memblock` | Allocates a raw memory block of given byte size |
| `DeleteMemblock` | `mem: Memblock` | — | Frees a memory block |
| `PeekInt` | `mem: Memblock, offset: Int` | `Int` | Reads 32-bit int at byte offset |
| `PeekFloat` | `mem: Memblock, offset: Int` | `Float` | Reads 32-bit float at byte offset |
| `PeekShort` | `mem: Memblock, offset: Int` | `Int` | Reads 16-bit unsigned at byte offset |
| `PeekByte` | `mem: Memblock, offset: Int` | `Int` | Reads 8-bit unsigned at byte offset |
| `PokeInt` | `mem: Memblock, offset: Int, value: Int` | — | Writes 32-bit int at byte offset |
| `PokeFloat` | `mem: Memblock, offset: Int, value: Float` | — | Writes 32-bit float at byte offset |
| `PokeShort` | `mem: Memblock, offset: Int, value: Int` | — | Writes 16-bit value at byte offset |
| `PokeByte` | `mem: Memblock, offset: Int, value: Int` | — | Writes 8-bit value at byte offset |

---

## System

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `Int` | `f: Float` | `Int` | Converts float to int (rounds: adds 0.5 then truncates) |
| `Int` | `s: String` | `Int` | Parses string to integer |
| `Print` | `i: Int` | — | Prints integer + newline to stdout |
| `Print` | `f: Float` | — | Prints float + newline to stdout |
| `Print` | `s: String` | — | Prints string + newline to stdout |
| `Print` | — | — | Prints blank line |
| `Timer` | — | `Int` | Returns milliseconds since program start |
| `Wait` | `ms: Int` | — | Pauses execution for given milliseconds |
| `End` | — | — | Terminates the program |
| `MakeError` | `msg: String` | — | Shows error message and terminates |

---

## User-Defined Types

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `New` | `type: Type` | `TypeMember` | Creates new instance at end of type's list |
| `First` | `type: Type` | `TypeMember` | Returns first instance of a type |
| `Last` | `type: Type` | `TypeMember` | Returns last instance of a type |
| `After` | `member: TypeMember` | `TypeMember` | Returns next instance (or null) |
| `Before` | `member: TypeMember` | `TypeMember` | Returns previous instance (or null) |
| `Delete` | `member: TypeMember` | — | Removes instance from type's list and frees it |

---

## Arrays

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `SortArray` | `arr: Int[]` | — | Sorts a 1D integer array in ascending order |

---

## Notes

- **Overload resolution**: Many functions have multiple overloads (e.g., `Color` with 3 or 4 args, `Rnd` with 1 or 2 args). The compiler resolves these by parameter count and type at compile time.
- **Handle types**: Image, File, Memblock are opaque handles. The user never sees their internals.
- **Coordinate system**: Graphics use top-left origin, Y increases downward.
- **Angles**: All trigonometric functions use degrees, not radians.
- **1-based indexing**: String positions (InStr, Left, Right, StrRemove) are 1-based per CoolBasic convention.
