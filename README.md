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

v0.5 adds constants (`constant NAME be VALUE`), homogeneous lists (`[a, b]`,
`an empty list of numbers`, `item N of`, `set item N of`, `for each ... in`),
and recursive `import` statements that can pull in several files. It keeps the
text operations and interactive session from v0.4. The compiler is a lexer,
parser, import expander, type checker, AArch64 code generator, and ELF writer,
with 102 tests. It targets **AArch64 Linux** only.

## Quick start

```console
cargo build --release
cargo run --                              # start the interactive session
cargo run -- run examples/hello.rk       # bake and run
cargo run -- build examples/loop.rk      # write ./examples/loop
cargo run -- check examples/numbers.rk   # bake without writing anything
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

# measure, index, and slice text; positions count from 1
say length of name            # 4
say letter 1 of name          # R
say part 2 to 3 of name       # us
say "Hello, " + letter 1 of "world" + "!"
```

Lists hold one kind of thing, count from 1, and can be walked directly:

```rusk
constant batch_size be 12

let loaves be [3, 6, 9]
say length of loaves          # 3
say item 2 of loaves          # 6
set item 2 of loaves to batch_size
say loaves                    # [3, 12, 9]

let names be ["rye", "wheat"]
for each name in names:
    say "one " + name + " loaf"

let empty be an empty list of numbers
say empty                     # []
```

A file can pull in several modules; each file is included at most once:

```rusk
import "lib/greetings"
import "../ruskmods/math"

say greeting with "world"
say math_square with 12
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

## The prompt

Run `rusk` with no arguments (or `rusk repl`) to work interactively:

```console
$ rusk
Rusk 0.5.0 — an English-like language
Type :help for commands, :quit to leave.

rusk> let who be "world"
rusk> "Hello, " + who + "!"
Hello, world!
rusk> to double with n as a number:
...       give back n * 2
...
rusk> double with 21
42
```

Names and functions survive between lines. A bare expression shows its value,
the way Python's prompt does; `let`, `set`, `say`, and definitions stay quiet.
A line ending in `:` opens a block, and a blank line closes it. The session
uses the same lexer, parser, and type checker as the compiler, and text
positions count bytes just like a baked program.

```text
:help            show the commands
:quit            leave the session
:reset           forget every name and function
:vars            show the names the session remembers
:functions       show the functions the session remembers
:load <file.rk>  run a file in the session
```

Piped input skips the banner and prompts, so `rusk < program.rk` works too.

## How it works

```text
.rk source
   │
   ▼
 imports    expand each file once, recursively
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
| `src/imports.rs` | Find files, expand imports recursively, detect cycles, include once |
| `src/lexer.rs` | Source to tokens; keywords are plain words matched by the parser |
| `src/parser.rs` | Recursive descent over the English grammar |
| `src/types.rs` | Type checking and return-kind inference (fixed point) |
| `src/codegen.rs` | AST to machine code; stack-machine expression evaluation, stack frames |
| `src/aarch64.rs` | Hand-written instruction encoder and branch/address fixups |
| `src/elf.rs` | Static ELF64 executable writer |
| `src/repl.rs` | Interactive session: tree-walking evaluator over the same AST |
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
- Lists are stored as `[len: u64][item: i64]...` in the same bump heap. A list
  value is a pointer, so `let b be a` gives two names for one list. Item and
  bounds checks happen in the runtime helper `array_get` / `array_set`.
- Text compares byte by byte (unsigned), then by length, so `"b"` is more than
  `"abc"` and `"ab"` is less than `"abc"` — ordinary dictionary order. Lists do
  not compare yet.
- `say` of a number calls a hand-built itoa loop that divides by ten and writes
  digits backwards into a 32-byte buffer.
- Everything is position-independent-free and fixed-address; there is no
  relocation pass at runtime.

## What v0.5 doesn't do yet

Honest list, so the docs don't lie:

- Text is immutable. Joining allocates from a fixed 1 MiB heap with no bound
  check, and nothing is reclaimed.
- Lists are homogeneous and flat: no nested lists, and no `push`, `pop`, or
  `remove`. Their contents can be replaced with `set item`, but the list itself
  has a fixed length.
- Imports are textual inclusion with one global namespace. There are no module
  namespaces, selective imports, or aliases, so modules prefix their names
  (`text_`, `math_`).
- Text positions count bytes, not Unicode characters.
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
