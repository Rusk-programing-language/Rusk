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
baked hello.rk -> hello (3.1 KB, native aarch64, 0 dependencies)
$ ./hello
Hello, world.
```

That binary is a static ELF file containing hand-encoded AArch64 or x86-64
machine code, chosen when you bake. No C, no LLVM, no assembler, no linker, no
libc — the program talks to the kernel through `write(2)` and `exit(2)`
syscalls.

## Status

v1.0 completes the everyday language: **shared read-only views**, **function
values**, and **closures**. A `view` lends text or a list to a function without
moving it, so the caller keeps using it afterwards. `the function double` makes
a named function into a value, `call f with ...` calls one, and
`let f be a function with ...:` captures the values around it. v0.9's nested
lists and module namespaces are unchanged.

The compiler is a lexer, parser, import expander, type checker, ownership
checker, target-selecting code generator, two machine-code backends, and an ELF
writer, with **226 tests**. The end-to-end suite also runs against x86-64 under
QEMU. It targets **Linux on AArch64 and x86-64**.

## Quick start

```console
cargo build --release
cargo run --                              # start the interactive session
cargo run -- run examples/hello.rk       # bake and run
cargo run -- run examples/closures.rk    # function values and views
cargo run -- tui                          # start the full-screen session
cargo run -- tui demo                     # try the terminal module demo
cargo run -- build examples/loop.rk      # write ./examples/loop
cargo run -- build examples/hello.rk --target x86_64  # cross-bake
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
a double free, or two writers racing over one value. When a function only needs
to read a value, borrow it as a **view** instead:

```rusk
to total with xs as a view of a list of numbers:
    let sum be 0
    for each x in xs:
        set sum to sum + x
    give back sum

let loaves be [3, 6, 9]
say total with loaves         # 18; loaves was not moved
say length of loaves          # 3
```

A view is shared read-only: it can be read, indexed, iterated, and passed on,
but not changed, moved, or used as the owner. `copy of` makes an independent
value.

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
import "lib/greetings"          # plain inclusion, global names
import "../ruskmods/math"       # math_square, ...

import "lib/units" as units     # units.convert, units.ratio, ...
from "lib/text" import trim     # trim, without a prefix
from "lib/text" import pad as pad_left

say greeting with "world"
say math_square with 12
say units.convert with 3
say pad_left with "x"
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

Functions can also be values, and a function value can remember the world where
it was made:

```rusk
let twice be the function double
say call twice with 21        # 42
say twice with 4              # the ordinary call spelling works too

let base be 10
let add_base be a function with n as a number:
    give back n + base

say call add_base with 5      # 15
```

A function written with `a function with ...:` is a **closure**: it captures
the names around it. Numbers and truth values are copied into the closure; text
and lists are captured as shared read-only views, so the closure can read them
but not move or change them. Function values can be called, stored in a
variable, and captured by another closure. They cannot be put in a list,
compared, copied, printed, or given back; a function value also cannot be
passed as an argument, because parameter kinds have no function form yet.

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

Rusk 1.0.0 — an English-like language
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
 codegen    AArch64 or x86-64 machine code
   │
   ▼
 elf        ELF64 header + one PT_LOAD + your code
   │
   ▼
 executable
