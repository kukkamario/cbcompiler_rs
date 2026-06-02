# CoolBasic Runtime Library Reference

This documents the **complete CoolBasic runtime surface** as implemented by
**cbEnchanted** — a from-scratch reimplementation of the CoolBasic interpreter
that is a verified drop-in replacement for the original. cbEnchanted is the
**reference / implementation target** for the cbcompiler_rs runtime; where the
two disagree, cbEnchanted is authoritative for the *language surface* (cbcompiler_rs
may still differ in internal representation — see [Implementation status](#implementation-status-in-cbcompiler_rs)).

> Earlier versions of this file described the much smaller CBCompiler runtime
> (the `CBF_`-prefixed surface). That implementation was incomplete; this file
> now reflects the cbEnchanted surface (~345 commands and functions).

## How the runtime is called

In cbEnchanted each runtime entry point is a C++ method named `commandXxx`
(a **statement**, no return value) or `functionXxx` (returns a value). The
CB-visible name is the suffix (`commandRandomize` → `Randomize`,
`functionGetAngle` → `GetAngle`). Arguments are passed on a value stack; the
compiler pushes them left-to-right, so a function reads them in reverse.

Throughout this document:

- **Returns `—`** means the entry is a statement (CB *command*) with no value.
- Parameters are listed in CB source order (left-to-right as written in code).
- Many commands have optional trailing parameters; the CoolBasic compiler
  supplies defaults for omitted ones, so the runtime always sees a fixed arity.

---

## Types

### Value types

| Type | Description |
|------|-------------|
| `Integer` | Signed 32-bit integer |
| `Float` | Single-precision 32-bit floating point |
| `String` | Byte string. cbEnchanted stores strings as **single-byte CP-1252 / Latin-1** internally (one byte per character); UTF-8 conversion happens only at I/O boundaries. See the [string semantics](#string-semantics) note. |

### Handle types

All handles are runtime-managed integer IDs; `0` conventionally means "invalid /
none". The user never sees their internals.

| Type | Description |
|------|-------------|
| `Image` | Bitmap/texture, optionally a multi-frame sprite sheet |
| `Font` | Loaded TrueType font |
| `Sound` | Preloaded audio sample |
| `Channel` | An active sound playback instance (returned by `PlaySound`) |
| `Object` | 2D sprite object (position, angle, scale, animation, collision) |
| `Map` | Tilemap |
| `Emitter` | Particle emitter |
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
| `Float` | `value: Integer` | `Float` | Converts a value to a float |
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
| `Rnd` | `low: Float, high: Float` | `Float` | Random float between `low` and `high`. If `high < low`, returns `randf() * low` (special-cased, not swapped) |
| `Rand` | `low: Integer, high: Integer` | `Integer` | Random integer between `low` and `high`. If `high < low`, returns `rand(low)` |
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
| `Mid` | `s: String, pos: Integer, len: Integer` | `String` | `len` characters starting at `pos` (1-based; `pos > 0`) |
| `Replace` | `s: String, find: String, repl: String` | `String` | Replaces **all** occurrences of `find`; empty `find` returns `s` unchanged |
| `InStr` | `s: String, find: String, start: Integer` | `Integer` | 1-based position of `find` at or after `start`; `0` if not found |
| `Upper` | `s: String` | `String` | Uppercase |
| `Lower` | `s: String` | `String` | Lowercase |
| `Trim` | `s: String` | `String` | Removes leading and trailing whitespace |
| `LSet` | `s: String, len: Integer` | `String` | Left-aligns into a field of width `len`, padding spaces on the right |
| `RSet` | `s: String, len: Integer` | `String` | Right-aligns into width `len`; truncates to rightmost `len` chars if longer |
| `Chr` | `code: Integer` | `String` | Single character from a byte value (0–255, CP-1252) |
| `Asc` | `s: String` | `Integer` | Byte value (0–255) of the first character (CP-1252) |
| `Len` | `s: String` | `Integer` | String length in **bytes** (== characters, since storage is single-byte) |
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

- **Single-byte storage.** cbEnchanted holds strings as CP-1252 / Latin-1, one
  byte per character. Consequently `Len`, `Left`, `Right`, `Mid`, `InStr`, etc.
  count **bytes**, and `Chr`/`Asc` map a single byte 0–255. UTF-8 is used only
  when reading/writing to the console or files.
  *(This differs from cbcompiler_rs, which currently treats strings as UTF-8 and
  counts Unicode codepoints — a known divergence.)*
- **1-based indexing** for all position arguments.
- **`InStr` not-found** returns `0`.

---

## System & Time

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `Timer` | — | `Integer` | Milliseconds, monotonic; suitable for measuring elapsed time |
| `Wait` | `ms: Integer` | — | Blocks for `ms` milliseconds (`al_rest`) |
| `Date` | — | `String` | Current date as a formatted string |
| `Time` | — | `String` | Current time as `HH:MM:SS` |
| `CommandLine` | — | `String` | Command-line arguments passed to the program |
| `GetEXEName` | — | `String` | Absolute path of the running executable |
| `FPS` | — | `Float` | Current frames-per-second |
| `FrameLimit` | `fps: Integer` | — | Caps the frame rate |
| `SetWindow` | `title: String, mode: Integer, confirm: String` | — | Sets window title; `mode`: 0=no change, 1=restore, 2=minimize, 3=maximize (Windows). `confirm` is shown if the user tries to close the window |
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
| `Box` | `x: Float, y: Float, w: Float, h: Float, fill: Integer` | — | Rectangle at top-left `(x,y)`; `fill`=0 outline, non-zero filled |
| `Circle` | `x: Float, y: Float, diameter: Float, fill: Integer` | — | Circle (input is a **diameter**); `fill`=0 outline, non-zero filled |
| `Ellipse` | `x: Float, y: Float, w: Float, h: Float, fill: Integer` | — | Ellipse (`w`,`h` are full diameters); `fill`=0 outline, non-zero filled |

### Pixel operations

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `PutPixel` | `x: Integer, y: Integer, color: Integer, buffer: Integer` | — | Writes packed `0xRRGGBB` to a target; `buffer`=0 means current target (`PutPixel2` is an alias) |
| `GetPixel` | `x: Integer, y: Integer, buffer: Integer` | `Integer` | Reads a pixel as packed `0xRRGGBB`; `buffer`=0 means current target (`GetPixel2` is an alias) |
| `Lock` | `buffer: Integer` | — | Locks a render target for direct pixel access (`buffer`=0 = current) |
| `Unlock` | `buffer: Integer` | — | Unlocks a render target, flushing pixel writes |

### Render targets

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `Image` | `imageId: Image, unused: Integer` | `Integer` | Render-target buffer id backing an image (the 2nd argument is unused) |
| `DrawToImage` | `imageId: Image` | — | Redirects subsequent drawing onto an image's bitmap |
| `DrawToScreen` | — | — | Redirects subsequent drawing back to the screen |
| `DrawToWorld` | `drawCommands: Integer, drawImages: Integer, drawText: Integer` | — | Flags controlling whether primitives / images / text render in world (camera) coordinates vs screen coordinates |
| `CopyBox` | `srcX, srcY, w, h, destX, destY: Float, srcBuf: Integer, destBuf: Integer` | — | Blits a rectangular region between render targets using the current blender |
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
| `MakeImage` | `w: Integer, h: Integer, frameCount: Integer` | `Image` | Creates a blank image (optionally multi-frame) |
| `CloneImage` | `img: Image` | `Image` | Copies an image and all properties |
| `ImageWidth` | `img: Image` | `Integer` | Image width (px) |
| `ImageHeight` | `img: Image` | `Integer` | Image height (px) |
| `DrawImage` | `img: Image, x: Float, y: Float, frame: Integer, useMask: Integer` | — | Draws a frame at `(x,y)`; `useMask` skips transparent pixels; honors `DrawToWorld` |
| `DrawGhostImage` | `img: Image, x: Float, y: Float, frame: Integer, alpha: Float` | — | Draws with alpha 0–100 (0=transparent, 100=opaque) |
| `DrawImageBox` | `img: Image, srcX: Float, srcY: Float, srcW: Float, srcH: Float, dstX: Float, dstY: Float, frame: Integer, useMask: Integer` | — | Draws a source sub-rectangle scaled to a destination size |
| `MaskImage` | `img: Image, r: Integer, g: Integer, b: Integer` | — | Sets the per-image transparent color key |
| `DefaultMask` | `enabled: Integer, r: Integer, g: Integer, b: Integer` | — | Sets the default mask color applied to future images |
| `HotSpot` | `id: Integer, x: Integer, y: Integer` | — | Sets rotation/scale origin: `id`=0 disable, 1 set default for future images, `>1` set for that image; `-1` auto-centers |
| `ResizeImage` | `img: Image, w: Integer, h: Integer` | — | Resizes an image |
| `RotateImage` | `img: Image, angle: Float` | — | Rotates the image bitmap (degrees, clockwise) |
| `PickImageColor` | `img: Image, x: Integer, y: Integer` | — | Reads a pixel from an image and makes it the draw color (`PickImageColor2` alias) |
| `SaveImage` | `img: Image, path: String, frame: Integer` | — | Writes an image (or one frame) to disk |
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
| `Print` | `s: String` | — | Writes `s` + newline to stdout (UTF-8; CP-1252 on Windows) |
| `Write` | `s: String` | — | Writes `s` to stdout without a newline |
| `Locate` | `x: Integer, y: Integer` | — | Sets the on-screen text cursor for `AddText` |
| `AddText` | `s: String` | — | Queues on-screen text at the cursor (current font/color), advancing one line |
| `ClearText` | — | — | Clears all queued on-screen text and resets the cursor |
| `Text` | `x: Float, y: Float, s: String` | — | Draws text immediately at `(x,y)` in the current font/color; honors `DrawToWorld` |
| `CenterText` | `x: Integer, y: Integer, s: String, style: Integer` | — | Centered text; `style`: 0=horizontal, 1=vertical, 2=both |
| `VerticalText` | `x: Integer, y: Integer, s: String` | — | Draws text one character per line, top-to-bottom |
| `LoadFont` | `name: String, size: Integer, bold: Integer, italic: Integer, underline: Integer` | `Font` | Loads a TrueType font by family name or file path; honors `Smooth2D`; 0 on failure |
| `SetFont` | `font: Font` | — | Sets the current font |
| `DeleteFont` | `font: Font` | — | Frees a font |
| `TextWidth` | `s: String` | `Integer` | Pixel width of `s` in the current font |
| `TextHeight` | `s: String` | `Integer` | Pixel height of `s` in the current font |

---

## Sound

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `LoadSound` | `path: String` | `Sound` | Loads an audio file; 0 on failure |
| `PlaySound` | `sound: Sound\|String, volume: Float, balance: Float, frequency: Integer` | `Channel` | Plays a sound (handle or path); returns a channel handle; the channel is freed when playback ends |
| `SoundPlaying` | `channel: Channel` | `Integer` | 1 if the channel is still playing |
| `SetSound` | `channel: Channel, looping: Integer, volume: Float, balance: Float, frequency: Integer` | — | Updates loop/volume/pan/pitch on an active channel |
| `StopSound` | `channel: Channel` | — | Stops a channel |
| `DeleteSound` | `sound: Sound` | — | Frees a preloaded sound |

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

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `MakeEmitter` | `img: Image, lifeTime: Integer` | `Emitter` | Creates a particle emitter using `img` as the particle sprite; `lifeTime` in ms |
| `ParticleEmission` | `emitter: Emitter, density: Float, count: Float, spread: Float` | — | Emission rate (per frame), particles per emission, and angular spread (degrees) |
| `ParticleMovement` | `emitter: Emitter, speed: Float, gravity: Float, acceleration: Float` | — | Initial speed, gravity, and per-frame acceleration |
| `ParticleAnimation` | `emitter: Emitter, frameCount: Integer` | — | Frame cycling for animated particle images |

---

## Objects (Sprites)

The object system represents 2D sprites with position, rotation, scale, optional
animation, custom data slots, and collision. Objects are integer handles. "Floor"
objects draw before regular objects (background layering). Positions are in world
space unless noted.

### Creation & destruction

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `LoadObject` | `path: String, rotation: Float` | `Object` | Loads a single-frame image as an object |
| `LoadAnimObject` | `path: String, frameW: Integer, frameH: Integer, startFrame: Integer, frameCount: Integer, rotation: Float` | `Object` | Loads a sprite-sheet object |
| `MakeObject` | — | `Object` | Creates an empty (imageless) object |
| `MakeObjectFloor` | — | `Object` | Creates a floor object (drawn before regular objects) |
| `CloneObject` | `obj: Object` | `Object` | Copies an object (image, position, orientation, frame) |
| `DeleteObject` | `obj: Object` | — | Deletes an object and clears its collisions |
| `ClearObjects` | — | — | Deletes all non-map objects |

### Position & movement

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `PositionObject` | `obj: Object, x: Float, y: Float` | — | Sets absolute world position |
| `ScreenPositionObject` | `obj: Object, sx: Float, sy: Float` | — | Sets position from screen coords (converted via camera) |
| `MoveObject` | `obj: Object, forward: Float, side: Float` | — | Moves relative to the object's facing angle |
| `TranslateObject` | `obj: Object, dx: Float, dy: Float` | — | Moves by an absolute world delta |
| `CloneObjectPosition` | `dst: Object, src: Object` | — | Copies `src`'s position to `dst` |
| `ObjectX` | `obj: Object` | `Float` | World X of the object center |
| `ObjectY` | `obj: Object` | `Float` | World Y of the object center |

### Rotation & angle

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `RotateObject` | `obj: Object, angle: Float` | — | Sets absolute rotation (degrees; 0°=right, 90°=down) |
| `TurnObject` | `obj: Object, speed: Float` | — | Rotates continuously at `speed` degrees per update |
| `PointObject` | `obj: Object, target: Object` | — | Rotates `obj` to face `target` |
| `CloneObjectOrientation` | `dst: Object, src: Object` | — | Copies `src`'s angle to `dst` |
| `ObjectAngle` | `obj: Object` | `Float` | Current rotation (degrees) |
| `GetAngle2` | `a: Object, b: Object` | `Float` | Angle from object `a` to object `b` |
| `Distance2` | `a: Object, b: Object` | `Float` | Distance between two object centers |

### Appearance

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `PaintObject` | `obj: Object, source: Integer` | — | Replaces the object's image; positive id = another object's image, negative = `-imageId` |
| `MaskObject` | `obj: Object, r: Integer, g: Integer, b: Integer` | — | Sets a transparent color key |
| `GhostObject` | `obj: Object, alpha: Float` | — | Sets alpha 0–100 (clamped) |
| `MirrorObject` | `obj: Object, direction: Integer` | — | Mirrors the image: 0=none, 1=horizontal, 2=vertical |
| `ShowObject` | `obj: Object, visible: Integer` | — | Shows/hides (hidden objects still update & collide) |
| `DefaultVisible` | `visible: Integer` | — | Default visibility for newly created objects |
| `ObjectOrder` | `obj: Object, direction: Integer` | — | Draw order: −1 = to back, 1 = to front |
| `ObjectSizeX` | `obj: Object` | `Integer` | Bounding width (accounts for rotation) |
| `ObjectSizeY` | `obj: Object` | `Integer` | Bounding height (accounts for rotation) |

### Animation

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `PlayObject` | `obj: Object, startFrame: Integer, endFrame: Integer, speed: Float, continuous: Integer` | — | Plays frames once; `endFrame = -1` stops and resets |
| `LoopObject` | `obj: Object, startFrame: Integer, endFrame: Integer, speed: Float, continuous: Integer` | — | Loops the frame range continuously |
| `StopObject` | `obj: Object` | — | Stops animation, keeping the current frame |
| `ObjectPlaying` | `obj: Object` | `Integer` | 1 if an animation is playing |
| `ObjectFrame` | `obj: Object` | `Float` | Current frame index (may be fractional) |

### Custom data slots

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `ObjectInteger` | `obj: Object [, value: Integer]` | `Integer` / — | Get/set a per-object integer slot |
| `ObjectFloat` | `obj: Object [, value: Float]` | `Float` / — | Get/set a per-object float slot |
| `ObjectString` | `obj: Object [, value: String]` | `String` / — | Get/set a per-object string slot |
| `ObjectLife` | `obj: Object [, ms: Integer]` | `Integer` / — | Get/set object lifetime (ms); auto-deletes at 0 |

### Collision

See the [collision model](#collision-model) note for types and handling modes.

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `SetupCollision` | `objA: Object, typeA: Integer, objB: Object, typeB: Integer, handling: Integer` | — | Registers a collision check. Types: 1=box, 2=circle, 4=map (B only). `handling`: 0=report, 1=stop (circle), 2=slide |
| `ObjectRange` | `obj: Object, range1: Float [, range2: Float]` | — | Collision bounds: box = width,height; circle = diameter |
| `ResetObjectCollision` | `obj: Object` | — | Clears recorded collisions for the object this frame |
| `ClearCollisions` | — | — | Removes all collision checks |
| `CountCollisions` | `obj: Object` | `Integer` | Number of collisions recorded this frame |
| `GetCollision` | `obj: Object, index: Integer` | `Object` | Colliding object by 0-based index (0 if none) |
| `CollisionX` | `obj: Object, index: Integer` | `Float` | Contact X of a collision |
| `CollisionY` | `obj: Object, index: Integer` | `Float` | Contact Y of a collision |
| `CollisionAngle` | `obj: Object, index: Integer` | `Float` | Contact normal angle (degrees) |
| `ObjectsOverlap` | `a: Object, b: Object, type: Integer` | `Integer` | One-shot overlap test: 1=box, 2=circle, 3=pixel (pixel not yet implemented) |

### Picking & line of sight

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `ObjectPickable` | `obj: Object, style: Integer` | — | Marks pickable: 0=off, 1=box, 2=circle, 3=pixel (pixel partial) |
| `ObjectPick` | `picker: Object` | — | Raycasts from `picker` along its facing angle; stores the nearest hit |
| `PickedObject` | — | `Object` | Object hit by the last `ObjectPick` (0 if none) |
| `PickedX` | — | `Float` | World X of the last pick hit |
| `PickedY` | — | `Float` | World Y of the last pick hit |
| `PickedAngle` | — | `Float` | Angle from picker to the pick point |
| `ObjectSight` | `a: Object, b: Object` | `Integer` | 1 if a clear line (no map walls) exists between two objects |

### Enumeration

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `InitObjectList` | — | — | Resets the object iterator |
| `NextObject` | — | `Object` | Next object id (0 at end); call `InitObjectList` first |

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
| `RotateCamera` | `angle: Float` | — | Sets absolute camera rotation (degrees) |
| `TurnCamera` | `degrees: Float` | — | Rotates by `degrees` (wraps 0–360) |
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
2=collision (always active), 3=unused. Tiles are referenced by **1-based** id in
game code (0 = empty). Only one map is active at a time.

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `LoadMap` | `mapPath: String, tilesetPath: String` | `Map` | Loads a `.map` file + tileset image; 0 on failure; replaces any existing map |
| `MakeMap` | `wTiles: Integer, hTiles: Integer, tileW: Integer, tileH: Integer` | `Map` | Creates an empty tilemap; replaces any existing map |
| `MapWidth` | — | `Integer` | Map width in tiles |
| `MapHeight` | — | `Integer` | Map height in tiles |
| `GetMap` | `layer: Integer, tx: Integer, ty: Integer` | `Integer` | Tile id at grid position (1-based; 0 if out of bounds) |
| `GetMap2` | `layer: Integer, wx: Float, wy: Float` | `Integer` | Tile id at world coordinates |
| `EditMap` | `layer: Integer, tx: Integer, ty: Integer, tile: Integer` | — | Sets a tile at a 1-based grid position (out-of-bounds ignored) |
| `SetMap` | `backLayer: Integer, overLayer: Integer` | — | Toggles visibility of the background (0) and foreground (1) layers |
| `SetTile` | `tile: Integer, animLength: Integer, animSlowness: Integer` | — | Configures per-tile animation (frame count + slowness) |

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
| `GetKey` | — | `Integer` | Next queued character code (CP-1252), or 0 |
| `WaitKey` | — | `Integer` / — | Blocks until a key is pressed; function form returns the scancode |
| `ClearKeys` | — | — | Clears key states; ignores keyboard events until the next frame |
| `EscapeKey` | — | `Integer` | 1 if Escape is held (level) |
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
| `Input` | `prompt: String, mask: String` | `String` | Interactive on-screen text entry; `mask` replaces typed characters (e.g. `"*"`) |
| `CloseInput` | — | — | Destroys the active input field |
| `SafeExit` | `flag: Integer` | — | When enabled, pressing Escape triggers a confirmed exit instead of acting as a normal key (`SAFEEXIT` in source) |

### Scancode table

CB scancode → key (DirectInput-style; gaps are unmapped):

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
| 59–68 | F1–F10 | 69 | Pause | 197 | NumLock |
| 199 | Home | 200 | Up | 201 | PgUp |
| 203 | Left | 205 | Right | 207 | End |
| 208 | Down | 209 | PgDn | 210 | Insert |
| 211 | Delete | 219/220 | LWin/RWin | 221 | Menu |

---

## File I/O

File handles are integer ids (0 = failed to open).

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
| `Insert` | `member: TypeMember, target: TypeMember` | — | Moves/inserts `member` before `target` in the list |
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
| `Encrypt` | `src: String\|Integer, dst: String\|Integer, password: String` | — | XOR-encrypts a file or memblock with a repeating password |
| `Decrypt` | `src: String\|Integer, dst: String\|Integer, password: String` | — | Inverse of `Encrypt` |
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
- **Single-byte strings.** See [string semantics](#string-semantics) — cbEnchanted
  is byte/CP-1252 oriented, not UTF-8.

### Collision model

- **Types**: 1 = box (AABB; `ObjectRange(obj, w, h)`), 2 = circle
  (`ObjectRange(obj, diameter)`), 4 = map (tilemap walls; target only), 3 = pixel
  (declared, not fully implemented).
- **Handling**: 0 = report only, 1 = stop (circle only), 2 = slide.
- Each recorded collision exposes the other object, a contact point `(X, Y)`, and
  a contact angle, queried via `CountCollisions`/`GetCollision`/`CollisionX/Y/Angle`.

---

## Implementation status in cbcompiler_rs

The sections above describe the **full cbEnchanted surface** (the target).
cbcompiler_rs currently implements a subset, and in places diverges
intentionally. Known status and divergences as of the latest runtime work
(FD-013/014/015/016):

- **String semantics divergence.** cbcompiler_rs strings are **UTF-8** and all
  char-indexed ops count Unicode **codepoints**, whereas cbEnchanted is
  single-byte CP-1252 and counts bytes. Out-of-range arguments **clamp** rather
  than error (`Left("hi",5)`→`"hi"`; `n<=0`→`""`). `Upper`/`Lower` are ASCII-only
  for now. The **full String surface is implemented** (FD-017): `Mid`, `Replace`,
  `LSet`, `RSet`, `Asc`, `Bin`, `String`, `Flip`, `StrInsert`, `StrMove`,
  `CountWords`, `GetWord` join the FD-013 set. `InStr` not-found returns `0` (per
  spec). Codepoint-specific notes: `Flip` reverses by codepoint (valid UTF-8 out);
  `Asc` returns the first **codepoint** (inverse of `Chr`), not a 0–255 byte;
  `StrInsert` uses proper 1-based indexing (cbEnchanted's StrInsert has an
  off-by-one its StrRemove/StrMove siblings do not — not reproduced); `String(s,n)`
  with `n<1` still yields one copy (cbEnchanted parity).
- **Math.** Full surface implemented (FD-017 adds `CurveValue`, `CurveAngle`,
  `BoxOverlap`). `Rand(low,high)` is **inclusive** `[low,high]` and `Rnd(low,high)`
  is `[low,high)` (cbEnchanted parity); the `high < low` branch is the documented
  special case (Rnd → `randf()*low`, Rand → `rand(low)`), not a swap. `Int(Float)`
  rounds to the **nearest integer, ties away from zero** (`f64::round`, so
  `10.5 → 11` and `-1.5 → -2`), and `Int(String)` trims then parses a leading
  integer — both via the interpreter's `convert_value` (so explicit `Int()` and
  implicit Float→Int coercion agree).
- **System / Time.** `Timer` returns monotonic ms since first call
  (`std::chrono::steady_clock`; the legacy used CPU-time `clock()`). `Wait(ms)`
  sleeps (`ms <= 0` is a no-op). `End` is a language statement lowered to an IR
  `Halt`; `MakeError(msg)` writes to stderr and exits with code 1. FD-017 adds
  `Date` (`"D Mon YYYY"`), `Time` (`"HH:MM:SS"`), `CommandLine`, and `GetEXEName`;
  the last two read the running `cb` process (no separate compiled program exists
  as in cbEnchanted). `FrameLimit`/`Errors`/`SetWindow`/`Crc32` remain deferred
  (window/loop/error-display plumbing).
- **Graphics / images** (FD-013 Batch 4, Allegro 5, `runtime/cb_gfx.cpp`).
  `Image` wraps an Allegro bitmap.
  `DeleteImage` exists (the legacy leaked image handles until exit). `Circle`'s
  argument is a diameter; `GetPixel`/`PutPixel` packed form uses 32-bit ARGB.
  `MakeImage`/`LoadImage` work before any `Screen()` (memory-bitmap fallback).
  FD-017 adds `Screen(w,h,depth,mode)` (depth ignored — always 32-bit) and the
  no-arg `Screen()` (buffer id 0), `ScreenDepth` (32), `GFXModeExists`
  (best-effort: any positive mode → 1), `DrawScreen(cls,vsync)`, `GetRGB`,
  `PickColor` (reads the current target), `Smooth2D` (linear filtering on new
  bitmaps), `Ellipse`, `CopyBox` (current target only), `PutPixel2`/`GetPixel2`
  (ARGB aliases; the buffer-id arg models only the current target). `ScreenGamma`
  is stored but not applied and `ScreenShot` is a no-op without a window
  (Allegro 5 has no portable display-gamma ramp). **Image** additions (FD-017,
  single-frame — multi-frame `LoadAnimImage`/`frame` params deferred):
  `CloneImage`, `ResizeImage`, `RotateImage` (rotated bounding box, centered
  hotspot), `PickImageColor`/`PickImageColor2`, `SaveImage` (`frame` ignored),
  `DrawGhostImage` (alpha 0–100), `DrawImageBox`, `DefaultMask`, `ImagesOverlap`
  (AABB), and `ImagesCollide` (pixel-precise via non-zero alpha; `frame` args
  ignored). `MakeImage` now clears to opaque black (defined contents). `CbImage`
  gained a **hotspot** (default top-left); `HotSpot(img, x, y)` is the per-image
  form (x<0 || y<0 auto-centers) — cbEnchanted's integer-id `0`/`1` default toggle
  has no analogue since `Image` is an opaque handle, not an int id.
- **Input** (FD-013 Batch 5, `runtime/cb_input.cpp`). Keyboard
  (`KeyDown`/`KeyUp`/`KeyHit`/`EscapeKey`) and the scancode table are ported 1:1.
  Input advances per frame inside `DrawScreen`. The mouse functions
  (`MouseDown`/`MouseHit`/`MouseUp`/`MouseZ`/`MouseMove*`) were added on Allegro's
  mouse model. `EscapeKey` is a pure query (no legacy auto-exit). FD-017 adds
  `GetKey` (typed-char queue), `LeftKey`/`RightKey`/`UpKey`/`DownKey`,
  `ClearKeys`/`ClearMouse` (swallow events until the next frame), `GetMouse`
  (button-down queue), `PositionMouse`, `ShowMouse` (0=hide/1=show; image cursors
  unsupported — `Image` is an opaque handle, not an int id), and `WaitKey`/
  `WaitMouse` (block on the window event queue; with no window they return 0
  immediately rather than hang). `MouseWX`/`MouseWY` and `Input`/`CloseInput`/
  `SafeExit` remain deferred (camera / interactive on-screen entry).
- **Text / fonts** (FD-018, Allegro font/ttf addons, `runtime/cb_gfx.cpp` +
  `runtime/cb_font.cpp`). `Font` is an opaque handle (like `Image`). Immediate
  `Text`/`CenterText`/`VerticalText` draw in the current color onto the active
  target; the `Locate`/`AddText`/`ClearText` queue re-renders every `DrawScreen`
  until cleared. `LoadFont` takes a system family name (resolved via a ported
  Windows font table / fontconfig on Linux) or a file path (name containing a
  `.`); `Smooth2D` toggles antialiased vs monochrome glyphs; `underline` is
  accepted but not rendered (cbEnchanted TODO). The default font is Courier New
  12pt, falling back to Allegro's built-in 8×8 font so `Text`/`TextWidth`/
  `TextHeight` never crash and work headless. `VerticalText` is documented as
  `(x, y, s)` (cbEnchanted's command pops `(y, x, s)` — a likely label swap).
- **Pixel-precise ARGB.** Where cbcompiler_rs uses packed 32-bit **ARGB**,
  cbEnchanted's `PutPixel`/`GetPixel` use packed `0xRRGGBB`. Reconcile when
  implementing.
- **Not yet implemented** in cbcompiler_rs: objects/sprites, collision, camera,
  tile maps, sound, video playback, particles, file I/O, memblocks,
  `Read`/`Restore`, `Encrypt`/`Decrypt`, `CallDLL`, multi-frame sprite sheets
  (`LoadAnimImage` and the `frame` params), and the plumbing-heavy System funcs
  (`Crc32`, `SetWindow`, `FrameLimit`, `Errors`).

### Runtime library architecture (cbcompiler_rs, FD-016)

The cbcompiler_rs C++ runtime is split into two static libraries with a strict,
one-directional dependency (functionality → core; core depends on nothing
functional):

- **`cb_runtime_core`** — the irreducible, plugin-facing ABI: the opaque
  `CbString` type and its primitives, the `CbStringApi` table, the catalog
  descriptor structs (`CbTypeTag`, `CbTypeDesc`, `CbParamDesc`, `CbFuncDesc`,
  `CbCatalog`), `CB_CATALOG_VERSION`, and `cb_runtime_get_catalog`. FD-015 adds
  the `CbHostApi`/`CbRuntimeHooks`/`cb_runtime_init` trap-channel handshake.
  **Zero Allegro dependency.** Header: `runtime/cb_runtime_core.h`; single TU:
  `runtime/cb_string.cpp`.
- **`cb_runtime`** (functionality) — the feature subsystems built on core: the
  catalog assembly, the String library (`cb_strfuncs.cpp`), Math, System/Time,
  Graphics, Input. Links `cb_runtime_core` plus the full Allegro closure. Header:
  `runtime/cb_runtime_func.h`.

`runtime/cb_runtime.h` remains a back-compat umbrella including both narrow
headers; it will be removed once every TU migrates.

#### Plugin ABI contract

Plugins (separate DLLs, per FD-009) handle `String` parameters by **statically
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
