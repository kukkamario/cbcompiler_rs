# FD-040: File I/O Runtime Functions and Types

**Status:** Complete
**Completed:** 2026-06-21
**Priority:** Medium
**Effort:** High (> 4 hours)
**Impact:** Brings CoolBasic's file-I/O subsystem online — opening/reading/writing binary & text files, plus filesystem/directory queries — unlocking save games, level/config files, and asset tooling in CoolBasic programs. Headless-testable (Allegro-free), so it lands in the SDK-free catalog and runs in CI.

## Problem

CoolBasic programs persist and load data through a file-I/O surface that the runtime does not yet implement. The whole batch (`OpenToRead`/`ReadByte`/`WriteLine`/`FileExists`/…) returns "not yet implemented" today — `docs/cb_runtime.md` §"File I/O" documents ~31 commands and a `File` opaque type, none of which exist in the runtime catalog.

This was deliberately deferred at FD-013 ("File I/O wants the FD-015 trap channel first"). That prerequisite landed in **FD-015** (the runtime can now raise a clean runtime error instead of `exit()`/UB), so the subsystem is unblocked. It is independent of the Sound → Video track and can be done now.

The behavior below was mined from the two authoritative sources (per the FD-036/038/039 practice): the **cbEnchanted** reference impl (`G:\projects\cbEnchanted\src\fileinterface.cpp`) and the **official CoolBasic Help** (`G:\tools\CoolBasic\Help\commands\*.html`, in Finnish). See "Authoritative reference semantics" and "Deliberate divergences" below.

## Solution

Follow the **FD-039 (Memory Blocks) playbook** — the closest precedent: a new Allegro-free C++ TU, a raw-pointer opaque handle registered via the FD-011 machinery **outside** the `CB_NO_ALLEGRO` guard (so it ships headless), little-endian-on-the-wire binary primitives, and trap-on-misuse instead of classic CB's blind-cast UB. Generic libffi dispatch means **zero Rust frontend work** beyond `cb-runtime-sys` catalog-content asserts.

### `File` opaque type *(resolves decision #1 — opaque type)*

- Register `File` as the next catalog type — **tag 16** (Image 11, Font 12, Object 13, Map 14, Memblock 15 → **File 16**). Raw-pointer `CbFile*` opaque handle, no numeric id space; a declared-but-unassigned `File` is `Null`; a failed open returns `Null` (the FD-018 null-opaque→`Value::Null` mapping already handles this in `ffi.rs`, so the documented "0 on failure" / `= Null` comparison works for free).
- Registered **outside** the `CB_NO_ALLEGRO` guard (Allegro-free), so it is present in the SDK-free catalog and runs headless. Type table grows **6 → 7**; **no `CB_CATALOG_VERSION` bump** (adding a type + functions changes no ABI struct layout; catalog stays v6 — same reasoning as FD-039).
- `struct CbFile` carries the open stream (`FILE*` via `<cstdio>`, or `std::fstream`), the **open mode** (Read / Write / Edit — needed for mode enforcement), and an EOF/last-op flag. Both cbEnchanted and classic CB use integer handles in a table; we deliberately use the opaque-handle pattern instead (see Divergences) — it gives `= Null` null-safety and type-distinct rejection of arithmetic/field-access for free.

### New Allegro-free TU(s)

- `runtime/cb_file.cpp` — the 31 `cb_rt_*` entry points + `struct CbFile`. Built unconditionally (Allegro-free), added to the SDK-free TU set in `build.rs` and the non-guarded `catalog.cpp` block. Binary primitives use the same explicit little-endian byte assembly as `cb_memblock.cpp`; filesystem/directory queries use C++17 `<filesystem>`.
- `runtime/cb_file.h` — pure, headless-testable helpers (the LE byte (de)serialization + length-prefix framing) so gtest can exercise the wire format without touching the filesystem, mirroring `cb_memblock`/`cb_particle.h`. Reuse FD-039's LE helpers if they can be shared cleanly.

