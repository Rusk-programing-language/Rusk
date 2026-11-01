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

v0.3 adds text joining and comparison, plus `while`, `for each`, and chained
`otherwise if`. The compiler is a lexer, parser, type checker, AArch64 code
generator, and ELF writer, with 63 tests. It targets **AArch64 Linux** only.

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

while temperature is more than 150:
    say "cooling..."
    set temperature to temperature - 20

for each slice from 1 to 3:
    say slice

# arithmetic and truth
say (10 - 4) * 2
say 100 / 3
say temperature is at least 180
say not false

# text joins with `+` and compares like a dictionary
let name be "Rusk"
say "Hello, " + name + "!"
if name is "Rusk":
    say "the crust is on"
```

Functions name a piece of work and can hand a value back:

```rusk
to double with n as a number:
    give back n * 2

to greet with who as text:
    say "Hello,"
    say who

to roll:
    say "rolling..."

say double with 21        # 42
greet with "world"
roll                      # a zero-argument call is just the name

to factorial with n as a number:
    if n is at most 1:
        give back 1
    give back n * factorial with n - 1

say factorial with 5      # 120
```

Parameters declare their kind (`a number`, `text`, `a truth value`). What a
function gives back is **inferred** from its `give back` lines, including
through recursion. A function with no `give back` gives nothing back and
cannot be used as a value.

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
| `src/types.rs` | Type checking and return-kind inference (fixed point) |
| `src/codegen.rs` | AST to machine code; stack-machine expression evaluation, stack frames |
| `src/aarch64.rs` | Hand-written instruction encoder and branch/address fixups |
| `src/elf.rs` | Static ELF64 executable writer |
| `src/main.rs` | The `rusk` CLI |

Runtime facts worth knowing:

- Code starts at `0x400000 + 0x78`; top-level variables and the number buffer
  live at `0x410000` in the zero-filled part of the same segment.
- Function arguments go in `X0..X7`, results come back in `X0`, and the return
  address rides in `X30`. Each call gets a frame at `X29` holding the saved
  `X29`/`X30`, its parameters, and its locals.
- Strings are stored as `[len: u64][bytes...]`, so a string value is just a
  pointer to its length. Joining two strings bumps a 1 MiB heap in the data
  area and copies both byte blobs into it.
- Text compares byte by byte (unsigned), then by length, so `"b"` is more than
  `"abc"` and `"ab"` is less than `"abc"` — ordinary dictionary order.
- `say` of a number calls a hand-built itoa loop that divides by ten and writes
  digits backwards into a 32-byte buffer.
- Everything is position-independent-free and fixed-address; there is no
  relocation pass at runtime.

## What v0.3 doesn't do yet

Honest list, so the docs don't lie:

- Text is immutable and append-only via `+`; there is no indexing, slicing, or
  `length of`. Joining allocates from a fixed 1 MiB heap with no bound check.
- At most 8 parameters per function, and no closures, anonymous functions, or
  function values.
- Division by zero traps the process instead of reporting a Rusk error.
- AArch64 Linux only. The codegen is written so an x86-64 encoder can slot in
  behind the same AST.
- The single load segment is RWX, which is convenient but not hardened.
- Deep recursion cannot be caught: it runs into the stack limit and dies.

## Why the name

Rusk is twice-baked bread: prepared ahead of time, shelf-stable, and it keeps.
Rust's compiler is `cargo`; Rusk's vocabulary leans into the bakery (`oven` for
the build tool, `loaf` for a package) — none of that exists yet, but the door is
open.

> Rusk is not Rust. It's an English-like AOT language that happens to be one
> letter away.

## License

MIT.
