# The Rusk Guide

A hands-on guide to **Rusk**, an English-like language that bakes ahead of
time. You write `.rk` files, the compiler emits AArch64 machine code directly,
and the result is a standalone static ELF64 Linux executable with no runtime,
no libc, and no dependencies.

This guide is for v0.7. For the exact language rules, see
[`docs/SPEC.md`](../docs/SPEC.md). For a quick overview, see the
[`README`](../README.md).

## Contents

1. [What makes Rusk different](#1-what-makes-rusk-different)
2. [Install and build](#2-install-and-build)
3. [Your first program](#3-your-first-program)
4. [The command-line tools](#4-the-command-line-tools)
5. [Language tour](#5-language-tour)
6. [Lists, growth, and removal](#6-lists-growth-and-removal)
7. [Ownership](#7-ownership)
8. [Talking to the system](#8-talking-to-the-system)
9. [Imports](#9-imports)
10. [The interactive session](#10-the-interactive-session)
11. [The full-screen session](#11-the-full-screen-session)
12. [Errors and troubleshooting](#12-errors-and-troubleshooting)
13. [Project layout and testing](#13-project-layout-and-testing)
14. [Limits and roadmap](#14-limits-and-roadmap)

---

## 1. What makes Rusk different

Rusk is deliberately small. Its whole compiler is a lexer, a recursive-descent
parser, an import expander, a type checker, an ownership checker, an AArch64
code generator, and an ELF writer. It does not call an assembler or a linker,
and baked programs do not link against libc.

The vocabulary is English-shaped:

```rusk
let temperature be 180
if temperature is more than 150:
    say "Too hot."
otherwise:
    say "Just right."
```

The constraints are part of the design:

- **Ahead of time** — no interpreter ships with your program.
- **AArch64 Linux only** — the backend encodes AArch64 instructions by hand.
- **Static ELF64** — one load segment, no dynamic section, no relocations.
- **No libc** — output and file I/O are raw Linux syscalls.
- **`.rk` files** — source is UTF-8 and line-based.
- **One owner** — text and lists are moved, not aliased; `copy of` duplicates.
- **No external TUI crate** — the full-screen session uses `termios` and ANSI.

A baked hello world is a little over 3 KB.

## 2. Install and build

You need Rust and Cargo. The compiler itself runs on the host; the programs it
produces run on **AArch64 Linux**.

```console
git clone https://github.com/Rusk-programing-language/Rusk.git
cd Rusk
cargo build --release
```

The binary lands in `target/release/rusk`. Put it somewhere on your `PATH`, or
call it directly. During development you can also use `cargo run --`:

```console
cargo run -- --help
cargo run -- run examples/hello.rk
```

Check the version:

```console
$ rusk version
rusk 0.7.0 — code, baked ahead of time
```

## 3. Your first program

Create `hello.rk`:

```rusk
# hello.rk
say "Hello, world."
```

Bake it:

```console
$ rusk build hello.rk
baked hello.rk -> hello (3.1 KB, native, 0 dependencies)
```

Run it:

```console
$ ./hello
Hello, world.
```

You can also bake and run in one step:

```console
$ rusk run hello.rk
Hello, world.
```

The executable is a real Linux program. `file` reports an ELF64 AArch64
binary, and it keeps working after you delete the `.rk` file.

## 4. The command-line tools

```text
rusk                              start the interactive session
rusk repl                         start the interactive session
rusk tui                          start the full-screen session
rusk tui demo                     open the terminal module demo
rusk build <file.rk> [-o <out>]   bake a native executable
rusk run   <file.rk> [args...]   bake it and run it, passing arguments on
rusk check <file.rk>              bake without writing an executable
rusk version
rusk help
```

Notes:

- `build` writes next to the source and strips the `.rk` extension unless
  `-o` names a destination.
- `run` passes later words through to `argument 1`, `argument 2`, and so on.
- `check` runs the lexer, parser, imports, type checker, ownership checker, and
  code generator, but writes nothing. Use it in scripts and editors.
- A failed bake writes no output file.

Example with arguments:

```rusk
# args.rk
say number of arguments
say argument 1
say argument 2
```

```console
$ rusk run args.rk rye wheat
2
rye
wheat
```

## 5. Language tour

### Comments and layout

A `#` starts a comment that runs to the end of the line. Blank lines are
ignored. Blocks start with `:` and continue with indentation.

```rusk
# This is a comment.
say "hi"   # so is this
```

### Values

| Kind | Examples | Notes |
|---|---|---|
| Number | `42`, `-7` | Signed 64-bit integer |
| Text | `"hello"` | Immutable, UTF-8 bytes |
| Truth | `true`, `false` | Result of comparisons and `not` |
| List | `[1, 2, 3]`, `["a", "b"]` | One item kind, positions from 1 |

There is no floating point. Integer overflow wraps like two's-complement
machine arithmetic.

Text escapes: `\n`, `\t`, `\r`, `\0`, `\\`, `\"`.

### Variables and constants

```rusk
let temperature be 180
set temperature to temperature + 5
say temperature             # 185

constant batch_size be 12
say batch_size * 2          # 24
# set batch_size to 13      # compile error: it's a constant
```

`variable` is a synonym for `let`. A name cannot change its kind later.

### Arithmetic and comparisons

```rusk
say 10 - 4 * 2              # 2
say (10 - 4) * 2            # 12
say 100 / 3                 # 33
say -15 + 3                 # -12

say 1 is less than 2
say 2 is at most 2
say 3 is not 4
say true and not false
say false or true
```

Comparison phrases:

```text
is                          equal to
is equal to                 equal to
is not                      not equal
is less than                <
is more than                >
is at least                 >=
is at most                  <=
```

Symbolic operators (`=`, `!=`, `<`, `<=`, `>`, `>=`) also work.

### Conditions

```rusk
let temperature be 180

if temperature is more than 150:
    say "Too hot. The crust will burn."
otherwise if temperature is at least 100:
    say "Bake a little longer."
otherwise:
    say "Just right."
```

### Loops

```rusk
repeat 3 times:
    say "one loaf"

let n be 0
while n is less than 3:
    say n
    set n to n + 1

for each slice from 1 to 3:
    say slice

let names be ["rye", "wheat"]
for each name in names:
    say "one " + name + " loaf"
```

`break` leaves the nearest loop. `continue` skips to the next turn.

### Text

```rusk
let name be "Rusk"
say "Hello, " + name + "!"      # Hello, Rusk!
say length of name              # 4
say letter 1 of name            # R
say part 2 to 3 of name         # us
say "abc" is less than "abd"
```

Text compares byte by byte, then by length, like a dictionary. Positions count
from 1. `part 2 to 1 of s` gives empty text. A position outside the text is a
runtime error.

### Functions

Parameters declare their kind. The return kind is inferred from `give back`.

```rusk
to double with n as a number:
    give back n * 2

to greet with who as text:
    say "Hello,"
    say who

to roll:
    say "rolling..."

say double with 21          # 42
greet with "world"
roll                        # zero-argument call is just the name

to factorial with n as a number:
    if n is at most 1:
        give back 1
    give back n * factorial with n - 1

say factorial with 5        # 120
```

A function with no `give back` gives nothing back and cannot be used as a
value. At most 8 parameters are allowed. There are no closures or function
values.

Declared kinds are `a number`, `text`, `a truth value`, `a list of numbers`,
`a list of text`, and `a list of truth values`.

## 6. Lists, growth, and removal

A list is homogeneous: numbers, text, or truth values, never a mixture and
never nested lists.

```rusk
let loaves be [3, 6, 9]
let names be ["rye", "wheat"]
let flags be [true, false, true]
```

An empty list needs its kind written down:

```rusk
let empty_numbers be an empty list of numbers
let empty_names be an empty list of text
let empty_flags be an empty list of truth values
```

`an` is optional: `empty list of text` also works.

### Length, items, and replacement

```rusk
let xs be [10, 20, 30]
say length of xs            # 3
say item 2 of xs            # 20
set item 2 of xs to 99
say xs                      # [10, 99, 30]
```

Positions count from 1. `set item` requires the same item kind as the list.

### Adding

```rusk
let xs be [10, 20, 30]
add 40 to xs
say xs                      # [10, 20, 30, 40]
say length of xs            # 4

let words be ["rye"]
add "wheat" to words
say words                   # [rye, wheat]
```

A text value moved into a list cannot be used afterwards; use `copy of` if you
need to keep it.

Behind the scenes, a list stores both a length and a capacity. `add` writes
into spare capacity when it can. When the list is full, the runtime allocates
a new block with at least four slots or twice the old capacity and copies the
items over. Old blocks are not reclaimed.

### Removing

v0.7 adds two removal forms:

```rusk
let xs be [10, 20, 30]
remove last from xs
say xs                      # [10, 20]

remove item 1 from xs
say xs                      # [20]

add 40 to xs
add 50 to xs
remove item 2 from xs
say xs                      # [20, 40]
```

Removal keeps the remaining items in order and shifts later items left.
Capacity does not shrink, so adding again can reuse it.

Removing from an empty list or asking for an item outside the list stops the
program with a Rusk error:

```console
burnt: line 3: there's nothing to remove
burnt: line 3: an item outside the list
```

You cannot remove from a constant:

```rusk
constant xs be [1, 2]
remove last from xs         # compile error: constant
```

### Copying lists

```rusk
let xs be [1, 2, 3]
let ys be copy of xs
remove item 2 from ys
say xs                      # [1, 2, 3]
say ys                      # [1, 3]
```

The copy is independent. Plain `let ys be xs` moves the list instead; `xs`
cannot be used after that.

## 7. Ownership

Every text value and every list has exactly one owner. These operations **move**
the value:

- `let` or `constant` binding a value
- `set NAME to VALUE`
- passing a value to a function
- putting a value in a list literal or `add`ing it
- `give back`

After a move, the old name is dead:

```rusk
let greeting be "hello"
let echoed be greeting      # moves greeting
say echoed                  # hello
# say greeting              # compile error: greeting was moved
```

`copy of` makes a fresh value instead:

```rusk
let words be ["rye", "wheat"]
let mine be copy of words   # independent copy
say words
say mine
```

Numbers and truth values are plain machine words. They are copied freely and
never move.

```rusk
let n be 3
let m be n                  # fine
say n + m                   # 6
```

Constants are read-only. Copying them is allowed; moving them is not.

The checker is flow-sensitive. A move in one branch of an `if` counts as a move
after the `if`, and `give back` ends the current path. The rule is checked
before code generation, so a rejected program never produces an executable.

There is no borrowing and no shared read-only view: a value has one owner at a
time. That rules out aliasing bugs by refusing to have aliases at all.

## 8. Talking to the system

Rusk programs use Linux syscalls directly. No library is required.

### Arguments and environment

```rusk
say number of arguments       # arguments after the program's own name
say argument 1                # empty text when there is no such argument
say environment "HOME"        # empty text when the variable is unset
```

```console
$ rusk run args.rk one two
2
one
two
```

### Standard input

```rusk
let name be read line         # one line, without its newline
let rest be read all input    # everything left on standard input
```

Example:

```rusk
say "What is your name?"
let name be read line
say "Hello, " + name + "!"
```

```console
$ printf 'Ada\n' | rusk run greet.rk
What is your name?
Hello, Ada!
```

### Files

```rusk
let notes be read file "notes.txt"
write "baked fresh" to file "loaf.txt"
```

Paths are used as written. A missing or unreadable file, or a path longer than
the 4 KiB scratch area, is a Rusk error. Reading stops if the 1 MiB heap fills.

### Standard error and exit status

```rusk
warn "using the spare oven"   # standard error, with a newline
stop with 2                   # choose the process exit status
```

`warn` writes to descriptor 2. `stop with N` exits immediately. In the
full-screen session, `stop with` restores the terminal first.

## 9. Imports

`import` includes another file once. Paths are resolved next to the importing
file first, then inside a `ruskmods/` folder next to it. Cycles are detected.

```rusk
import "lib/greetings"
import "../ruskmods/math"

say greeting with "world"
say math_square with 12
```

Imports are textual inclusion into one global namespace. There are no module
namespaces or selective imports, so modules prefix their names (`math_square`,
`text_trim`).

## 10. The interactive session

Run `rusk` with no arguments (or `rusk repl`) to start a line-oriented session:

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

Names and functions survive between lines. A bare expression shows its value;
`let`, `set`, `say`, and definitions stay quiet. A line ending in `:` opens a
block, and a blank line closes it. The session uses the same lexer, parser, and
type checker as the compiler.

Commands:

```text
:help            show the commands
:quit            leave the session
:reset           forget every name and function
:vars            show the names the session remembers
:functions       show the functions the session remembers
:load <file.rk>  run a file in the session
```

Piped input skips the banner and prompts, so `rusk < program.rk` also works.

## 11. The full-screen session

`rusk tui` starts the same session in a full-screen interface. It keeps the
transcript, input line, history, and `:commands` on one screen, with results,
multi-line blocks, and errors scrolling above the prompt.

Editing supports arrows, Home/End, Delete, Page Up/Down, and UTF-8. Up/Down
recall earlier lines, Ctrl-C cancels the current line or block, Ctrl-L clears
the transcript, and Ctrl-D leaves.

The UI is native: it puts the terminal into raw mode through `termios`, draws
ANSI frames, notices window resizes, and restores the terminal on the way out,
including when a program calls `stop with`. There is no `crossterm` or
`ratatui`. Over a pipe, both commands fall back to plain text instead of
writing escape sequences.

`rusk tui demo` opens the smaller terminal-module demo.

## 12. Errors and troubleshooting

### Bake-time errors

Bake-time errors are reported with a line number and no executable is written:

```console
$ rusk build bad.rk
burnt: line 3: I don't know a thing called 'x'
```

Common causes:

| Message | Meaning |
|---|---|
| `I don't know a thing called 'x'` | A name is undeclared or out of scope |
| `'x' was moved already` | An owned value was used after being moved; use `copy of` |
| `Rusk won't change a thing's kind` | A name was assigned a value of another kind |
| `'x' is a constant` | A `set` or `remove` tried to change a constant |
| `this list holds a number` | A list item or `add` used the wrong kind |
| `empty list needs a kind` | Write `an empty list of numbers` (or text/truth values) |
| `'remove' needs a list` | The remove target is not a list |
| `the item number must be a number` | A list position was not a number |

### Runtime errors

Runtime errors print `burnt: line N: ...` on standard error and exit with
status 1:

```console
burnt: line 4: an item outside the list
burnt: line 5: there's nothing to remove
burnt: line 2: division by zero
burnt: line 7: recursion went too deep
burnt: line 9: the heap is full
```

A runtime error stops the program. In the REPL/TUI, the session stays alive
and you can continue typing.

### Common fixes

- **`command not found: rusk`** — use `cargo run -- ...` from the repository,
  or put `target/release/rusk` on your `PATH`.
- **The binary won't run** — baked programs are AArch64 Linux only. Check with
  `uname -m`.
- **`rusk build` writes nothing** — baking failed; read the message. `rusk
  check` is useful for editor/CI loops.
- **A list keeps growing memory usage** — the heap is bump-allocated. Growth
  and removal never reclaim old blocks.
- **Text positions look odd** — `length of`, `letter`, and `part` count bytes,
  not Unicode characters.

## 13. Project layout and testing

```text
Cargo.toml            package metadata and the rusk binary
README.md             overview and quick start
docs/SPEC.md          exact language specification
guide/README.md       this guide
examples/             small runnable .rk programs
src/imports.rs        resolve and expand imports
src/lexer.rs          tokens, indentation markers
src/parser.rs         recursive-descent parser
src/types.rs          kinds and return-kind inference
src/ownership.rs      single-owner checking
src/codegen.rs        AST to AArch64 machine code
src/aarch64.rs        hand-written instruction encoder and fixups
src/elf.rs            static ELF64 writer
src/repl.rs           tree-walking evaluator and line session
src/tui.rs            native full-screen terminal UI
src/main.rs           command-line interface
tests/e2e.rs          bake and run compiled programs
tests/cli.rs          CLI and piped-session tests
```

Run the test suite:

```console
cargo test
```

The suite covers unit tests in each compiler pass, CLI behavior, and
end-to-end tests that bake real executables and run them. For a quick
smoke test:

```console
cargo run -- run examples/hello.rk
cargo run -- run examples/lists.rk
cargo test --quiet
```

## 14. Limits and roadmap

Implemented and checked:

- AArch64 Linux static ELF64 output, no libc
- English expressions, conditions, loops, and functions
- Constants, text operations, and imports
- Homogeneous lists with `item`, `set item`, `add`, `remove last`, and
  `remove item N`
- Single-owner checking with `copy of`
- Arguments, environment, standard input, files, `warn`, and `stop with`
- Line REPL and native full-screen TUI

Not implemented yet:

- Nested lists
- Borrowing and shared read-only views
- Module namespaces and selective imports
- Function values and closures
- An x86-64 backend
- Relocation-aware data layout
- Self-hosting Rusk in Rusk

Backend limits worth knowing:

- The image is placed at a fixed base and data sits at `0x410000`; very large
  programs can collide.
- Lists and joined text share a 1 MiB bump heap. Nothing is reclaimed.
- List capacity never shrinks.
- A list can hold at most one kind, and lists cannot be compared.
- At most 8 parameters per function.
- The single load segment is RWX.

For the full details, read [`docs/SPEC.md`](../docs/SPEC.md).
