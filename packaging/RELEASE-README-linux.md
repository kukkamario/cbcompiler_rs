# CoolBasic compiler (`cb`) — Linux x86_64

This archive contains:

| Path    | What it is                                                              |
| ------- | ----------------------------------------------------------------------- |
| `cb`    | The compiler — both backends: `--backend interp` and `--backend llvm`.  |
| `lib/`  | The CoolBasic runtime the AOT (`llvm`) backend links into your program. |

`cb` links Allegro 5 **dynamically**, so the Allegro shared libraries must be
present to run `cb` at all.

## Prerequisites

Install Allegro 5 (and, for AOT compilation, a C compiler):

```sh
sudo apt install \
  liballegro5-dev liballegro-acodec5-dev liballegro-audio5-dev \
  liballegro-image5-dev liballegro-ttf5-dev \
  build-essential
```

(The `-dev` packages pull in the runtime libraries and also provide the
`-lallegro*` link targets the AOT backend needs.)

## Usage

```sh
# Run a program with the interpreter:
./cb --backend interp program.cb

# Compile a native executable and run it:
./cb --backend llvm program.cb -o program
./program
```

The AOT backend finds its runtime in `lib/` next to `cb`; keep the two together.
It uses the system C compiler (`cc`) as the link driver — override with the
`CB_LINK_DRIVER` environment variable if needed.