```

| File | Job |
|---|---|
| `src/imports.rs` | Find files, expand plain/named/selective imports, detect cycles, include once |
| `src/lexer.rs` | Source to tokens; keywords are plain words matched by the parser |
| `src/parser.rs` | Recursive descent over the English grammar |
| `src/types.rs` | Type checking and return-kind inference (fixed point) |
| `src/ownership.rs` | Flow-sensitive single-owner checking for text and lists |
| `src/codegen.rs` | AST to machine code; stack-machine expression evaluation, stack frames |
| `src/backend.rs` | Target selection and the common assembly interface |
| `src/aarch64.rs` | Hand-written AArch64 encoder and branch/address fixups |
| `src/x86_64.rs` | Hand-written x86-64 encoder with memory-backed virtual registers |
| `src/elf.rs` | Static ELF64 executable writer for either target |
| `src/repl.rs` | Interactive session: tree-walking evaluator, line prompt, and full-screen loop |
| `src/tui.rs` | Native terminal UI: raw mode, ANSI frames, key decoding, resize fallback |
| `src/main.rs` | The `rusk` CLI |

Runtime facts worth knowing:

- Code starts at `0x400000 + 0x78`; top-level variables and the number buffer
  live at `0x410000` in the zero-filled part of the same segment.
- The code generator speaks one small virtual-register language: arguments go
  in `X0..X7`, results come back in `X0`, and the frame pointer is `X29`.
  AArch64 uses those registers directly. x86-64 keeps the virtual registers in
  a zeroed block at `0x418000`, uses the real `rsp` for `SP`, and lowers every
  instruction to one or more x86-64 operations. The return address lives on the
  process stack rather than in `X30`.
- Strings are stored as `[len: u64][bytes...]`, so a string value is just a
  pointer to its length. Joining two strings bumps a 1 MiB heap in the data
  area and copies both byte blobs into it.
- Lists are stored as `[len: u64][capacity: u64][item: i64]...` in the same
  bump heap. A list value is a pointer, but the ownership checker allows only
  one live name for it, so `let b be a` moves the list and `copy of a` makes a
  new one. Nested lists are pointers too; `copy of` and `say` use recursive
  routines that the code generator emits once per list shape. Item and bounds
  checks happen in the runtime helper `array_get` / `array_set`. `array_add`
  appends in place while the length is below the capacity; when it must grow it
  allocates a new block with at least four slots, or twice the old capacity,
  and leaves the old block behind. `array_remove_last` and `array_remove_at`
  shorten the list in place, shift later items left, and keep the capacity
  unchanged.
- Text compares byte by byte (unsigned), then by length, so `"b"` is more than
  `"abc"` and `"ab"` is less than `"abc"` — ordinary dictionary order. Lists do
  not compare yet.
- The kernel's initial stack pointer is saved at entry, so `argument N` and
  `environment NAME` can walk `argv` and `envp` directly. File paths are copied
  into a 4 KiB scratch area and NUL-terminated before `openat(2)`. Standard
  input and files are read with `read(2)`; files are written with
  `openat(2)`/`write(2)`/`close(2)`.
- `say` of a number calls a hand-built itoa loop that divides by ten and writes
  digits backwards into a 32-byte buffer. Syscall numbers are translated per
  target: AArch64 `write`/`exit`/`read`/`openat`/`close` are
  `64`/`93`/`63`/`56`/`57`, and x86-64 uses `1`/`60`/`0`/`257`/`3`.
- Before code generation, `ownership` walks every path and rejects a use of a
  moved name. A move in one arm of an `if` counts as a move after it; a `give
  back` ends that path. `copy of` is the only way to duplicate an owned value.
- Runtime guards turn three machine-level failures into Rusk sentences on
  standard error with exit status 1: division by zero, heap exhaustion, and
  recursion deeper than 1000 calls.
- Everything is position-independent-free and fixed-address; there is no
  relocation pass at runtime.

## What v1.0 doesn't do yet

Honest list, so the docs don't lie:

- Text is immutable. Joining allocates from a fixed 1 MiB heap, and nothing is
  reclaimed. Running past the end is now a Rusk error rather than a crash.
- Ownership is single-owner. A `view` lends a value read-only, but a view
  cannot be changed, moved, stored, or used where an owner is needed; `copy of`
  is still the escape hatch. Views do not extend a value's lifetime beyond the
  frame that owns it.
- Function values are one value at a time: they cannot be stored in lists,
  compared, copied, printed, or given back. A function value cannot be passed
  to a function, because parameter kinds have no function form yet. Calls
  through a function value check the number of arguments, not their kinds.
- Lists are homogeneous and may nest, but they cannot be compared. Their
  contents can be replaced with `set item`, `add VALUE to LIST` appends with
  spare capacity, and `remove last from LIST` / `remove item N from LIST`
  shorten them. Capacity never shrinks, and removal does not give memory back
  to the heap.
- System I/O is intentionally small: text files and standard streams only, no
  directories, sockets, processes, or random access. A path is limited by the
  4 KiB scratch area, and reading stops when the 1 MiB heap is full.
- Imports have two styles: plain textual inclusion in one global namespace, or
  named/selective modules. There are no re-exports, package manifests, or a
  standard module registry.
- Text positions count bytes, not Unicode characters.
- At most 8 parameters per function. There are no function-typed parameters and
  no overloading.
- Linux only, on AArch64 or x86-64. There is no macOS, Windows, or 32-bit
  backend.
- The x86-64 backend keeps virtual registers in memory, so it is slower and
  larger than the AArch64 output. It is correct first, fast later.
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
