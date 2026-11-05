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

v0.7 adds **list removal and capacity**. Lists now carry a capacity alongside
their length, so `add VALUE to LIST` appends in place while there is room and
only copies to fresh heap space when it must grow. `remove last from LIST` and
`remove item N from LIST` shorten a list in place and shift later items left,
with runtime checks for an empty list or an item number outside it. Everything
from v0.6 — ownership, system I/O, and the full-screen session — still works.
The compiler is a lexer, parser, import expander, type checker, ownership
checker, AArch64 code generator, and ELF writer, with 177 tests. It targets
**AArch64 Linux** only.

## Quick start

```console
cargo build --release
cargo run --                              # start the interactive session
cargo run -- run examples/hello.rk       # bake and run
cargo run -- tui                          # start the full-screen session
cargo run -- tui demo                     # try the terminal module demo
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

# break leaves the loop; continue jumps to the next turn
for each slice from 1 to 5:
    if slice is 3:
        continue
    if slice is 5:
        break
    say slice

# arithmetic and truth
say (10 - 4) * 2
say 100 / 3
say temperature is at least 180
say not false
say true and not false
say 1 is less than 2 or 2 is less than 1

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
add "spelt" to names
say names                     # [rye, wheat, spelt]
remove last from names
say names                     # [rye, wheat]
remove item 1 from names
say names                     # [wheat]

let empty be an empty list of numbers
say empty                     # []
```

Text and lists are owned. Giving one a second name moves it, and the compiler
rejects any later use of the old name — so there is never a dangling name,
a double free, or two writers racing over one value:

```rusk
let words be ["rye", "wheat"]
let mine be copy of words     # `copy of` makes an independent list
say words                     # [rye, wheat]
say mine                      # [rye, wheat]

let greeting be "hello"
let echoed be greeting        # this moves greeting
say echoed                    # hello

# say greeting                # compile error: greeting was moved

constant motto be "bake twice"
let sign be copy of motto     # constants are read-only; copy them
say sign
```

Numbers and truth values are plain machine words, so they are copied freely
and never move. The rule is checked before code generation, not at runtime.

Programs can talk to the system without a library:

```rusk
say number of arguments       # arguments after the program's own name
say argument 1                # empty text when there is no such argument
say environment "HOME"        # empty text when the variable is unset

let name be read line         # one line, without its newline
let rest be read all input    # everything left on standard input
let notes be read file "notes.txt"
write "baked fresh" to file "loaf.txt"

warn "using the spare oven"   # standard error, with a newline
stop with 2                   # choose the process exit status
```

`read file` and `write ... to file` use the path as written. A missing or
unreadable file, or a path too long for the runtime scratch area, is reported
as a Rusk error. `rusk run file.rk one two` passes `one` and `two` through to
`argument 1` and `argument 2`.

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
comment. Full reference: [docs/SPEC.md](docs/SPEC.md). A hands-on tour lives in
the [guide](guide/README.md).

## The prompt

Run `rusk` with no arguments (or `rusk repl`) to work interactively:

```console
$ rusk
░█▀▄░█░█░█▀▀░█░█
░█▀▄░█░█░▀▀█░█▀▄
░▀░▀░▀▀▀░▀▀▀░▀░▀

Rusk 0.7.0 — an English-like language
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

`rusk tui` starts the same session in a full-screen interface. It keeps the
transcript, input line, history, and `:commands` on one screen, with the
banner, expression results, multi-line blocks, and errors scrolling above the
prompt. Editing supports arrows, Home/End, Delete, Page Up/Down, and UTF-8;
Up/Down recall earlier lines, Ctrl-C cancels the current line or block, Ctrl-L
clears the transcript, and Ctrl-D leaves. It puts the terminal into raw mode
through `termios`, draws ANSI frames, notices window resizes, and restores the
terminal on the way out — including when a program uses `stop with`.
`rusk tui demo` opens the smaller terminal-module demo instead. There is no
`crossterm` or `ratatui`; over a pipe both commands fall back to plain text
instead of escapes.

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
 types      kinds and return-kind inference
   │
   ▼
ownership   one owner per text value and list
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
| `src/ownership.rs` | Flow-sensitive single-owner checking for text and lists |
| `src/codegen.rs` | AST to machine code; stack-machine expression evaluation, stack frames |
| `src/aarch64.rs` | Hand-written instruction encoder and branch/address fixups |
| `src/elf.rs` | Static ELF64 executable writer |
| `src/repl.rs` | Interactive session: tree-walking evaluator, line prompt, and full-screen loop |
| `src/tui.rs` | Native terminal UI: raw mode, ANSI frames, key decoding, resize fallback |
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
- Lists are stored as `[len: u64][capacity: u64][item: i64]...` in the same
  bump heap. A list value is a pointer, but the ownership checker allows only
  one live name for it, so `let b be a` moves the list and `copy of a` makes a
  new one. Item and bounds checks happen in the runtime helper `array_get` /
  `array_set`. `array_add` appends in place while the length is below the
  capacity; when it must grow it allocates a new block with at least four
  slots, or twice the old capacity, and leaves the old block behind.
  `array_remove_last` and `array_remove_at` shorten the list in place, shift
  later items left, and keep the capacity unchanged.
- Text compares byte by byte (unsigned), then by length, so `"b"` is more than
  `"abc"` and `"ab"` is less than `"abc"` — ordinary dictionary order. Lists do
  not compare yet.
- The kernel's initial stack pointer is saved at entry, so `argument N` and
  `environment NAME` can walk `argv` and `envp` directly. File paths are copied
  into a 4 KiB scratch area and NUL-terminated before `openat(2)`. Standard
  input and files are read with `read(2)`; files are written with
  `openat(2)`/`write(2)`/`close(2)`.
- `say` of a number calls a hand-built itoa loop that divides by ten and writes
  digits backwards into a 32-byte buffer.
- Before code generation, `ownership` walks every path and rejects a use of a
  moved name. A move in one arm of an `if` counts as a move after it; a `give
  back` ends that path. `copy of` is the only way to duplicate an owned value.
- Runtime guards turn three machine-level failures into Rusk sentences on
  standard error with exit status 1: division by zero, heap exhaustion, and
  recursion deeper than 1000 calls.
- Everything is position-independent-free and fixed-address; there is no
  relocation pass at runtime.

## What v0.7 doesn't do yet

Honest list, so the docs don't lie:

- Text is immutable. Joining allocates from a fixed 1 MiB heap, and nothing is
  reclaimed. Running past the end is now a Rusk error rather than a crash.
- There is no borrow checker and no shared borrowing: a value has one owner at
  a time, and `copy of` deep-copies text and lists. That is simpler than Rust,
  and it rules out aliasing bugs by refusing to have aliases at all.
- Lists are homogeneous and flat: no nested lists. Their contents can be
  replaced with `set item`, `add VALUE to LIST` appends with spare capacity,
  and `remove last from LIST` / `remove item N from LIST` shorten them.
  Capacity never shrinks, and removal does not give memory back to the heap.
- System I/O is intentionally small: text files and standard streams only, no
  directories, sockets, processes, or random access. A path is limited by the
  4 KiB scratch area, and reading stops when the 1 MiB heap is full.
- Imports are textual inclusion with one global namespace. There are no module
  namespaces, selective imports, or aliases, so modules prefix their names
  (`text_`, `math_`).
- Text positions count bytes, not Unicode characters.
- At most 8 parameters per function, and no closures, anonymous functions, or
  function values.
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
