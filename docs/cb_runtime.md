# CoolBasic Runtime Library Reference

This documents the **complete CoolBasic runtime surface** implemented by
cbcompiler_rs (~345 commands and functions). It is the authoritative reference
for the runtime's language surface. Where cbcompiler_rs's internal representation
diverges from classic CoolBasic, the difference is noted — see
[Implementation status](#implementation-status-in-cbcompiler_rs).

## How the runtime is called

Each runtime entry point is a C++ method named `commandXxx`
(a **statement**, no return value) or `functionXxx` (returns a value). The
CB-visible name is the suffix (`commandRandomize` → `Randomize`,
`functionGetAngle` → `GetAngle`). Arguments are passed on a value stack; the
compiler pushes them left-to-right, so a function reads them in reverse.

Throughout this document:

- **Returns `—`** means the entry is a statement (CB *command*) with no value.
- Parameters are listed in CB source order (left-to-right as written in code).
- Many commands have optional trailing parameters; the CoolBasic compiler
  supplies defaults for omitted ones, so the runtime always sees a fixed arity.
  Where it matters to a CB programmer, the tables note the optional trailing
  parameters (in `[, …]` brackets) and their compiler-supplied defaults.

---

## Types

### Value types

| Type | Description |
|------|-------------|
| `Integer` | Signed 32-bit integer |
| `Float` | Single-precision 32-bit floating point |
| `String` | A string of characters. cbcompiler_rs stores strings internally as **Unicode code points** (UTF-8 storage); position and length operations count code points. See the [string semantics](#string-semantics) note. |

### Handle types

All handles are runtime-managed integer IDs; `0` conventionally means "invalid /
none". The user never sees their internals.

| Type | Description |
|------|-------------|
| `Image` | Bitmap/texture, optionally a multi-frame sprite sheet |
| `Font` | Loaded TrueType font |
| `Sound` | Preloaded audio sample |
| `SoundChannel` | An active sound playback instance (returned by `PlaySound`) |
| `Object` | 2D sprite object (position, angle, scale, animation, collision). Also the type of a particle emitter — see Particle Effects |
| `Map` | Tilemap |
| `File` | Open file handle |
| `Memblock` | Raw memory block for byte-level access |

### Composite types

| Type | Description |
|------|-------------|
| User-defined (`Type...EndType`) | Struct-like type; instances form a linked list per type |
| Arrays (`Dim`) | Multi-dimensional arrays of any value type |

---

## Math

All trigonometric functions work in **degrees**, not radians (converted to/from
radians internally).

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `Int` | `value: Float` | `Integer` | Float→int: rounds to the nearest integer, ties **away from zero** (`10.5 → 11`, `-1.5 → -2`), **not** a straight truncation toward zero |
| `Int` | `value: String` | `Integer` | Parses a string to an integer (trims whitespace, `stoi`) |
| `Float` | `value: Integer\|Float\|String` | `Float` | Converts a value to a float; a String is trimmed and parsed (`stof`, `0.0` on failure) |
| `RoundUp` | `value: Float` | `Integer` | Ceiling (`ceil`) |
| `RoundDown` | `value: Float` | `Integer` | Floor (`floor`) |
| `Abs` | `value: Float` | `Float` | Absolute value; preserves type (Int in → Int out, Float in → Float out) |
| `Abs` | `value: Integer` | `Integer` | Absolute value (integer) |
| `Sqrt` | `value: Float` | `Float` | Square root |
| `Sin` | `angle: Float` | `Float` | Sine (degrees) |
| `Cos` | `angle: Float` | `Float` | Cosine (degrees) |
| `Tan` | `angle: Float` | `Float` | Tangent (degrees) |
| `ASin` | `value: Float` | `Float` | Arc sine, result in degrees |
| `ACos` | `value: Float` | `Float` | Arc cosine, result in degrees |
| `ATan` | `value: Float` | `Float` | Arc tangent, result in degrees |
| `GetAngle` | `x1: Float, y1: Float, x2: Float, y2: Float` | `Float` | Angle (degrees, `[0,360)`) from point 1 to point 2; uses `atan2` with inverted Y |
| `Log` | `value: Float` | `Float` | Natural logarithm (base e) |
| `Log10` | `value: Float` | `Float` | Base-10 logarithm |
| `Rnd` | `[low: Float,] high: Float` | `Float` | Random float between `low` and `high` (single-arg `Rnd(high)` ⇒ `0..high`, `low` defaults to 0). If `high < low`, returns `randf() * low` (special-cased, not swapped) |
| `Rand` | `[low: Integer,] high: Integer` | `Integer` | Random integer between `low` and `high` (single-arg `Rand(high)` ⇒ `0..high`). If `high < low`, returns `rand(low)` |
| `Randomize` | `seed: Integer` | — | Seeds the random number generator |
| `Min` | `a: Float, b: Float` | `Float` | Smaller of two values (also `Integer` overload) |
| `Max` | `a: Float, b: Float` | `Float` | Larger of two values (also `Integer` overload) |
| `Distance` | `x1: Float, y1: Float, x2: Float, y2: Float` | `Float` | Euclidean distance between two points |
| `WrapAngle` | `angle: Float` | `Float` | Normalizes an angle to `[0,360)`; preserves Int/Float type |
| `CurveValue` | `target: Float, current: Float, smoothness: Float` | `Float` | Eased step toward `target`: `current + (target - current) / smoothness` |
| `CurveAngle` | `target: Float, current: Float, smoothness: Float` | `Float` | Like `CurveValue` but for angles; wraps at 360 and takes the shortest path |
| `BoxOverlap` | `x1, y1, w1, h1, x2, y2, w2, h2: Float` | `Integer` | 1 if two axis-aligned rectangles overlap, else 0 (Y negated internally for world space) |

---

## Strings

String positions are **1-based** (CoolBasic convention).

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `Str` | `value: Integer` | `String` | Integer→string (also `Float`/`String` overloads; a String passes through) |
| `Left` | `s: String, n: Integer` | `String` | Leftmost `n` characters |
| `Right` | `s: String, n: Integer` | `String` | Rightmost `n` characters (`n ≥ 0`; clamps to full string) |
| `Mid` | `s: String, pos: Integer [, len: Integer]` | `String` | `len` characters starting at `pos` (1-based; `pos > 0`); omit `len` to read to the end of the string |
| `Replace` | `s: String, find: String, repl: String` | `String` | Replaces **all** occurrences of `find`; empty `find` returns `s` unchanged |
| `InStr` | `s: String, find: String [, start: Integer]` | `Integer` | 1-based position of `find` at or after `start` (default `start` = 1); `0` if not found |
| `Upper` | `s: String` | `String` | Uppercase |
| `Lower` | `s: String` | `String` | Lowercase |
| `Trim` | `s: String` | `String` | Removes leading and trailing whitespace |
| `LSet` | `s: String, len: Integer` | `String` | Left-aligns into a field of width `len`, padding spaces on the right |
| `RSet` | `s: String, len: Integer` | `String` | Right-aligns into width `len`; truncates to rightmost `len` chars if longer |
| `Chr` | `code: Integer` | `String` | Single character from a character code (inverse of `Asc`) |
| `Asc` | `s: String` | `Integer` | Character code of the first character (inverse of `Chr`) |
| `Len` | `s: String` | `Integer` | String length in **characters** |
| `Hex` | `value: Integer` | `String` | Uppercase hex, zero-padded to 8 characters |
| `Bin` | `value: Integer` | `String` | 32-bit binary string |
| `String` | `s: String, count: Integer` | `String` | `s` repeated `count` times |
| `Flip` | `s: String` | `String` | Reversed string |
| `StrInsert` | `s: String, pos: Integer, txt: String` | `String` | Inserts `txt` at 1-based `pos` (appends if `pos` past end) |
| `StrRemove` | `s: String, pos: Integer, len: Integer` | `String` | Removes `len` chars starting at 1-based `pos` |
| `StrMove` | `s: String, pos: Integer, len: Integer, offset: Integer` | `String` | Cuts `len` chars at `pos` and re-inserts them at `pos + offset` |
| `CountWords` | `s: String, sep: String` | `Integer` | Number of `sep`-separated words (empty `sep` → space) |
| `GetWord` | `s: String, n: Integer, sep: String` | `String` | The `n`-th (1-based) `sep`-separated word (empty `sep` → space) |

### String semantics

- **Code-point storage.** Strings are held as Unicode code points (UTF-8
  storage), one element per code point. Consequently `Len`, `Left`, `Right`,
  `Mid`, `InStr`, etc. count **code points**, and `Chr`/`Asc` map a single code
  point. This is a deliberate divergence from classic CoolBasic's single-byte
  CP-1252 / Latin-1 strings, which counted bytes and mapped a single byte 0–255.
- **1-based indexing** for all position arguments.
- **`InStr` not-found** returns `0`.

---

## System & Time

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `Timer` | — | `Integer` | Milliseconds since system boot (uptime sampled at program start + elapsed); monotonic, suitable for measuring elapsed time |
| `Wait` | `ms: Integer` | — | Blocks for `ms` milliseconds (`al_rest`) |
| `Date` | — | `String` | Current date as `D Mon YYYY` (e.g. `9 Jun 2026`; non-zero-padded day, 3-letter English month) |
| `Time` | — | `String` | Current time as `HH:MM:SS` |
| `CommandLine` | — | `String` | Command-line arguments passed to the program |
| `GetEXEName` | — | `String` | Absolute path of the running executable |
| `FPS` | — | `Integer` | Current frames-per-second |
| `FrameLimit` | `fps: Integer` | — | Caps the frame rate |
| `SetWindow` | `title: String [, mode: Integer] [, confirm: String]` | — | Sets window title; `mode` (optional): 0=no change, 1=restore, 2=minimize, 3=maximize (Windows). Optional `confirm` is shown if the user tries to close the window |
| `Crc32` | `pathOrMemblock: String\|Integer` | `Integer` | CRC32 of a file (path) or memblock (id) |
| `Errors` | `enabled: Integer` | — | Enables/disables display of error messages |
| `MakeError` | `msg: String` | — | Raises a fatal error with `msg` and halts |
| `End` | — | — | Stops the program cleanly |

---

## Graphics

Coordinate system: origin top-left, **X right, Y down**. Many primitives offset
coordinates by `+0.5` for pixel-center alignment.

### Screen management

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `Screen` | `w: Integer, h: Integer, depth: Integer, mode: Integer` | — | Opens/resizes the display. `depth` defaults to 32. `mode`: 0=fullscreen, 1=windowed, 2=resizable, 3=resizable locked-aspect |
| `Screen` | — | `Integer` | (function form) Returns the screen render-target buffer id |
| `ScreenWidth` | — | `Integer` | Current screen width (px) |
| `ScreenHeight` | — | `Integer` | Current screen height (px) |
| `ScreenDepth` | — | `Integer` | Color depth (bits) |
| `GFXModeExists` | `w: Integer, h: Integer, depth: Integer` | `Integer` | 1 if that graphics mode is available |
| `DrawScreen` | `cls: Integer, vsync: Integer` | — | Flushes drawing to the display, optionally clears the backbuffer, processes window events, and applies the frame limit |
| `Cls` | — | — | Clears the current render target to the clear color |
| `ClsColor` | `r: Integer, g: Integer, b: Integer` | — | Sets the clear color (0–255) |
| `ScreenGamma` | `r: Integer, g: Integer, b: Integer` | — | Applies whole-screen gamma correction |
| `ScreenShot` | `filename: String` | — | Saves the screen buffer to an image file |

### Draw state / color

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `Color` | `r: Integer, g: Integer, b: Integer` | — | Sets the draw color (0–255) for subsequent drawing |
| `GetRGB` | `channel: Integer` | `Integer` | Component of the current draw color (1=R, 2=G, 3=B, 4=A) |
| `PickColor` | `x: Integer, y: Integer` | — | Reads a screen pixel and makes it the current draw color |
| `Smooth2D` | `enabled: Integer` | — | Enables/disables 2D antialiasing & smoothing |

### Drawing primitives

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `Dot` | `x: Float, y: Float` | — | Draws a single pixel |
| `Line` | `x1: Float, y1: Float, x2: Float, y2: Float` | — | Draws a line between two points |
| `Box` | `x: Float, y: Float, w: Float, h: Float [, fill: Integer]` | — | Rectangle at top-left `(x,y)`; `fill`=0 outline, non-zero filled (optional, default filled) |
| `Circle` | `x: Float, y: Float, diameter: Float [, fill: Integer]` | — | Circle (input is a **diameter**); `fill`=0 outline, non-zero filled (optional, default filled) |
| `Ellipse` | `x: Float, y: Float, w: Float, h: Float [, fill: Integer]` | — | Ellipse (`w`,`h` are full diameters); `fill`=0 outline, non-zero filled (optional, default filled) |

### Pixel operations

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `PutPixel` | `x: Integer, y: Integer, r: Integer, g: Integer, b: Integer` | — | Writes an RGB pixel to the current render target |
| `PutPixel` | `x: Integer, y: Integer, r: Integer, g: Integer, b: Integer, a: Integer` | — | Writes an RGBA pixel to the current render target |
| `PutPixel` | `x: Integer, y: Integer, argb: Integer` | — | Writes a packed 32-bit `0xAARRGGBB` pixel (`PutPixel2` is the alias for this form) |
| `GetPixel` | `x: Integer, y: Integer` | `Integer` | Reads the current render target as packed `0xAARRGGBB` (`GetPixel2` is an alias) |
| `GetPixel` | `img: Image, x: Integer, y: Integer` | `Integer` | Reads a pixel from a specific image as packed `0xAARRGGBB` |
| `Lock` | `[mode: Integer]` | — | Locks the **current** render target for direct pixel access. `mode` (optional): 0=read/write (default), 1=read-only, 2=write-only |
| `Lock` | `img: Image [, mode: Integer]` | — | Locks a specific image's bitmap; `mode` as above |
| `Unlock` | `[img: Image]` | — | Unlocks the current render target (or `img`), flushing pixel writes back |

**Locking.** `Lock`/`Unlock` bracket a run of `PutPixel`/`GetPixel` calls: locking
maps the target bitmap into directly-addressable memory so per-pixel access skips
the per-call driver round-trip, and `Unlock` flushes writes back. Always pair a
`Lock` with an `Unlock`. The optional `mode` lets the runtime pick a faster
transfer path when you only read (`1`) or only write (`2`) — e.g. `Lock 2` for a
pure pixel-generation loop. Which surface is locked follows the current render
target (`DrawToImage`/`DrawToScreen`) unless an `Image` is passed explicitly.

### Render targets

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `Image` | `imageId: Image, unused: Integer` | `Integer` | Render-target buffer id backing an image (the 2nd argument is unused) |
| `DrawToImage` | `imageId: Image` | — | Redirects subsequent drawing onto an image's bitmap |
| `DrawToScreen` | — | — | Redirects subsequent drawing back to the screen |
| `DrawToWorld` | `drawCommands: Integer, drawImages: Integer, drawText: Integer` | — | Flags controlling whether primitives / images / text render in world (camera) coordinates vs screen coordinates |
| `CopyBox` | `srcX, srcY, w, h, destX, destY: Float, srcBuf: Integer, destBuf: Integer` | — | Copies a rectangular region between render targets as a straight opaque blit (forces a replace blender, bypassing the mask / current blender) |
| `UpdateGame` | — | — | Runs object update callbacks and advances game objects |
| `DrawGame` | — | — | Runs draw callbacks and renders game objects to the current target |

---

## Images

An `Image` may be a single bitmap or a multi-frame sprite sheet. Frame indices
are zero-based.

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `LoadImage` | `path: String` | `Image` | Loads an image; applies the default mask/hotspot if enabled; 0 on failure |
| `LoadAnimImage` | `path: String, frameW: Integer, frameH: Integer, startFrame: Integer, frameCount: Integer` | `Image` | Loads a sprite sheet sliced into `frameW × frameH` frames |
| `MakeImage` | `w: Integer, h: Integer [, frameCount: Integer]` | `Image` | Creates a blank image; omit `frameCount` for a single frame |
| `CloneImage` | `img: Image` | `Image` | Copies an image and all properties |
| `ImageWidth` | `img: Image` | `Integer` | Image width (px) |
| `ImageHeight` | `img: Image` | `Integer` | Image height (px) |
| `DrawImage` | `img: Image, x: Float, y: Float, frame: Integer, useMask: Integer` | — | Draws a frame at `(x,y)`; `useMask` skips transparent pixels; honors `DrawToWorld` |
| `DrawGhostImage` | `img: Image, x: Float, y: Float, frame: Integer, alpha: Float` | — | Draws with alpha 0–100 (0=transparent, 100=opaque) |
| `DrawImageBox` | `img: Image, dstX: Float, dstY: Float, srcX: Float, srcY: Float, w: Float, h: Float, frame: Integer, useMask: Integer` | — | Copies the `w`×`h` source sub-rectangle at `(srcX,srcY)` of `img` to `(dstX,dstY)` on the current target — a 1:1 copy, **no scaling**; honors `useMask`/`frame` and `DrawToWorld` |
| `MaskImage` | `img: Image, r: Integer, g: Integer, b: Integer` | — | Sets the per-image transparent color key |
| `DefaultMask` | `enabled: Integer, r: Integer, g: Integer, b: Integer` | — | Sets the default mask color applied to future images |
| `HotSpot` | `id: Integer, x: Integer, y: Integer` | — | Sets rotation/scale origin: `id`=0 disable default, 1 store default `(x,y)` for future images, `>1` set hotspot of that image handle. Passing a negative `x` or `y` centers the hotspot on the frame/image |
| `ResizeImage` | `img: Image, w: Integer, h: Integer` | — | Resizes an image |
| `RotateImage` | `img: Image, angle: Float` | — | Rotates the image bitmap (degrees, clockwise) |
| `PickImageColor` | `img: Image, x: Integer, y: Integer` | — | Reads a pixel from an image and makes it the draw color (`PickImageColor2` alias) |
| `SaveImage` | `img: Image, path: String [, frame: Integer]` | — | Writes the whole image to disk (the extension picks the format); `frame` is accepted but **ignored** |
| `DeleteImage` | `img: Image` | — | Frees an image |
| `ImagesOverlap` | `img1: Image, x1: Float, y1: Float, img2: Image, x2: Float, y2: Float` | `Integer` | Axis-aligned bounding-box test between two placed images |
| `ImagesCollide` | `img1: Image, x1: Float, y1: Float, frame1: Integer, img2: Image, x2: Float, y2: Float, frame2: Integer` | `Integer` | Pixel-precise collision test between two placed image frames |

---

## Text & Fonts

`Print`/`Write` go to **stdout** (the program's console). On-screen text uses the
`Locate`/`AddText` family and the immediate `Text`/`CenterText`/`VerticalText`
drawing commands.

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `Print` | `[s: String]` | — | Writes `s` + newline to stdout (UTF-8); omit `s` for a blank line. Accepts Integer/Float/String |
| `Write` | `s: String` | — | Writes `s` to stdout without a newline |
| `Locate` | `x: Integer, y: Integer` | — | Sets the on-screen text cursor for `AddText` |
| `AddText` | `s: String` | — | Queues on-screen text at the cursor (current font/color), advancing one line |
| `ClearText` | — | — | Clears all queued on-screen text and resets the cursor |
| `Text` | `x: Float, y: Float, s: String` | — | Draws text immediately at `(x,y)` in the current font/color; honors `DrawToWorld` |
| `CenterText` | `x: Integer, y: Integer, s: String [, style: Integer]` | — | Centered text; `style` (optional, default 0): 0=horizontal, 1=vertical, 2=both |
| `VerticalText` | `x: Integer, y: Integer, s: String` | — | Draws text one character per line, top-to-bottom |
| `LoadFont` | `name: String [, size: Integer] [, bold: Integer] [, italic: Integer] [, underline: Integer]` | `Font` | Loads a TrueType font by family name or file path; honors `Smooth2D`; 0 on failure. Optional `size` (default 13), `bold`/`italic`/`underline` (default off) |
| `SetFont` | `font: Font` | — | Sets the current font |
| `DeleteFont` | `font: Font` | — | Frees a font |
| `TextWidth` | `s: String` | `Integer` | Pixel width of `s` in the current font |
| `TextHeight` | `s: String` | `Integer` | Pixel height of `s` in the current font |

---

## Sound

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `LoadSound` | `path: String` | `Sound` | Loads an audio file fully into memory as a sample; `Null` on failure |
| `PlaySound` | `sound: Sound\|String, volume: Float, balance: Float, frequency: Integer` | `SoundChannel` | Plays a sound — a preloaded `Sound` (one-shot sample) or a filename `String` (streamed); returns a `SoundChannel`. `volume`/`balance`/`frequency` are optional (default 100 / 0 / native). Also usable as a fire-and-forget statement (the channel is discarded). A finished channel is auto-reaped each frame |
| `SoundPlaying` | `channel: SoundChannel` | `Integer` | 1 if the channel is still playing, else 0 (incl. a finished/reaped channel) |
| `SetSound` | `channel: SoundChannel, looping: Integer, volume: Float, balance: Float, frequency: Integer` | — | Updates loop/volume/pan/pitch on an active channel; `volume`/`balance`/`frequency` optional |
| `StopSound` | `channel: SoundChannel` | — | Stops a channel |
| `DeleteSound` | `sound: Sound` | — | Frees a preloaded sound |

**Implemented (`runtime/cb_sound.cpp`).** `Sound` is the opaque `CbSound*`
handle (catalog tag 17, a loaded `ALLEGRO_SAMPLE`); `SoundChannel` is a playing
instance (catalog tag 18 — a sample-instance or audio-stream). Both are
Allegro-dependent (`allegro_audio` + `allegro_acodec`), so they are **absent from
the SDK-free catalog** (like `Image`/`Font`/`Object`/`Map`). Deliberate
divergences from classic CoolBasic:

- **Opaque `Sound`/`SoundChannel` types** (`Null` default / `Null` on load
  failure) instead of classic CB's plain `int32` ids — consistent with
  `Object`/`Map`/`File`. Drops the classic int-handle arithmetic.
- **The CB-visible channel type is `SoundChannel`, not `Channel`** — the bare
  noun is polysemous (colour/network channel) and stays free for user code.
- **A finished `SoundChannel` is a safe silent no-op.** Finished channels are
  reaped every frame; a generation-tagged handle pool makes
  `SetSound`/`StopSound`/`SoundPlaying` on a stale/finished channel a silent
  no-op (return 0), never a use-after-free. An invalid **`Sound`** handle still
  traps (exit 1) — a deliberate asymmetry.
- **Graceful audio-less degradation.** Best-effort init never aborts: with no
  audio device `LoadSound`/`PlaySound` return `Null`, `SoundPlaying` returns 0,
  `Set`/`StopSound` no-op, and the null-`Sound` trap is suppressed (so a
  `Null`-ignoring program runs silently on a headless/CI host).
- **`PlaySound`'s polymorphic first arg** is two overloads (preloaded `Sound`
  vs filename `String` → streamed, 3×8192 buffers); the optional
  `volume`/`balance`/`frequency` are arity overloads. `volume`/`balance` are the
  0–100 scale (→ Allegro gain / pan ±1); `frequency` is an absolute target Hz
  (→ a speed ratio; ≤0 leaves the native rate).
- **No CD-track form** (the Help's integer-track CD path is unimplemented; CD
  audio is dead on modern targets); no `Music`/master-volume/3D-positional
  commands (classic CB has none — "music" is streamed `PlaySound`).
- **Audible playback** (real playback, looping, pan/pitch sweep, long-file
  streaming) requires a real audio device; the gain/pan/speed math and the
  channel-pool liveness run headless.

---

## Video Playback

CoolBasic `…Animation` commands play a video file (single active video).

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `PlayAnimation` | `path: String` | `Integer` | Loads and starts a video; audio mixes through the sound interface; 1 on success |
| `AnimationWidth` | — | `Integer` | Scaled display width of the current video (0 if none) |
| `AnimationHeight` | — | `Integer` | Scaled display height of the current video (0 if none) |
| `AnimationPlaying` | — | `Integer` | 1 if a video is currently playing |
| `DrawAnimation` | — | — | Draws the current video frame at `(0,0)` |
| `StopAnimation` | — | — | Stops and closes the current video |

---

## Particle Effects

A particle emitter **is an `Object`**. `MakeEmitter` returns the `Object` handle,
so the emitter is moved, rotated, given a life, deleted, and enumerated with the
ordinary object commands below — particles fly in the direction the emitter
object faces, within the `spread` sector. There is no distinct `Emitter` type
(cbcompiler_rs reuses `Object`; a distinct type would not type-check against the
object commands). The four entry points below are the entire public surface.

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `MakeEmitter` | `img: Image, lifeTime: Integer` | `Object` | Creates a particle emitter at (0,0) using `img` as the particle sprite; `lifeTime` is each **particle's** life in frames (update cycles). Delete with `DeleteObject` |
| `ParticleMovement` | `emitter: Object, speed: Float, gravity: Float [, acceleration: Float]` | — | Launch `speed` (px), `gravity` (subtracted from vertical velocity each update — positive pulls particles down), and the optional per-frame velocity multiplier `acceleration` (default 1; `<1` decelerates, `>1` accelerates) |
| `ParticleEmission` | `emitter: Object, density: Integer, count: Integer, spread: Integer` | — | `density` = spawn **interval** in update cycles (smaller = denser; a batch spawns whenever an internal per-frame counter exceeds `density`), `count` = particles per batch, `spread` = the ± angular half-sector in degrees (0..180; 180 = all directions, 0 = a tight stream). Launch direction is **uniform** over the sector |
| `ParticleAnimation` | `emitter: Object, frameCount: Integer` | — | Animate a `LoadAnimImage` particle sprite as a `frameCount`-long strip, played once over each particle's life (frame 0 at spawn → last at death). Clamped to the strip's real length |

**Behavior notes & divergences from the docs:**

- **Emitters never pick or collide.** Despite the CoolBasic Help ("*even collision
  and pick commands work with sources*"), emitters do neither —
  `ObjectPickable`/`SetupCollision`/`ObjectsOverlap` are inert on an emitter, and
  pixel-pick is kept from crashing.
- **`ParticleMovement`/`ParticleEmission`/`ParticleAnimation` trap** (clean runtime
  error) if handed a non-emitter `Object` — classic CB blind-casts the handle (UB).
- **Animation plays forward** (frame 0 → last over the particle's life), per the
  Help. Frame slicing uses correct row/offset math (`row = frame / framesX`).
- **No `StopEmitting` command** — it is not part of CoolBasic. Deleting an emitter
  (`DeleteObject`, or `ObjectLife` expiry) lets its live particles finish before
  the emitter is freed.
- A non-positive emission `density` spawns nothing.

---

## Objects (Sprites)

The object system represents 2D sprites with position, rotation, scale, optional
animation, custom data slots, and collision. `Object` is an **opaque handle type**
(tag 13) — in cbcompiler_rs the handle's bit pattern is the runtime's `CbObject*`,
mirroring `Image`/`Font`/`Map`; there is no integer id (classic CoolBasic's legacy
`Integer` handles and shared id space are dropped). "Floor" objects draw before
regular objects (background layering). Positions are in world space unless noted.

### Creation & destruction

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `LoadObject` | `path: String [, rotQuality: Integer]` | `Object` | Loads a single-frame image as an object. `rotQuality` (1–360, default 1) = facing-direction count; accepted but ignored |
| `LoadAnimObject` | `path: String, frameW: Integer, frameH: Integer, startFrame: Integer, frameCount: Integer [, rotQuality: Integer]` | `Object` | Loads a sprite-sheet object. `rotQuality` as in `LoadObject` (accepted but ignored) |
| `MakeObject` | — | `Object` | Creates an empty (imageless) object |
| `MakeObjectFloor` | — | `Object` | Creates a floor object (drawn before regular objects) |
| `CloneObject` | `obj: Object` | `Object` | Copies an object's image (shared, reference-counted), mask, frames, animation and range; **position and angle reset to 0** and **visibility is forced on** (`visible=true`, not copied). Map objects can't be cloned |
| `DeleteObject` | `obj: Object` | — | Deletes an object and clears its collisions |
| `ClearObjects` | — | — | Deletes all objects and clears the draw chains. **Objects-only — the active tilemap is left alone** (the map is an independent singleton owned by `LoadMap`/`MakeMap`, not a floor object, so `ClearObjects` does not free it — a divergence from classic CoolBasic) |

### Position & movement

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `PositionObject` | `obj: Object, x: Float, y: Float [, z: Float]` | — | Sets absolute world position (`z` accepted but ignored) |
| `ScreenPositionObject` | `obj: Object, sx: Float, sy: Float` | — | Sets position from screen coords (converted via camera) |
| `MoveObject` | `obj: Object, forward: Float [, side: Float] [, z: Float]` | — | Moves relative to the object's facing angle. The 2-arg form moves straight forward (`side = 0`, the common idiom); `z` accepted but ignored |
| `TranslateObject` | `obj: Object, dx: Float, dy: Float [, dz: Float]` | — | Moves by an absolute world delta (`dz` forwarded to the object's depth) |
| `CloneObjectPosition` | `dst: Object, src: Object` | — | Copies `src`'s position to `dst` |
| `ObjectX` | `obj: Object` | `Float` | World X of the object center |
| `ObjectY` | `obj: Object` | `Float` | World Y of the object center |

### Rotation & angle

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `RotateObject` | `obj: Object, angle: Float` | — | Sets absolute rotation (degrees; 0°=right, 90°=down) |
| `TurnObject` | `obj: Object, speed: Float` | — | Rotates **by** `speed` degrees, **relative** to the current angle (a one-shot delta applied when the command runs — *not* a stored per-frame turn rate) |
| `PointObject` | `obj: Object, target: Object` | — | Rotates `obj` to face `target` |
| `CloneObjectOrientation` | `dst: Object, src: Object` | — | Copies `src`'s angle to `dst` |
| `ObjectAngle` | `obj: Object` | `Float` | Current rotation (degrees) |
| `GetAngle2` | `a: Object, b: Object` | `Float` | Angle from object `a` to object `b` |
| `Distance2` | `a: Object, b: Object` | `Float` | Distance between two object centers |

### Appearance

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `PaintObject` | `obj: Object, image: Image` | — | Replaces the object's image with a masked clone of the image |
| `PaintObject` | `obj: Object, source: Object` | — | Replaces the object's image with a clone of another object's image |
| `PaintObject` | `map: Map, image: Image` | — | Repaints the active tilemap's tileset with the image (the `map` handle is popped but ignored, like `EditMap`) |
| `MaskObject` | `obj: Object, r: Integer, g: Integer, b: Integer` | — | Sets a transparent color key |
| `GhostObject` | `obj: Object, alpha: Float` | — | Sets alpha 0–100 (clamped) |
| `MirrorObject` | `obj: Object, direction: Integer` | — | Mirrors the image: 0=horizontal, 1=vertical, 2=both (regular objects only) |
| `ShowObject` | `obj: Object, visible: Integer` | — | Shows/hides (hidden objects still update & collide) |
| `DefaultVisible` | `visible: Integer` | — | Default visibility for newly created objects |
| `ObjectOrder` | `obj: Object, direction: Integer` | — | Draw order: −1 = to back, 1 = to front |
| `ObjectSizeX` | `obj: Object` | `Integer` | Bounding width (accounts for rotation) |
| `ObjectSizeY` | `obj: Object` | `Integer` | Bounding height (accounts for rotation) |

### Animation

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `PlayObject` | `obj: Object [, startFrame: Integer] [, endFrame: Integer] [, speed: Float] [, continuous: Integer]` | — | Plays frames once (optional args; `speed` default 0.1, `continuous` default off); `endFrame = -1` stops and resets |
| `PlayObject` | `map: Map [, startFrame: Integer] [, endFrame: Integer] [, speed: Float] [, continuous: Integer]` | — | Starts the active tilemap's per-tile animation (each animated tile cycles `tile..tile+animLength`, i.e. `animLength+1` frames); only `speed` applies and tiles do not advance until called. Higher `speed` = slower (it divides the elapsed-time step); `speed = 0` / `endFrame = -1` stops. The `Map` first param selects this overload |
| `LoopObject` | `obj: Object [, startFrame: Integer] [, endFrame: Integer] [, speed: Float] [, continuous: Integer]` | — | Loops the frame range continuously (optional args; `speed` default 0.1) |
| `StopObject` | `obj: Object` | — | Stops animation, keeping the current frame |
| `ObjectPlaying` | `obj: Object` | `Integer` | 1 if an animation is playing |
| `ObjectFrame` | `obj: Object` | `Float` | Current frame index (may be fractional) |

### Custom data slots

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `ObjectInteger` | `obj: Object [, value: Integer]` | `Integer` / — | Get/set a per-object integer slot |
| `ObjectFloat` | `obj: Object [, value: Float]` | `Float` / — | Get/set a per-object float slot |
| `ObjectString` | `obj: Object [, value: String]` | `String` / — | Get/set a per-object string slot |
| `ObjectLife` | `obj: Object [, frames: Integer]` | `Integer` / — | Get/set object lifetime in **update ticks**; decremented once per tick (the implicit `DrawScreen` update, *or* an explicit `UpdateGame`/`DrawGame`), auto-deletes at 0 |

### Collision

See the [collision model](#collision-model) note for types and handling modes.

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `SetupCollision` | `objA: Object, objB: Object, typeA: Integer, typeB: Integer, handling: Integer` | — | Registers a collision check. Types: 1=box, 2=circle, 4=map (B only). `handling`: 0=report, 1=stop (circle), 2=slide |
| `ObjectRange` | `obj: Object, range1: Float [, range2: Float]` | — | Collision bounds: box = width,height; circle = diameter |
| `ResetObjectCollision` | `obj: Object` | — | Clears recorded collisions for the object this frame |
| `ClearCollisions` | — | — | Removes all collision checks |
| `CountCollisions` | `obj: Object` | `Integer` | Number of collisions recorded this frame |
| `GetCollision` | `obj: Object, index: Integer` | `Object` | Colliding object by **1-based** index (1..`CountCollisions`; 0 if none) |
| `CollisionX` | `obj: Object, index: Integer` | `Float` | Contact X of a collision |
| `CollisionY` | `obj: Object, index: Integer` | `Float` | Contact Y of a collision |
| `CollisionAngle` | `obj: Object, index: Integer` | `Float` | Contact normal angle (degrees) |
| `ObjectsOverlap` | `a: Object, b: Object [, type: Integer]` | `Integer` | One-shot overlap test; `type` (optional, default 1): 1=box, 2=circle, 3=pixel (pixel not yet implemented) |

### Picking & line of sight

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `ObjectPickable` | `obj: Object, style: Integer` | — | Marks pickable: 0=off, 1=box, 2=circle, 3=pixel (pixel partial) |
| `ObjectPick` | `picker: Object` | — | Raycasts from `picker` along its facing angle; stores the nearest hit |
| `PixelPick` | `picker: Object [, accuracy: Integer]` | — | Pixel-perfect pick from inside `picker` along its facing angle (needs `ObjectPickable obj, 3`). **Registered but a no-op stub** |
| `PickedObject` | — | `Object` | Object hit by the last `ObjectPick` (0 if none) |
| `PickedX` | — | `Float` | World X of the last pick hit |
| `PickedY` | — | `Float` | World Y of the last pick hit |
| `PickedAngle` | — | `Float` | Angle from picker to the pick point |
| `ObjectSight` | `a: Object, b: Object` | `Integer` | 1 if a clear line (no map walls) exists between two objects |

### Enumeration

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `InitObjectList` | — | — | Resets the shared object iterator |
| `NextObject` | — | `Object` | Next object handle in creation order, or `Null` at the end; call `InitObjectList` first. (Returns an `Object` handle / `Null` rather than classic CoolBasic's id / `0`, and does **not** surface map ids — `Map` is a separate opaque type) |

---

## Camera

The camera holds a world position, rotation, and zoom (`>1` zooms in, `<1` out).
Screen↔world conversion uses Allegro transforms; world Y is inverted relative to
screen Y.

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `PositionCamera` | `x: Float, y: Float, zoom: Float` | — | Sets absolute position and zoom (`zoom > 0.00001`) |
| `MoveCamera` | `forward: Float, side: Float, dzoom: Float` | — | Moves relative to the camera angle, adjusting zoom |
| `TranslateCamera` | `dx: Float, dy: Float, dzoom: Float` | — | Moves in absolute world space, adjusting zoom |
| `RotateCamera` | `logical: Float, render: Float` | — | Sets absolute rotation. `logical` (degrees) is reported by `CameraAngle` and drives `MoveCamera`'s heading; `render` (degrees) is the world-matrix rotation. The two fields are **independent** and may diverge |
| `TurnCamera` | `dLogical: Float, dRender: Float` | — | Rotates relatively: `dLogical` (degrees) wraps to 0–360; `dRender` (degrees) accumulates into the world-matrix rotation (stored internally in radians, wrapped to 0–2π) |
| `PointCamera` | `obj: Object` | — | Rotates the camera to point at an object |
| `CameraFollow` | `obj: Object, style: Integer, setting: Float` | — | Follows an object. `style` 1=smooth (divide distance by `setting`), 2=margin deadzone (`setting`=px), 3=orbit (`setting`=distance) |
| `CloneCameraPosition` | `obj: Object` | — | Snaps camera position to an object; stops following |
| `CloneCameraOrientation` | `obj: Object` | — | Snaps camera angle to an object |
| `CameraPick` | `sx: Float, sy: Float` | — | Picks the object at screen coords (world-converts then picks) |
| `CameraX` | — | `Float` | Camera world X |
| `CameraY` | — | `Float` | Camera world Y |
| `CameraAngle` | — | `Float` | Camera angle (degrees, 0–360) |

---

## Tile Maps

A `Map` is a tile grid with up to 4 layers: 0=background, 1=foreground,
2=collision (always active), 3=data (per-tile integers, e.g. triggers/terrain;
readable/writable via GetMap/GetMap2/EditMap but never drawn). Tiles are referenced by **1-based** id in
game code (0 = empty). Only one map is active at a time.

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `LoadMap` | `mapPath: String, tilesetPath: String` | `Map` | Loads a `.til` map file + tileset image; `Null` on failure; replaces any existing map |
| `MakeMap` | `wTiles: Integer, hTiles: Integer, tileW: Integer, tileH: Integer` | `Map` | Creates an empty tilemap; replaces any existing map |
| `MapWidth` | — | `Integer` | Map width in tiles |
| `MapHeight` | — | `Integer` | Map height in tiles |
| `GetMap` | `layer: Integer, x: Float, y: Float` | `Integer` | Tile id at world coordinates (0 if out of bounds) |
| `GetMap2` | `layer: Integer, tx: Integer, ty: Integer` | `Integer` | Tile id at grid position (1-based; 0 if out of bounds) |
| `EditMap` | `map: Map, layer: Integer, tx: Integer, ty: Integer, tile: Integer` | — | Sets a tile at a 1-based grid position (out-of-bounds ignored). `map` is popped but ignored — the single active map is edited |
| `SetMap` | `backLayer: Integer, overLayer: Integer` | — | Toggles visibility of the background (0) and foreground (1) layers |
| `SetTile` | `tile: Integer, animLength: Integer [, animSlowness: Integer]` | — | Configures per-tile animation (frame count + slowness; `animSlowness` default 1) |

The map is loaded from a CoolBasic `.til` binary (little-endian; on-disk layer
order 0, 2, 1, 3; two absolute seeks for editor metadata). The format is
**compatibility-frozen** and was byte-verified against a real asset; the per-tile
animation block stores `tileCount` entries but only `tileCount-1` are read (the
trailing 8 bytes are ignored). `EditMap`'s `map` argument is popped but ignored —
the single active map is edited. `SetTile` stores per-tile animation params; tile
animation advances on the game-loop update tick — a deterministic **frame-step**
(a fixed step per tick so headless runs reproduce). The map renders inside the
object draw order (background layer 0 before objects, foreground layer 1 after).

---

## Input

Input state is sampled per **frame** inside `DrawScreen` (which drains the event
queue). Edge queries (`KeyHit`/`KeyUp`, `MouseHit`/`MouseUp`) and movement deltas
(`MouseMoveX/Y/Z`) are relative to the previous `DrawScreen`. Without a window /
`DrawScreen`, every query returns 0.

State uses a 2-bit-per-key/button model: `Down` = currently held, `Pressed` =
went down this frame (edge), `Released` = went up this frame (edge), `Up` = idle.

### Keyboard

`scancode` uses the legacy DirectInput-style numbering (see [table](#scancode-table)).

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `KeyDown` | `scancode: Integer` | `Integer` | 1 if the key is currently held (level) |
| `KeyHit` | `scancode: Integer` | `Integer` | 1 if pressed this frame (edge) |
| `KeyUp` | `scancode: Integer` | `Integer` | 1 if released this frame (edge) |
| `GetKey` | — | `Integer` | Next queued character code, or 0 |
| `WaitKey` | — | `Integer` / — | Blocks until a key is pressed; function form returns the scancode |
| `ClearKeys` | — | — | Clears key states; ignores keyboard events until the next frame |
| `EscapeKey` | — | `Integer` | 1 if Escape is held (level). Only observable when `SafeExit` is OFF — with the default `SafeExit` ON, Escape stops the program before its held state is recorded |
| `LeftKey` / `RightKey` / `UpKey` / `DownKey` | — | `Integer` | 1 if the arrow key is held (level) |

### Mouse

Buttons are 1-based: **1=left, 2=right, 3=middle**, 4+ extra.

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `MouseDown` | `button: Integer` | `Integer` | 1 if held (level) |
| `MouseHit` | `button: Integer` | `Integer` | 1 if pressed this frame (edge) |
| `MouseUp` | `button: Integer` | `Integer` | 1 if released this frame (edge) |
| `GetMouse` | — | `Integer` | Next queued button-down event (0 if none) |
| `WaitMouse` | — | `Integer` / — | Blocks until a button is pressed; function form returns the button |
| `MouseX` | — | `Integer` | Mouse X (screen space) |
| `MouseY` | — | `Integer` | Mouse Y (screen space) |
| `MouseWX` | — | `Float` | Mouse X in world space (current camera) |
| `MouseWY` | — | `Float` | Mouse Y in world space (current camera) |
| `MouseZ` | — | `Integer` | Mouse wheel position (accumulated) |
| `MouseMoveX` | — | `Integer` | X movement since the last call |
| `MouseMoveY` | — | `Integer` | Y movement since the last call |
| `MouseMoveZ` | — | `Integer` | Wheel movement since the last call |
| `PositionMouse` | `x: Integer, y: Integer` | — | Moves the cursor to screen coords |
| `ShowMouse` | `mode: Integer` | — | 0=hide, 1=standard cursor, `>1`=use image id as cursor |
| `ClearMouse` | — | — | Clears mouse button states for the rest of the frame |

### Text input

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `Input` | `prompt: String [, mask: String]` | `String` | Interactive on-screen text entry; optional `mask` replaces typed characters (e.g. `"*"`; default off) |
| `CloseInput` | — | — | Destroys the active input field |
| `SafeExit` | `flag: Integer` | — | Enabled by default; when on, pressing Escape stops the program immediately instead of acting as a normal key. When off, Escape is readable via `EscapeKey` (`SAFEEXIT` in source) |

### Scancode table

CB scancode → key (DirectInput-style; gaps are unmapped). Scancodes 69/197 are
NumLock/Pause, matching DirectInput and the real CoolBasic `cbKey*` constants:

| Scan | Key | Scan | Key | Scan | Key |
|-----:|-----|-----:|-----|-----:|-----|
| 1 | Esc | 30–38 | A,S,D,F,G,H,J,K,L | 71–73 | Numpad 7–9 |
| 2–11 | 1,2,…,9,0 | 39 | `\`` | 74 | Numpad − |
| 12 | =/+ | 40 | `'` | 75–77 | Numpad 4–6 |
| 13 | [ | 41 | `\` | 78 | Numpad + |
| 14 | Backspace | 42 | LShift | 79–81 | Numpad 1–3 |
| 15 | Tab | 43 | / | 82 | Numpad 0 |
| 16–25 | Q,W,E,R,T,Y,U,I,O,P | 44–50 | Z,X,C,V,B,N,M | 83 | Numpad Del |
| 26 | ] | 51 | , | 87–88 | F11–F12 |
| 27 | ; | 52 | . | 156 | Numpad Enter |
| 28 | Enter | 53 | −/_ | 157 | RCtrl |
| 29 | LCtrl | 54 | RShift | 181 | Numpad / |
| 55 | Numpad * | 56 | Alt | 183 | PrintScreen |
| 57 | Space | 58 | CapsLock | 184 | AltGr |
| 59–68 | F1–F10 | 69 | NumLock | 197 | Pause |
| 199 | Home | 200 | Up | 201 | PgUp |
| 203 | Left | 205 | Right | 207 | End |
| 208 | Down | 209 | PgDn | 210 | Insert |
| 211 | Delete | 219/220 | LWin/RWin | 221 | Menu |

---

## File I/O

**Implemented (`runtime/cb_file.cpp`).** `File` is the opaque `CbFile*`
handle (catalog tag 16): a declared-but-unassigned `File` is `Null`, and a failed
open returns `Null` — not an integer id (classic CB used integer file ids). The
subsystem is Allegro-free, so it is present in the SDK-free catalog and runs
headless. Deliberate divergences from classic CoolBasic, all for
safety/correctness:

- **Lenient at end-of-data.** A read at/past EOF returns a zero value
  (`0`/`0.0`/`""`) and zero-fills any missing bytes of a multi-byte read; `EOF`
  stays the guard. Classic CB returned uninitialised garbage (and `ReadByte`
  returned 255 at the end).
- **Traps on misuse.** A null/closed/invalid `File`, or a wrong-mode op (writing
  a read handle / reading a write handle), raises a runtime error (exit 1).
  Classic CB is permissive.
- **Little-endian on the wire**, independent of host byte order (byte-compatible
  with classic x86 CB files). `ReadByte`/`ReadShort` are unsigned, `ReadInt`
  signed, and `ReadFloat`/`WriteFloat` are 32-bit on disk.
- **`ReadString`** reads exactly the 32-bit length prefix's bytes (preserving
  embedded NULs) and guards a negative/over-long prefix; **`ReadLine`** strips
  LF, CR, or CRLF (classic CB only broke on CR/EOF, mis-reading Unix files).
- **On-disk string content is raw UTF-8** (the runtime string ABI), vs classic
  CB's CP1252 — identical for ASCII, different for non-ASCII content.
- **`FindFile`** returns real entries only (no `"."`/`".."`), `""` when done,
  over a single global, non-reentrant cursor on the current directory.
  **`CurrentDir`** keeps a trailing separator. **`CopyFile`** traps if the
  destination already exists. **`Execute`** shells out via `start` (Windows) /
  `xdg-open` (elsewhere).

A `File` is returned into a `File` variable; compare against `Null` to detect a
failed open.

### Open / close / position

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `OpenToRead` | `path: String` | `File` | Opens for reading |
| `OpenToWrite` | `path: String` | `File` | Opens for writing (creates/truncates) |
| `OpenToEdit` | `path: String` | `File` | Opens for read/write (creates if missing) |
| `CloseFile` | `f: File` | — | Closes a handle |
| `SeekFile` | `f: File, pos: Integer` | — | Seeks to an absolute byte offset |
| `FileOffset` | `f: File` | `Integer` | Current byte offset |
| `EOF` | `f: File` | `Integer` | Non-zero at end of file |

### Binary read / write

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `ReadByte` | `f: File` | `Integer` | Unsigned 8-bit |
| `ReadShort` | `f: File` | `Integer` | Unsigned 16-bit |
| `ReadInt` | `f: File` | `Integer` | Signed 32-bit |
| `ReadFloat` | `f: File` | `Float` | 32-bit float |
| `ReadString` | `f: File` | `String` | 32-bit length prefix + bytes |
| `ReadLine` | `f: File` | `String` | Reads to CR/LF, stripping the terminator |
| `WriteByte` | `f: File, v: Integer` | — | Unsigned 8-bit |
| `WriteShort` | `f: File, v: Integer` | — | 16-bit |
| `WriteInt` | `f: File, v: Integer` | — | 32-bit |
| `WriteFloat` | `f: File, v: Float` | — | 32-bit float |
| `WriteString` | `f: File, s: String` | — | 32-bit length prefix + bytes |
| `WriteLine` | `f: File, s: String` | — | String + OS line ending (CRLF on Windows) |

### Filesystem & directory

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `FileExists` | `path: String` | `Integer` | Non-zero if the path exists |
| `IsDirectory` | `path: String` | `Integer` | Non-zero if the path is a directory |
| `FileSize` | `path: String` | `Integer` | Size in bytes (0 if missing or a directory) |
| `CurrentDir` | — | `String` | Working directory (trailing separator) |
| `ChDir` | `path: String` | — | Changes the working directory |
| `MakeDir` | `path: String` | — | Creates a directory |
| `CopyFile` | `src: String, dst: String` | — | Copies a file (fatal error if `dst` exists) |
| `DeleteFile` | `path: String` | — | Deletes a file |
| `Execute` | `cmd: String` | — | Launches an external command (`start`/`xdg-open`) |
| `StartSearch` | — | — | Begins iterating the current directory |
| `FindFile` | — | `String` | Next entry (`"."`, `".."`, …); empty string when done |
| `EndSearch` | — | — | Ends directory iteration |

---

## Memory Blocks

(Source names use `MEMBlock`; CoolBasic spelling is `MakeMEMBlock` etc.)

**Implemented (`runtime/cb_memblock.cpp`).** `Memblock` is the opaque
`CbMemblock*` handle (catalog tag 15) — a raw-pointer opaque handle with no
numeric id space; a declared-but-unassigned `Memblock` is `Null`. The subsystem
is Allegro-free, so it is present in the SDK-free catalog and runs headless.
Deliberate divergences from classic CoolBasic:

- **Bounds/handle safety traps.** An out-of-range offset, a null/invalid handle,
  a negative `MakeMEMBlock`/`ResizeMEMBlock` size, or a bad `MemCopy` range
  raises a runtime error (exit 1) instead of the classic blind-cast that walks
  off the buffer (undefined behaviour).
- **Little-endian on the wire.** Multi-byte `Peek`/`Poke` (`Short`/`Int`/`Float`)
  use little-endian byte order regardless of host architecture, so a memblock's
  contents are platform-independent.
- **`PeekByte`/`PeekShort` return unsigned** (`0..255` / `0..65535`); `PeekInt`
  is a signed 32-bit reinterpret.
- **`Float` is 32-bit on the wire** — `PokeFloat` narrows the CB `Float` (f64) to
  IEEE-754 32-bit and `PeekFloat` widens it back.
- **`ResizeMEMBlock`** preserves existing bytes and zero-fills any growth;
  `MemCopy` uses `memmove`, so an in-block copy with overlapping ranges is
  well-defined.

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `MakeMEMBlock` | `size: Integer` | `Memblock` | Allocates a zero-filled block of `size` bytes |
| `DeleteMEMBlock` | `mem: Memblock` | — | Frees a block |
| `ResizeMEMBlock` | `mem: Memblock, size: Integer` | — | Resizes, preserving existing bytes and zero-filling growth |
| `MEMBlockSize` | `mem: Memblock` | `Integer` | Size in bytes |
| `MemCopy` | `srcMem: Memblock, srcOff: Integer, dstMem: Memblock, dstOff: Integer, len: Integer` | — | Copies `len` bytes between blocks |
| `PeekByte` | `mem: Memblock, offset: Integer` | `Integer` | Reads 8-bit unsigned |
| `PeekShort` | `mem: Memblock, offset: Integer` | `Integer` | Reads 16-bit unsigned |
| `PeekInt` | `mem: Memblock, offset: Integer` | `Integer` | Reads 32-bit signed |
| `PeekFloat` | `mem: Memblock, offset: Integer` | `Float` | Reads 32-bit float |
| `PokeByte` | `mem: Memblock, offset: Integer, value: Integer` | — | Writes 8-bit |
| `PokeShort` | `mem: Memblock, offset: Integer, value: Integer` | — | Writes 16-bit |
| `PokeInt` | `mem: Memblock, offset: Integer, value: Integer` | — | Writes 32-bit |
| `PokeFloat` | `mem: Memblock, offset: Integer, value: Float` | — | Writes 32-bit float |

---

## User-Defined Types

Instances of a `Type ... EndType` form a per-type linked list.

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `New` | `type: Type` | `TypeMember` | Creates a new instance at the end of the type's list |
| `First` | `type: Type` | `TypeMember` | First instance (or null) |
| `Last` | `type: Type` | `TypeMember` | Last instance (or null) |
| `After` | `member: TypeMember` | `TypeMember` | Next instance (or null) |
| `Before` | `member: TypeMember` | `TypeMember` | Previous instance (or null) |
| `Insert` | `member: TypeMember, target: TypeMember` | — | Moves `member` to sit **after** `target` in the list (special case: if `target` is the first member, `member` becomes the new first member) |
| `Delete` | `member: TypeMember` | — | Removes an instance from the list and frees it |
| `ConvertToInteger` | `member: TypeMember` | `Integer` | Stable integer id for a type pointer (so it can live in integer arrays) |
| `ConvertToType` | `id: Integer` | `TypeMember` | Recovers the type pointer from a `ConvertToInteger` id |

---

## Arrays

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `SortArray` | `arr: Integer[]` | — | Sorts a 1D array in ascending order |

> `Dim`, `ReDim`, and `ClearArray` are language constructs handled by the
> compiler/bytecode rather than runtime library calls.

---

## DATA, Encryption & Advanced

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `Read` | — | `Integer`\|`Float`\|`String` | Reads the next value from `DATA` statements; type matches the data |
| `Restore` | `label` | — | Resets the `DATA` read pointer to a label |
| `Encrypt` | `src: String\|Integer, dst: String\|Integer, password: String` | — | Byte-adds a repeating password to each byte of a file or memblock (additive cipher, not XOR) |
| `Decrypt` | `src: String\|Integer, dst: String\|Integer, password: String` | — | Inverse of `Encrypt` (subtracts the repeating password from each byte) |
| `CallDLL` | `dll: String, func: String, memIn: Memblock, memOut: Memblock` | — | Loads a DLL (cached) and calls `func`, passing in/out memblocks (Windows) |
| `SaveProgram` | — | — | **Stub** — not implemented |
| `LoadProgram` | — | — | **Stub** — not implemented |
| `GotoSavedLocation` | — | — | **Stub** — not implemented |

---

## Notes

- **Overload resolution.** Many entries have multiple overloads (e.g. `Color`
  3/4 args, `Abs` int/float). The compiler resolves these by parameter count and
  type; the runtime sees a fixed arity.
- **Statements vs functions.** Entries with `Returns —` are CB *commands*
  (statements). The rest are *functions* returning a value.
- **Coordinate system.** Graphics use a top-left origin with Y increasing
  downward. World coordinates (camera space) invert Y relative to screen space.
- **Angles** are degrees everywhere.
- **1-based** string and tilemap indices.
- **Code-point strings.** See [string semantics](#string-semantics) — strings
  are Unicode code points (UTF-8 storage), not single-byte CP-1252.

### Collision model

- **Types**: 1 = box (AABB; `ObjectRange(obj, w, h)`), 2 = circle
  (`ObjectRange(obj, diameter)`), 4 = map (tilemap walls; target only), 3 = pixel
  (declared, not fully implemented).
- **Handling**: 0 = report only, 1 = stop (circle only), 2 = slide.
- Each recorded collision exposes the other object, a contact point `(X, Y)`, and
  a contact angle, queried via `CountCollisions`/`GetCollision`/`CollisionX/Y/Angle`
  using **1-based** indices (1..`CountCollisions`).

---

## Implementation status in cbcompiler_rs

The sections above describe the **full CoolBasic runtime surface**. cbcompiler_rs
currently implements a subset, and in places diverges intentionally. Known status
and divergences as of the latest runtime work:

- **String semantics.** Strings are **UTF-8** and all char-indexed ops count
  Unicode **code points** (a deliberate divergence from classic CoolBasic's
  single-byte CP-1252 byte counting). Out-of-range arguments **clamp** rather
  than error (`Left("hi",5)`→`"hi"`; `n<=0`→`""`). `Upper`/`Lower` are ASCII-only
  for now. The **full String surface is implemented**. `InStr` not-found returns
  `0` (per spec). Code-point-specific notes: `Flip` reverses by code point (valid
  UTF-8 out); `Asc` returns the first **code point** (inverse of `Chr`), not a
  0–255 byte; `StrInsert` uses proper 1-based indexing; `String(s,n)` with `n<1`
  still yields one copy.
- **Math.** Full surface implemented. `Rand(low,high)` is **inclusive**
  `[low,high]` and `Rnd(low,high)` is `[low,high)`; the `high < low` branch is the
  documented special case (Rnd → `randf()*low`, Rand → `rand(low)`), not a swap.
  `Int(Float)` rounds to the **nearest integer, ties away from zero** (`f64::round`,
  so `10.5 → 11` and `-1.5 → -2`), and `Int(String)` trims then parses a leading
  integer — both via the interpreter's `convert_value` (so explicit `Int()` and
  implicit Float→Int coercion agree). This is an intentional divergence from
  classic CoolBasic, whose `Int(Float)` adds `0.5` then truncates toward zero
  (`-1.5 → -1`, and quirkily `-1.4 → 0`).
- **System / Time.** `Timer` returns monotonic ms since first call
  (`std::chrono::steady_clock`; the legacy used CPU-time `clock()`). `Wait(ms)`
  sleeps (`ms <= 0` is a no-op). `End` is a language statement lowered to an IR
  `Halt`; `MakeError(msg)` writes to stderr and exits with code 1. `Date`
  (`"D Mon YYYY"`), `Time` (`"HH:MM:SS"`), `CommandLine`, and `GetEXEName` are
  implemented; the last two read the running `cb` process. `FrameLimit`/`Errors`/
  `SetWindow`/`Crc32` remain deferred (window/loop/error-display plumbing).
- **Graphics / images** (Allegro 5, `runtime/cb_gfx.cpp`). `Image` wraps an
  Allegro bitmap. `DeleteImage` exists (the legacy leaked image handles until
  exit). `Circle`'s argument is a diameter; `GetPixel`/`PutPixel` packed form uses
  32-bit ARGB. `MakeImage`/`LoadImage` work before any `Screen()` (memory-bitmap
  fallback). Implemented: `Screen(w,h,depth,mode)` (depth ignored — always 32-bit)
  and the no-arg `Screen()` (buffer id 0), `ScreenDepth` (32), `GFXModeExists`
  (best-effort: any positive mode → 1), `DrawScreen(cls,vsync)`, `GetRGB`,
  `PickColor` (reads the current target), `Smooth2D` (linear filtering on new
  bitmaps), `Ellipse`, `CopyBox` (current target only), `PutPixel2`/`GetPixel2`
  (ARGB aliases; the buffer-id arg models only the current target). `ScreenGamma`
  is stored but not applied and `ScreenShot` is a no-op without a window
  (Allegro 5 has no portable display-gamma ramp). Image additions:
  `CloneImage`, `ResizeImage`, `RotateImage` (rotated bounding box, centered
  hotspot), `PickImageColor`/`PickImageColor2`, `SaveImage` (`frame` ignored),
  `DrawGhostImage` (alpha 0–100), `DrawImageBox`, `DefaultMask`, `ImagesOverlap`
  (AABB), and `ImagesCollide` (pixel-precise via non-zero alpha; `frame` args
  ignored). `MakeImage` now clears to opaque black (defined contents). `CbImage`
  gained a **hotspot** (default top-left); `HotSpot(img, x, y)` is the per-image
  form (x<0 || y<0 auto-centers) — classic CoolBasic's integer-id `0`/`1` default
  toggle has no analogue since `Image` is an opaque handle, not an int id.
- **Multi-frame sprite sheets** (`runtime/cb_gfx.cpp`). `LoadAnimImage` slices an
  `Image` into `frameW × frameH` cells; the `frame` parameter is honored by
  `DrawImage`/`DrawGhostImage`/`DrawImageBox` (overloaded per arity — the catalog
  has no default-arg mechanism). `frame` is 0-based, taken `% framesX`, and **not
  clamped**; an image with no anim params (`anim_length == 0`) ignores `frame` and
  draws whole (single-frame fallback). `MakeImage(w, h, frameCount)` accepts but
  **ignores** `frameCount` (no frame size to slice by → stays single-frame). The
  slice math uses `row = frame / framesX` and `top = row * frameHeight`. **`useMask`
  on `DrawImage`/`DrawImageBox` is accepted but ignored** — masking here is
  destructive (`MaskImage` bakes alpha into the single bitmap, leaving no unmasked
  copy to select between). `SaveImage`/`ImagesCollide` `frame` args stay inert.
  `HotSpot(-1, -1)` centers on a single frame when a frame size is set, else the
  whole image. `anim_begin`/`startFrame` is stored but never read. `LoadAnimImage`
  shares `MakeImage`'s memory-bitmap fallback so sheets load without a display.
- **Input** (`runtime/cb_input.cpp`). Keyboard
  (`KeyDown`/`KeyUp`/`KeyHit`/`EscapeKey`) and the scancode table are implemented.
  Input advances per frame inside `DrawScreen`. The mouse functions
  (`MouseDown`/`MouseHit`/`MouseUp`/`MouseZ`/`MouseMove*`) are built on Allegro's
  mouse model. `EscapeKey` is a pure query (no legacy auto-exit). Also implemented:
  `GetKey` (typed-char queue), `LeftKey`/`RightKey`/`UpKey`/`DownKey`,
  `ClearKeys`/`ClearMouse` (swallow events until the next frame), `GetMouse`
  (button-down queue), `PositionMouse`, `ShowMouse` (0=hide/1=show; image cursors
  unsupported — `Image` is an opaque handle, not an int id), and `WaitKey`/
  `WaitMouse` (block on the window event queue; with no window they return 0
  immediately rather than hang). `MouseWX`/`MouseWY` and `Input`/`CloseInput`/
  `SafeExit` remain deferred (camera / interactive on-screen entry).
- **Text / fonts** (Allegro font/ttf addons, `runtime/cb_gfx.cpp` +
  `runtime/cb_font.cpp`). `Font` is an opaque handle (like `Image`). Immediate
  `Text`/`CenterText`/`VerticalText` draw in the current color onto the active
  target; the `Locate`/`AddText`/`ClearText` queue re-renders every `DrawScreen`
  until cleared. `LoadFont` takes a system family name (resolved via a Windows
  font table / fontconfig on Linux) or a file path (name containing a `.`);
  `Smooth2D` toggles antialiased vs monochrome glyphs; `underline` is accepted but
  not rendered (a known TODO). The default font is Courier New 12pt, falling back
  to Allegro's built-in 8×8 font so `Text`/`TextWidth`/`TextHeight` never crash
  and work headless. `VerticalText` is documented as `(x, y, s)`.
- **Pixel-precise ARGB.** `PutPixel`/`GetPixel` use packed 32-bit **ARGB** (vs
  the `0xRRGGBB` packing classic CoolBasic used). `Lock`/`Unlock` (current target
  or an explicit `Image`, with the optional read/write/`mode` access flag) bracket
  them — the access-mode form is a `Lock` overload, not a separate `Lock2`.
- **Camera** is implemented: the world↔screen transform core (`PositionCamera`,
  `MoveCamera`, `TranslateCamera`, `RotateCamera`, `TurnCamera`,
  `CameraX`/`Y`/`Angle`), `DrawToWorld` (wired into every user draw command), and
  `MouseWX`/`MouseWY`. The object-referencing camera funcs (`PointCamera`,
  `CameraFollow`, `CloneCameraPosition`/`Orientation`, `CameraPick`) are
  implemented with two bug fixes over classic CoolBasic: `PointCamera` aims with
  the object's X (not Y twice), and `CloneCameraOrientation` sets **both** angle
  fields (classic CB left the render matrix desynced). `CameraFollow`'s style-2
  deadzone uses the physical window size; the follow step runs once per
  `DrawScreen`. The camera keeps two independent angle fields — `CameraAngle`
  (degrees, also driving `MoveCamera`'s heading) and the render-matrix angle —
  which `RotateCamera`/`TurnCamera` set from separate args and may diverge.
- **Tile maps** (`runtime/cb_map.cpp`) are implemented: a single active tilemap
  (`LoadMap`/`MakeMap`, `MapWidth`/`MapHeight`, `GetMap`/`GetMap2`, `EditMap`,
  `SetMap`, `SetTile`) with the four-layer model and the `.til` binary format,
  rendered in world space via the camera. The `.til` format was byte-verified
  against a real CoolBasic asset (`testmap.til`). Defensive divergences from
  classic CoolBasic: all funcs null-guard the active map (classic CB null-derefs),
  layer indices are bounds-checked (0 / no-op out of range), and `SetTile`'s
  array-grow bug is fixed. The map renders inside the object draw order, and
  layer 2 backs object map-collision (type 4) and `ObjectSight` (a DDA wall walk).
  Tile animation advances on the game-loop update tick — a deterministic
  **frame-step** (a fixed step per tick so headless runs reproduce).
- **Objects / sprites** (`runtime/cb_object.cpp`) are implemented:
  creation/lifecycle (`LoadObject`/`LoadAnimObject`/`MakeObject`/`MakeObjectFloor`/
  `CloneObject`/`DeleteObject`/`ClearObjects`), position/movement, rotation/angle
  (incl. `GetAngle2`/`Distance2`/`PointObject`), appearance (`PaintObject` — three
  handle-typed overloads — `MaskObject`/`GhostObject`/`MirrorObject`/`ShowObject`/
  `DefaultVisible`/`ObjectOrder`/`ObjectSizeX`/`Y`), animation (`PlayObject`/
  `LoopObject`/`StopObject`/`ObjectPlaying`/`ObjectFrame`), custom data slots
  (`ObjectInteger`/`Float`/`String`), `ObjectLife`, and enumeration
  (`InitObjectList`/`NextObject`). `Object` is the opaque tag-13 handle (no integer
  ids; the registry is creation-order + per-draw-chain vectors). Textures are
  **shared and reference-counted**: `CloneObject` shares the bitmap (resets pos/
  angle, forces `visible=true`); `PaintObject`/`MaskObject` mutate the shared
  bitmap in place so all clones see the change; `MirrorObject` repoints the one
  object to a fresh private bitmap. The render pass follows the CoolBasic draw
  order under one world transform — map background → floor objects → regular
  objects → map foreground.
- **Collision, picking & game loop** (`runtime/cb_object.cpp` +
  `runtime/cb_collision_data.h`) are implemented. **Collision**: `SetupCollision`
  is a *persistent* registration re-tested every update tick (object-object, plus a
  `Map`-handle overload for type-4 map walls); box-box, circle-circle, box-map and
  circle-map geometry with report/stop/slide handling; `ObjectRange`,
  `ResetObjectCollision`, `ClearCollisions`, `CountCollisions`, the 1-based
  `GetCollision`/`CollisionX`/`Y`/`Angle`, and `ObjectsOverlap`. Inherited
  classic-CoolBasic quirks: `Stop` handling is circle-only, box↔circle object pairs
  never collide (classic CoolBasic's dead `CircleRect`/`RectCircle` tests),
  `MakeObject`/`MakeObjectFloor` leave `ObjectRange` 0×0 (so their collisions are
  inert until set), and pixel overlap is unimplemented (→ 0). `GetCollision`
  returns an `Object` handle (or `Null`), never an integer; a map-wall hit yields
  `Null` (a `Map` is not an `Object`). **Picking**: `ObjectPickable`/`ObjectPick`
  (nearest raycast hit), `PickedObject`/`X`/`Y`/`Angle` (`PickedAngle` reports
  degrees-from-hit, a fix over classic CoolBasic's stale radians), `PixelPick`
  (registered no-op stub), `ObjectSight`, and `ScreenPositionObject`/`CameraPick`
  (screen→world then test). **Game loop**: `UpdateGame`/`DrawGame` run the built-in
  update/draw with `gameUpdated`/`gameDrawn` dedup against `DrawScreen`'s implicit
  pass — there are **no user CB callbacks**. The update tick advances animation and
  `ObjectLife` (auto-deleting at 0), wipes per-frame collision lists, steps
  map-tile animation, and re-runs every collision check.
- **Particle emitters** (`runtime/cb_object.cpp` + `runtime/cb_particle.h`) are
  implemented: `MakeEmitter`/`ParticleMovement`/`ParticleEmission`/
  `ParticleAnimation`. An emitter is a `CbObject` carrying an emitter payload (the
  kind discriminator), so it reuses the `Object` type (tag 13) — every object
  command drives it, with zero frontend/catalog-type changes. It renders its
  particles and steps them on the update tick, defers `DeleteObject` until the
  particles drain, and is **excluded from picking and collision** (real CB does
  neither, contradicting the Help docs). The pure simulation (uniform launch
  direction, gravity/acceleration integration, cull, forward+clamped animation
  frame) lives in the Allegro-free `cb_particle.h`. Safety and correctness
  behaviors: a non-emitter handle traps instead of being blind-cast (classic CB's
  UB), animation plays forward per the Help, there is no `StopEmitting`, and a
  non-positive emission density spawns nothing.
- **Not yet implemented** in cbcompiler_rs: video playback, `Read`/`Restore`,
  `Encrypt`/`Decrypt`, `CallDLL`, and the plumbing-heavy System funcs (`Crc32`,
  `SetWindow`, `FrameLimit`, `Errors`). (Sound, file I/O, and memblocks are
  implemented.)

### Runtime library architecture (cbcompiler_rs)

The cbcompiler_rs C++ runtime is split into two static libraries with a strict,
one-directional dependency (functionality → core; core depends on nothing
functional):

- **`cb_runtime_core`** — the irreducible, plugin-facing ABI: the opaque
  `CbString` type and its primitives, the `CbStringApi` table, the catalog
  descriptor structs (`CbTypeTag`, `CbTypeDesc`, `CbParamDesc`, `CbFuncDesc`,
  `CbCatalog`), `CB_CATALOG_VERSION`, and `cb_runtime_get_catalog`, plus the
  `CbHostApi`/`CbRuntimeHooks`/`cb_runtime_init` trap-channel handshake.
  **Zero Allegro dependency.** Header: `runtime/cb_runtime_core.h`; single TU:
  `runtime/cb_string.cpp`.
- **`cb_runtime`** (functionality) — the feature subsystems built on core: the
  catalog assembly, the String library (`cb_strfuncs.cpp`), Math, System/Time,
  Graphics, Input. Links `cb_runtime_core` plus the full Allegro closure. Header:
  `runtime/cb_runtime_func.h`.

`runtime/cb_runtime.h` remains a back-compat umbrella including both narrow
headers; it will be removed once every TU migrates.

#### Plugin ABI contract

Plugins (separate DLLs) handle `String` parameters by **statically
linking `cb_runtime_core`** and calling its primitives directly — no
function-pointer indirection on the hot path. This relies on value-based
`CbString` identity across modules:

- **Shared heap required.** A `CbString*` allocated in one module and freed in
  another must hit the same `malloc`/`free`. Plugins **must** use the same
  dynamic CRT (`/MD`; the `x64-windows-static-md` triplet guarantees one shared
  ucrt heap) and the same `CB_CATALOG_VERSION`.
- **Identity is value-based, never address-based.** Emptiness is `len == 0`;
  immortality (retain/release no-op) is `refcount < 0`. The single
  `CB_EMPTY_STRING_INSTANCE` address is only an intra-module shortcut — never
  compare a string pointer against `cb_runtime_string_api.empty` for correctness.

#### SDK-free build

`cb-runtime-sys` builds on machines that have **only a Rust toolchain** — no
CMake, vcpkg, or Allegro SDK — so `cargo test --workspace` runs the interpreter
(the reference implementation) and the driver fixtures anywhere, including CI and
cloud sessions. It does this by leaning on the runtime's two-library split: the Allegro-free TUs
(`cb_string.cpp`, `cb_host.cpp`, `cb_math.cpp`, `cb_strfuncs.cpp`,
`cb_system.cpp`) plus `catalog.cpp` compiled with **`-DCB_NO_ALLEGRO`** are built
directly via the `cc` crate. Under that define, `catalog.cpp` guards out only the
graphics/text/input `CB_FN` rows (and the `Image`/`Font` type entries) — the sole
things that would otherwise force a link against the Allegro closure — leaving a
real `cb_runtime_get_catalog` for every language-core function (`Print`, `Abs`,
Math, the String library, System/Time) with the **same** string implementation.
There is no second, divergent mock of the string primitives.

`build.rs` picks the path automatically:

| Situation | Path |
|-----------|------|
| `cmake` present and configures cleanly | full Allegro build (CMake), complete catalog |
| `cmake` absent, or configure/build fails | SDK-free `cc` build (language-core catalog) |
| `CB_RUNTIME_FORCE_SDK_FREE=1` | SDK-free `cc` build, no probing |
| `CB_RUNTIME_REQUIRE_ALLEGRO=1` | full build, **fatal** if it fails (no fallback) |

(The two env vars are mutually exclusive.) When the SDK-free path is taken,
`build.rs` emits a `cargo:warning` so the downgrade is visible, and exposes
`cb_runtime_sys::HAS_GRAPHICS = false`. Tests that exercise graphics/input/text
gate on `HAS_GRAPHICS` and skip cleanly rather than failing on absent catalog
entries.
