# CoolBasic compiler (`cb`) — Windows x64

This archive contains:

| Path      | What it is                                                                 |
| --------- | ------------------------------------------------------------------------- |
| `cb.exe`  | The compiler — both backends: `--backend interp` and `--backend llvm`.     |
| `lib\`    | The CoolBasic runtime + the Allegro static libraries the AOT backend uses. |
| `bin\`    | A bundled `clang.exe` + `lld-link.exe` — the AOT link driver.              |

Allegro is linked **statically**, so the interpreter backend is self-contained:

```bat
cb.exe --backend interp program.cb
```

## AOT compilation (`--backend llvm`)

```bat
cb.exe --backend llvm program.cb -o program.exe
program.exe
```

clang + lld are bundled, so **no separate LLVM install is needed**. The final
link still uses the Microsoft Windows SDK / Visual C++ runtime (the CRT and
system import libraries), so install one of:

- **Build Tools for Visual Studio** (the "Desktop development with C++" workload
  includes the Windows SDK), or
- at minimum the **Microsoft Visual C++ Redistributable** to *run* produced
  programs.

The AOT backend finds its runtime in `lib\` and its linker in `bin\` next to
`cb.exe`; keep them together. Override the link driver with the `CB_LINK_DRIVER`
environment variable if needed.
