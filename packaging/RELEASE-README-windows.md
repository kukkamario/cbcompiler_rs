# CoolBasic compiler (`cb`) — Windows x64

This archive contains:

| Path      | What it is                                                                 |
| --------- | ------------------------------------------------------------------------- |
| `cb.exe`  | The compiler — both backends: `--backend interp` and `--backend llvm`.     |
| `lib\`    | The CoolBasic runtime + the Allegro static libraries the AOT backend uses. |
| `bin\`    | A bundled `clang.exe` + `lld-link.exe` (the AOT link driver) and `xwin.exe`, which fetches the Microsoft CRT + Windows SDK import libraries on first use. |

Allegro is linked **statically**, so the interpreter backend is self-contained:

```bat
cb.exe --backend interp program.cb
```

## AOT compilation (`--backend llvm`)

One-time setup — download the Microsoft CRT + Windows SDK import libraries the
linker needs (**no Visual Studio / Windows SDK install required**):

```bat
cb.exe --setup-toolchain
```

This runs the bundled `xwin` to fetch the import libs into `%LOCALAPPDATA%\cb\winsdk`.
They are Microsoft components governed by the Microsoft Software License Terms,
which running `--setup-toolchain` accepts. Then compile and run:

```bat
cb.exe --backend llvm program.cb -o program.exe
program.exe
```

clang + lld are bundled too, so **no separate LLVM install is needed**. To *run*
produced programs (and `cb.exe` itself), install the **Microsoft Visual C++
Redistributable** — that is the only Microsoft component you need on the machine.

The AOT backend finds its runtime in `lib\` and its linker + `xwin` in `bin\`
next to `cb.exe`; keep them together. Overrides, if needed: `CB_LINK_DRIVER` (the
link driver), `CB_WIN_SDK` (an existing import-lib sysroot, skipping the fetch),
`CB_XWIN` (the xwin tool).
