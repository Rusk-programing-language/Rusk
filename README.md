# Rusk

**An English-like language that bakes ahead of time.**

A rusk is bread baked twice so it keeps for months. Rusk the language works the
same way: the compiler does the hard work up front, and what ships is a
standalone native executable with no runtime and no dependencies.

```rusk
say "Hello, world."
```

```console
$ rusk build hello.rk
baked hello.rk -> hello (366 B, native, 0 dependencies)
$ ./hello
Hello, world.
```

That binary is a static ELF file containing hand-encoded AArch64 machine code.
No C, no LLVM, no assembler, no linker, no libc — the program talks to the
kernel through `write(2)` and `exit(2)` syscalls.

## Status

v0.1 is a working vertical slice: lexer, parser, type checker, AArch64 code
generator, and ELF writer, with 30 tests. It targets **AArch64 Linux** only.

## Quick start

```console
cargo build --release
cargo run -- run examples/hello.rk      # bake and run
cargo run -- build examples/loop.rk     # write ./examples/loop
cargo run -- check examples/numbers.rk  # bake without writing anything
```

Or install the `rusk` binary from `target/release/` somewhere on your `PATH`.

## A taste of the language

```rusk
# variables
let temperature be 180
set temperature to temperature + 5

# English comparisons
if temperature is more than 150:
    say "Too hot. The crust will burn."
otherwise:
    say "Just right."

# loops
repeat 3 times:
    say "one loaf"

# arithmetic and truth
say (10 - 4) * 2
say 100 / 3
say temperature is at least 180
say not false
```

Everything is line-based, blocks are colon-plus-indentation, and `#` starts a
comment. Full reference: [docs/SPEC.md](docs/SPEC.md).

## How it works

```text
.rk source
   │
   ▼
 lexer      tokens (+ Indent/Dedent markers)
   │
   ▼
 parser     AST, recursive descent, English operators
   │
   ▼
 codegen    AArch64 machine code, one word at a time
   │
   ▼
 elf        ELF64 header + one PT_LOAD + your code
   │
   ▼
 executable
```

| File | Job |
|---|---|
| `src/lexer.rs` | Source to tokens; keywords are plain words matched by the parser |
| `src/parser.rs` | Recursive descent over the English grammar |
| `src/codegen.rs` | AST to machine code; stack-machine expression evaluation |
| `src/aarch64.rs` | Hand-written instruction encoder and branch/address fixups |
| `src/elf.rs` | Static ELF64 executable writer |
| `src/main.rs` | The `rusk` CLI |

Runtime facts worth knowing:

- Code starts at `0x400000 + 0x78`; variables and the number buffer live at
  `0x410000` in the zero-filled part of the same segment.
- Strings are stored as `[len: u64][bytes...]`, so a string value is just a
  pointer to its length.
- `say` of a number calls a hand-built itoa loop that divides by ten and writes
  digits backwards into a 32-byte buffer.
- Everything is position-independent-free and fixed-address; there is no
  relocation pass at runtime.

## What v0.1 doesn't do yet

Honest list, so the docs don't lie:

- No functions, parameters, or `give back` — they're the next milestone.
- No text comparison or concatenation (`"a" + "b"` is a type error, not a bug).
- One loop form (`repeat N times`), one condition form (`if` / `otherwise`).
- Division by zero traps the process instead of reporting a Rusk error.
- AArch64 Linux only. The codegen is written so an x86-64 encoder can slot in
  behind the same AST.
- The single load segment is RWX, which is convenient but not hardened.

## Why the name

Rusk is twice-baked bread: prepared ahead of time, shelf-stable, and it keeps.
Rust's compiler is `cargo`; Rusk's vocabulary leans into the bakery (`oven` for
the build tool, `loaf` for a package) — none of that exists yet, but the door is
open.

> Rusk is not Rust. It's an English-like AOT language that happens to be one
> letter away.

## License

MIT.