### Command surface (31 — exact set from `docs/cb_runtime.md`)

**Open / close / position (7):** `OpenToRead` (`rb`), `OpenToWrite` (`wb`, create/truncate), `OpenToEdit` (`rb+` if exists else `wb+`, read/write), `CloseFile`, `SeekFile(f, pos)` (absolute, `SEEK_SET`), `FileOffset(f) -> Int`, `EOF(f) -> Int`.

**Binary read / write (12):** `ReadByte`/`ReadShort`/`ReadInt`/`ReadFloat`/`ReadString`/`ReadLine` and `WriteByte`/`WriteShort`/`WriteInt`/`WriteFloat`/`WriteString`/`WriteLine`.

**Filesystem & directory (12):** `FileExists`, `IsDirectory`, `FileSize`, `CurrentDir() -> String`, `ChDir`, `MakeDir`, `CopyFile(src, dst)`, `DeleteFile`, `Execute(cmd)`, `StartSearch`/`FindFile() -> String`/`EndSearch`.

> **Naming gotcha:** `EOF`, `DeleteFile`, `CopyFile` collide with C/Win32 macros. The CB-visible catalog *names* are fine as strings, but the C++ symbols must be `cb_rt_eof`/`cb_rt_delete_file`/`cb_rt_copy_file` and `cb_file.cpp` (being Allegro-free) must not pull in `<windows.h>`. Guard against `#include <cstdio>`'s `EOF` macro inside the function body.

### Binary wire format — mirror FD-039 exactly (byte-compatible with classic CB)

cbEnchanted writes native-endian on x86 (little-endian); FD-039's explicit-LE-on-the-wire is therefore **byte-for-byte compatible with classic CB data files** *and* consistent with Memblock. Keep it:

- **Little-endian on the wire** regardless of host byte order (explicit byte assembly, not `reinterpret_cast`).
- `ReadByte`/`ReadShort` return **unsigned** (0..255 / 0..65535); `ReadInt` **signed** i32. *(Confirmed by both sources: Help says ReadByte "0–255", ReadShort "0–65535"; cbEnchanted's `uint8_t`/`uint16_t` zero-extend.)*
- `Float` is **32-bit on the wire**: `WriteFloat` narrows the CB f64 → f32, `ReadFloat` widens back. *(Confirmed: cbEnchanted reads/writes `float`.)*
- `WriteByte`/`WriteShort` write the low 8/16 bits of the CB Int (matches cbEnchanted `toByte`/`toShort`).
- `ReadString`/`WriteString`: **32-bit (LE) length prefix + raw bytes**, no NUL terminator (matches cbEnchanted's `int32` prefix + `fwrite`). Content bytes are the string's **raw UTF-8** (our `CbString` ABI) — no transcoding; see Divergences for the CP1252 implication.

### Text I/O — `ReadLine` / `WriteLine`

- `WriteLine`: append the **OS line ending** — CRLF on Windows, LF elsewhere (matches cbEnchanted + the doc).
- `ReadLine`: read to end-of-line and **strip the terminator**, handling **LF, CR, and CRLF** uniformly (consume the full terminator). This deliberately fixes cbEnchanted's bug where ReadLine only breaks on CR or EOF and silently swallows LF — making LF-only Unix files read as one giant line (see Divergences).

### Filesystem / directory

- `FileExists` → 1 if a file **or** folder exists, else 0. `IsDirectory` → 1 for a directory, 0 for a regular file/missing. `FileSize` → byte size as i32, **0 for a missing path or a directory** (per our doc; cbEnchanted returns whatever Allegro reports).
- `CurrentDir` → working directory as a String, **with a trailing separator** (matches cbEnchanted + actual CoolBasic; the official Help's "no trailing backslash" is wrong).
- `ChDir`/`MakeDir`/`DeleteFile` operate on the given path; `DeleteFile` removes a file or an **empty** directory (a non-empty directory cannot be removed — matches the Help). On failure these are non-fatal in classic CB; we either no-op or trap — fold into the error-handling policy below.
- `CopyFile(src, dst)` — **arg order is (source, destination)** (confirmed by both sources). **Refuses to overwrite:** if `dst` already exists it traps *(resolves decision #4)* rather than overwriting (matches cbEnchanted's fatal error and the Help's "operation fails").
- `Execute(cmd)` — **match cbEnchanted exactly**: build a shell string and call `std::system` — `"start " + cmd` on Windows, `"xdg-open " + cmd` elsewhere. No return value, no extra quoting (the caller quotes paths with spaces via `Chr(34)`, per the Help). Not headless-testable, so its real (visual) smoke is deferred like the FD-036/038 display smokes.
- Path encoding: our `CbString` is UTF-8 (FD-014); pass UTF-8 paths to `std::filesystem` (use `u8path`/native conversion so Windows non-ASCII paths work). cbEnchanted is inconsistent here (CP1252 to `fopen`, UTF-8 to Allegro) — we standardize on UTF-8.

### Error handling — lenient at end-of-data, trap on invalid handle

Two distinct classes, split deliberately:

- **End-of-data on a valid handle is lenient** *(resolves decision #2)*: reading at/past EOF returns a **zero value** (`0` / `0.0` / `""`), zero-filling any missing bytes of a multi-byte read, and leaves `EOF()` true. A `ReadString` whose length prefix exceeds the bytes remaining (or is negative) returns what's available / empty — never over-reads. This is *safer than* cbEnchanted (which returns uninitialized garbage and can crash on a bad length) while keeping read loops simple — the user's rationale: "otherwise reading files is too tricky."
- **A genuinely invalid handle is a program bug → trap** (FD-015 channel, exit 1): operating on a `Null`/closed/never-opened `File`, or a **wrong-mode** op (writing a Read handle, reading a Write handle). cbEnchanted is permissive (uses the `FILE*` as-is, no mode check); we deliberately trap instead — a stricter-than-classic safety check that catches a real program bug rather than silently no-op'ing.

### Directory search — single global cursor, no `.`/`..` *(resolves decision #5)*

`StartSearch`/`FindFile`/`EndSearch` share **one global cursor** over the current working directory (faithful, non-reentrant — matches both sources). `FindFile` returns the next bare entry name (files **and** folders), and the **empty string `""` when exhausted**. It does **not** synthesize `"."`/`".."` — a deliberate divergence from cbEnchanted (which emits them as the first two entries when the dir is ≥2 levels deep); the Help is silent on them, so omitting is within the documented contract and cleaner for callers.

## Authoritative reference semantics (confirmed)

From cbEnchanted `fileinterface.cpp` + official Help; both agree unless noted:

| Aspect | Confirmed behavior |
|--------|--------------------|
| Open modes | Read=`rb`, Write=`wb` (create/truncate), Edit=`rb+` if exists else `wb+`; **0/Null on failure** (missing file, is-a-directory, write-protected, disk full) |
| Offset model | Pointer starts at **0** on open; advances by bytes read/written; `SeekFile` is absolute from start |
| EOF | Boolean (1 at end, 0 otherwise); documented only as a *pre-read* test |
| ReadByte/Short | **unsigned** (0..255 / 0..65535) | 
| ReadInt | **signed** i32 |
| Read/WriteFloat | **32-bit** on disk |
| ReadString/WriteString | length-prefixed (cbEnchanted: i32 LE prefix + raw bytes) |
| WriteLine | OS line ending (CRLF on Win, LF on Unix) |
| CopyFile | arg order **(src, dst)**; **fails if dst exists** (no overwrite) |
| DeleteFile | removes file or **empty** dir; no Recycle Bin; non-empty dir cannot be removed |
| FindFile | next bare name, folders included, `""` when done; single global cursor over CWD |
| Execute | cbEnchanted: `system("start "+cmd)` (Win) / `system("xdg-open "+cmd)` (Unix); Help: launches program / opens docs by default app / URLs / `mailto:`; quote paths with spaces |

**Documentation gaps** (Help leaves unspecified — we pin them): read-past-EOF return value; ReadString/WriteString on-disk format; ReadLine/WriteLine exact terminator bytes; FindFile `.`/`..` filtering; OpenToWrite truncation of an existing file. (The Help also has minor numeric typos: ReadShort range "0–65536", WriteInt min "-2147483647".)

## Deliberate divergences from cbEnchanted / classic CB (all safety/correctness improvements)

1. **Opaque `File` handle** (tag 16, `Null` default) instead of integer ids in a table — consistent with Object/Map/Memblock; gives null-safety + type-distinct rejection. *Update `docs/cb_runtime.md`'s "File handles are integer ids" line.*
2. **Explicit LE wire** (vs cbEnchanted's native-endian) — byte-compatible on x86, portable everywhere.
3. **Lenient zero-fill on over-read** (vs uninitialized garbage); reads never UB.
4. **`ReadString` guards** a negative / over-long length prefix and **preserves embedded NULs** (reads exactly N bytes into the length-prefixed `CbString`) — cbEnchanted crashes on a negative length and truncates at the first NUL via `string(cstr)`.
5. **`ReadLine` handles LF/CR/CRLF** — cbEnchanted only breaks on CR or EOF, mis-reading LF-only files.
6. **`FindFile` omits `.`/`..`** (vs cbEnchanted synthesizing them).
7. **Trap on null/closed/invalid handle and wrong-mode ops** via the FD-015 channel.
8. **On-disk strings are raw UTF-8** (our `CbString` ABI) vs classic CB's CP1252. Identical bytes for ASCII; non-ASCII string content differs, so classic-CB data files with non-ASCII strings are not byte-compatible. Accepted for simplicity (no transcoding / CP1252 table). `Execute` is **not** a divergence — it matches cbEnchanted (`system("start"/"xdg-open" + cmd)`).

## Design Decisions (all resolved)

All design questions are settled — the FD is ready to implement:

- **#1** opaque `File` type (tag 16, `Null` default)
- **#2** lenient end-of-data (zero-fill + set EOF, no trap)
- **#3** `Execute` per cbEnchanted (`std::system` + `start`/`xdg-open`)
- **#4** `CopyFile` traps on an existing dst (no overwrite)
- **#5** single search cursor, no `.`/`..`
- **#7** on-disk strings are raw UTF-8
- **#9** `CurrentDir` appends a trailing separator (matches cbEnchanted + actual CoolBasic; the Help's "no trailing backslash" is wrong — `cb_runtime.md` already says "trailing separator", so no doc change needed)
- **Wrong-mode ops trap** (stricter-than-classic safety check; cbEnchanted is permissive)

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `runtime/cb_file.cpp` | CREATE | The 31 `cb_rt_*` file-I/O entry points; `struct CbFile` (FILE* + mode + last-op); LE wire (de)serialization; `<filesystem>` directory ops; single global search cursor. **No separate header** — like `cb_memblock.cpp`, `CbFile` is forward-declared in `cb_runtime_func.h` and the TU is self-contained (the gtest drives the `cb_rt_*` entry points directly) |
| `runtime/cb_runtime_func.h` | MODIFY | Forward-declare `CbFile`; declare the `cb_rt_*` file prototypes |
| `runtime/catalog.cpp` | MODIFY | Register `File` type (tag 16) + 31 `CB_FN` rows, **outside** the `CB_NO_ALLEGRO` guard |
| `runtime/CMakeLists.txt` | MODIFY | Add `cb_file.cpp` to the core (Allegro-free) sources + the `CB_RUNTIME_TESTS` gtest target |
| `runtime/tests/test_file.cpp` | CREATE | Native gtest: LE round-trips (Byte/Short/Int/Float/String/Line), seek/offset/EOF, lenient over-read zero-fill, ReadString bad-length guard + embedded NUL, ReadLine LF/CR/CRLF, mode-enforcement + null-handle traps, directory ops + search cursor over a temp dir |
| `crates/cb-runtime-sys/build.rs` | MODIFY | Add `cb_file.cpp` to the SDK-free `cc` build + rerun-if-changed watches |
| `crates/cb-runtime-sys/src/lib.rs` | MODIFY | Catalog-content asserts: `File` tag 16, type counts (SDK-free + full), the 31 signatures decode |
| `crates/cb-driver/tests/programs.rs` | MODIFY | `runtime_file_fd040` golden — write-then-read-back round-trip over a temp file |
| `crates/cb-driver/tests/fixtures/programs/` | CREATE | `.cb` source + `.out` golden for the round-trip fixture |
| `crates/cb-driver/tests/cli.rs` | MODIFY | Trap fixtures — op on a null/closed handle, and `CopyFile` over an existing dst, exit 1 with stderr |
| `docs/cb_runtime.md` | MODIFY | Mark §"File I/O" implemented; rewrite "File handles are integer ids" → opaque `File`; record divergences (lenient EOF, no `.`/`..`, ReadLine endings, raw-UTF-8 string content). `CurrentDir` trailing-separator wording already correct |
| `docs/features/FEATURE_INDEX.md` | MODIFY | Move File I/O Active → Completed on close; add the row |

## Verification

- `cargo build --workspace` and `cargo test --workspace` green, **both** SDK-free (CI path) and full-Allegro (`CB_RUNTIME_REQUIRE_ALLEGRO=1`) — the new TU is Allegro-free so the `runtime_file_fd040` driver golden and the trap CLI tests must **run**, not graphics-skip.
- `cargo test -p cb-runtime-sys` — catalog asserts confirm `File` tag 16, the type table is 2 (SDK-free) / 7 (full), and the 31 signatures decode.
- Native `ctest` — new `File.*` gtest cases pinning: unsigned Byte/Short + signed Int + 32-bit Float round-trips, LE byte order on disk, `ReadString`/`WriteString` length-prefix framing (incl. embedded-NUL preservation + bad-length guard), `ReadLine` over LF/CR/CRLF inputs, `WriteLine` OS terminator, `SeekFile`/`FileOffset`/`EOF`, lenient over-read zero-fill, mode-enforcement + null-handle traps, `CopyFile` refuse-overwrite, `FindFile` no-`.`/`..` + `""`-at-end, and the `<filesystem>` queries over a temp directory.
- `cargo clippy --workspace --all-targets -D warnings` and `cargo fmt --all --check` clean.
- End-to-end driver smoke: a `.cb` program that `OpenToWrite`s a temp file, writes each primitive + a line, closes, `OpenToRead`s it back, prints the values, and walks the dir via `StartSearch`/`FindFile`/`EndSearch` — matches the golden.
- **Deferred (not headless):** real `Execute` smoke (launch a program / open a URL) — implemented but can't be verified in CI.

## Implementation Notes (2026-06-21)

Implemented exactly to the design above, following the FD-039 playbook.

- **`runtime/cb_file.cpp`** — all 31 `cb_rt_*` entry points + `struct CbFile { FILE*; FileMode; last_op }`. Built on `<cstdio>` (FILE*) + `<filesystem>`; **no separate header** (matches `cb_memblock.cpp`). Windows opens via `_wfopen` on the wide path (Unicode filenames); elsewhere `fopen` on the UTF-8 bytes. `<filesystem>` paths are built from `std::u8string` so UTF-8 is honored on every platform (no deprecated `u8path`). The Edit (`rb+`/`wb+`) read↔write interleave is handled by a `prepare()` no-op seek on direction switch; EOF probes one byte + `ungetc` (fixes the classic empty-file EOF bug). All filesystem ops use the `std::error_code` overloads so no exception crosses `extern "C"`.
- **Registration** — `File` tag 16 + the 31 `CB_FN` rows added **outside** the `CB_NO_ALLEGRO` guard in `catalog.cpp`; `CbFile` + prototypes in `cb_runtime_func.h`. No `CB_CATALOG_VERSION` bump (still v6); type table 6→7 full / 2→3 SDK-free. **Zero Rust frontend changes** — generic libffi dispatch + the FD-018 null-opaque→`Null` mapping cover everything; only `cb-runtime-sys` catalog asserts changed.
- **Build wiring** — `cb_file.cpp` added to the `cb_runtime` CMake lib + `SDK_FREE_TUS`/`RERUN_SOURCES` in `build.rs`; `tests/test_file.cpp` added to the gtest target.
- **Tests** — `runtime/tests/test_file.cpp` (17 `FileTest.*` gtest cases); driver golden `runtime_file_fd040` via a new non-graphics-gated `run_isolated` helper (temp cwd, relative file); `cli.rs` traps `file_op_on_null_handle_traps` + `copy_file_over_existing_traps`.

**Verified (2026-06-21, Windows + Allegro SDK):**
- SDK-free `cargo test --workspace` (CI path) — all green; `runtime_file_fd040` + both file CLI traps **ran** (not graphics-skipped); `cb-runtime-sys` catalog asserts confirm `File` tag 16, 3 SDK-free types, and the 31 signatures decode.
- Full Allegro `cargo test --workspace` (`CB_RUNTIME_REQUIRE_ALLEGRO=1`) — all green; catalog asserts confirm 7 types; genuine full link clean.
- `cargo clippy --workspace --all-targets -D warnings` clean; `cargo fmt --all --check` clean.
- Native **`ctest` 102/102** (85 prior + **17** new `FileTest.*`: primitive/LE/32-bit-float round-trips, embedded-NUL `ReadString`, LF/CR/CRLF `ReadLine`, seek/offset, Edit write↔read interleave, empty-file + past-EOF leniency, missing-open→Null, filesystem queries, `CopyFile` refuse-overwrite, directory search with no `.`/`..`, `CurrentDir` trailing separator, null-handle + wrong-mode safety).

**No deferred items** other than the real (visual) `Execute` smoke, which can't run headless. The subsystem is fully headless-testable.

## Related

- [`docs/cb_runtime.md`](../cb_runtime.md) §"File I/O" — command surface (31 functions, `File` type).
- **FD-039** (Memory Block Runtime Functions) — closest precedent: Allegro-free TU, raw-pointer opaque handle outside the `CB_NO_ALLEGRO` guard, LE wire format, unsigned-Byte/Short + signed-Int + 32-bit-Float conventions, trap-on-misuse. Reuse its LE wire-format helpers.
- **FD-015** (Runtime Trap Channel) — the prerequisite (now landed) that lets file ops raise clean runtime errors; FD-013 explicitly deferred File I/O until this existed.
- **FD-011** (Runtime Custom Types) — the opaque-type machinery `File` registers through.
- **FD-014** (Runtime String ABI) — the UTF-8 `CbString*` ABI that drives the on-disk-encoding decision (#7).
- **FD-018** (Runtime Text & Font Support) — established the null-opaque-return → `Value::Null` mapping that makes "Null on open failure" work with zero frontend change.
- **FD-036** (Game-Object Runtime Cluster) — precedent for dropping a numeric-id space in favor of raw-pointer opaque handles and updating the doc to match.
- `[[cbenchanted-runtime-reference]]` — authoritative semantics mined from `G:\projects\cbEnchanted\src\fileinterface.cpp`; official contract cross-checked against `G:\tools\CoolBasic\Help\commands\*.html`.
- Unblocks the memblock/file forms of `Crc32` and `Encrypt`/`Decrypt` (those take `String|Integer` = a file path or a memblock id).
