# The Rusk Guide

A hands-on guide to **Rusk**, an English-like language that bakes ahead of
time. You write `.rk` files, the compiler emits AArch64 or x86-64 machine code
directly, and the result is a standalone static ELF64 Linux executable with no
runtime, no libc, and no dependencies.

This guide is for v1.1.3. For the exact language rules, see
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
parser, an import expander, a type checker, an ownership checker, two
machine-code backends (AArch64 and x86-64), and an ELF writer. It does not call
an assembler or a linker, and baked programs do not link against libc.

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
- **Linux on AArch64 or x86-64** — both backends encode machine instructions
  by hand; the target defaults to the compiler host.
- **Static ELF64** — one load segment, no dynamic section, no relocations.
- **No libc** — output and file I/O are raw Linux syscalls.
- **`.rk` files** — source is UTF-8 and line-based.
- **One owner** — text and lists are moved, not aliased; `copy of` duplicates.
- **No external TUI crate** — the full-screen session uses `termios` and ANSI.

A baked hello world is a little over 3 KB on AArch64 and about 12 KB on
x86-64.

## 2. Install and build

You need Rust and Cargo. The compiler itself runs on the host; the programs it
produces run on **Linux**, on either **AArch64** or **x86-64**. Bakes default to
the host architecture.

The compiler host can be Linux on AArch64 or x86-64, or Windows on x86-64.
Builds and checks are identical on Windows, but `rusk run` cannot execute the
Linux ELF it produces; build it and run it under WSL or on a Linux machine.

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
cargo run -- build examples/hello.rk --target x86_64
```

To skip the build, install a prebuilt release with `toastset`:

```console
curl --proto '=https' --tlsv1.2 -sSf \
  https://raw.githubusercontent.com/Rusk-programing-language/Rusk/main/toastset | sh
```

It picks a static musl build on Termux, Alpine, and other musl systems, and a
glibc build on ordinary Linux. Running it again upgrades in place. A progress
bar appears while the archive downloads.

On Windows (x86-64), run the same installer from Git Bash, MSYS2, or Cygwin.
It installs `rusk.exe`, adds the bin directory to your shell profile, and
updates the Windows user `PATH` via PowerShell. New terminals then find `rusk`
automatically. Use `--no-modify-path` to skip both profile and `PATH` edits, or
`--bin-dir` to pick the installation directory.

Check the version:

```console
$ rusk version
rusk 1.1.3 — code, baked ahead of time
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
baked hello.rk -> hello (3.1 KB, native aarch64, 0 dependencies)
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

The executable is a real Linux program. `file` reports an ELF64 binary for the
chosen target (AArch64 here, or x86-64), and it keeps working after you delete
the `.rk` file.

## 4. The command-line tools

```text
rusk                              start the interactive session
rusk repl                         start the interactive session
rusk tui                          start the full-screen session
rusk tui demo                     open the terminal module demo
rusk build <file.rk> [-o <out>] [--target <arch>]
                                  bake a native executable
rusk run   [--target <arch>] <file.rk> [args...]
                                  bake it and run it, passing arguments on
rusk check <file.rk> [--target <arch>]
                                  bake without writing an executable
rusk version
rusk help
```

Notes:

- `build` writes next to the source and strips the `.rk` extension unless
  `-o` names a destination.
- `run` passes later words through to `argument 1`, `argument 2`, and so on.
- `check` runs the lexer, parser, imports, type checker, ownership checker, and
  code generator, but writes nothing. Use it in scripts and editors.
- `--target aarch64` or `--target x86_64` picks the instruction set. Without
  it, the compiler chooses its own architecture. `arm64`, `x86-64`, and `amd64`
  are accepted aliases.
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

### Choosing a target

Normally you do not need to think about this: `rusk build` and `rusk run`
target the machine the compiler is running on, so the result executes directly.

To produce a binary for the other architecture, pass `--target`:

```console
$ rusk build hello.rk --target x86_64
baked hello.rk -> hello (3.1 KB, native x86_64, 0 dependencies)
$ rusk check hello.rk --target aarch64
hello.rk looks good — would bake to 3.1 KB (native aarch64, 0 dependencies)
```

Both outputs are static ELF64 files. The x86-64 file has `e_machine = 62`; the
AArch64 file has `e_machine = 183`. `rusk run --target ...` works only when the
host can execute that architecture, for example through `binfmt_misc` or a
user-mode emulator such as `qemu-x86_64`.

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
value. A function may have up to 128 parameters.

Declared kinds are `a number`, `text`, `a truth value`, and list kinds such as
`a list of numbers`, `a list of text`, `a list of truth values`, or
`a list of lists of numbers`. Function kinds are written
`a function taking a number giving back a number`; a function with no
parameters is `a function giving back a number`, and a list of functions is
`a list of functions taking ... giving back ...`.

Top-level `let` names are visible inside functions. A function can read a
global and change it with `set` or `add`, but cannot move it. A parameter or
local may shadow a global:

```rusk
let total be 0

to bump with n as a number:
    set total to total + n

bump with 2
bump with 3
say total                   # 5
```

#### Function values

A named function can be kept as a value and called later:

```rusk
to double with n as a number:
    give back n * 2

let twice be the function double
say call twice with 21      # 42
say twice with 4            # the ordinary spelling works too
```

`call VALUE with ...` calls any function value. Function values are ordinary:
they can be stored, copied, printed (`<function>`), put in a list, passed as an
argument, and given back. They cannot be compared. A dynamic call checks both
the number of arguments and their kinds.

```rusk
to apply with f as a function taking a number giving back a number and n as a number:
    give back call f with n

say apply with the function double and 21    # 42

let fs be [the function double, the function double]
say call (item 2 of fs) with 3               # 6
```

Parenthesize a call on a list item: `call (item 1 of fs) with 3`.

#### Closures

A function written with `a function with ...:` is a closure. It remembers the
names that were visible where it was created:

```rusk
let base be 10
let add_base be a function with n as a number:
    give back n + base

say call add_base with 5    # 15
```

Numbers and truth values are copied into the closure. Text and lists are
captured read-only, so the closure can read them but cannot move or change them.
Closures can be nested and can capture other function values:

```rusk
to make_adder with n as a number:
    let add be a function with x as a number:
        give back x + n
    give back call add with 5

say make_adder with 100     # 105
```

A function can give back a function, and the result can be captured or called
straight away:

```rusk
to make_adder with n as a number:
    give back a function with x as a number:
        give back x + n

let add5 be make_adder with 5
say call add5 with 10       # 15
```

A dynamic call checks both how many arguments it gets and their kinds.

#### Borrowed views

A parameter written `as a view of ...` borrows its argument instead of moving
it:

```rusk
to total with xs as a view of a list of numbers:
    let sum be 0
    for each x in xs:
        set sum to sum + x
    give back sum

let loaves be [3, 6, 9]
say total with loaves       # 18
say length of loaves        # 3; the call did not move it
```

A view is read-only: it can be read, indexed, measured, iterated, and passed to
another view, but not changed or moved. `a view of VALUE` makes a shared alias
with the same rules:

```rusk
let loaves be [3, 6, 9]
let shared be a view of loaves
say item 2 of shared        # 6
say loaves                  # [3, 6, 9]; loaves is still the owner
```

## 6. Lists, growth, and removal

A list is homogeneous: all its items have the same kind. That kind can itself
be a list, so lists nest to any depth.

```rusk
let loaves be [3, 6, 9]
let names be ["rye", "wheat"]
let flags be [true, false, true]

let grid be [[1, 2], [3, 4], an empty list of numbers]
let cube be [[[1, 2]], [[3, 4]]]
```

Every sibling must have the same shape: `[[1], [2, 3]]` is fine, but
`[[1], "two"]` is not.

An empty list needs its kind written down:

```rusk
let empty_numbers be an empty list of numbers
let empty_names be an empty list of text
let empty_flags be an empty list of truth values
let empty_grid be an empty list of lists of numbers
```

`an` is optional: `empty list of text` also works. Each `lists of` adds one level
of nesting, and the innermost word is `numbers`, `text`, or `truth values`.

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
a new block with at least four slots or twice the old capacity, copies the
items over, and hands the old block back to the heap.

### Removing

v0.7 added two removal forms, unchanged in v1.1:

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
cannot be used after that. For nested lists, `copy of` walks every level, so
the copy shares no inner list with the original.

```rusk
let grid be [[1, 2], [3]]
let spare be copy of grid
set item 2 of item 1 of spare to 9
say grid                    # [[1, 2], [3]]
say spare                   # [[1, 9], [3]]
```

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

There is one exception to one-owner-at-a-time: a **view** lends a value
read-only. A borrowed parameter (`as a view of ...`) or `a view of VALUE` may
read the value but cannot move, change, or store it. The owner keeps using its
value, and the view never becomes a second writer. Use `copy of` when you need
an independent value to change or keep.

```rusk
let words be ["rye", "wheat"]
let shared be a view of words   # shared reads the same list
say item 1 of shared            # rye
say length of words             # 2; words was not moved

# add "spelt" to shared         # compile error: a view is read-only
```

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

`all arguments` gives the whole command line as a list of text, which is handy
for loops:

```rusk
for each arg in all arguments:
    say "[" + arg + "]"
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
the 4 KiB scratch area, is a Rusk error.

### Directories and paths

```rusk
make directory "bakery"
say file exists "bakery"                 # true

write "rye" to file "bakery/list.txt"
say list directory "bakery"              # [list.txt]

rename "bakery/list.txt" to "bakery/old.txt"
remove file "bakery/old.txt"
remove directory "bakery"
```

- `make directory PATH` creates one directory. An existing directory is success.
- `file exists PATH` gives back a truth value and follows the usual filesystem
  rules for links and permissions.
- `list directory PATH` gives back a list of entry names as text, without `.`
  and `..`.
- `rename OLD to NEW` moves or renames a file or directory.
- `remove file PATH` deletes a file; `remove directory PATH` deletes an empty
  directory. Both ignore a path that is already gone.

### Running commands

```rusk
say run command "echo from the shell"    # prints from the shell, then 0
say run command "exit 3"                 # 3
```

`run command TEXT` hands the text to `/bin/sh -c`, lets the child share standard
input, output, and error, and gives back the exit status. A missing shell or a
failed fork also gives back `127`.

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

A plain import is textual inclusion into one global namespace. Modules that
want separation use a name:

```rusk
import "lib/math" as math

say math.double with 21
say math.answer
```

Every top-level definition in `lib/math.rk` now lives under `math.`, and the
module refers to its own names normally. A namespaced module's own plain
imports are private: helpers it pulls in get the same prefix and do not leak
into your program.

Selective imports bring just the names you ask for, with no prefix:

```rusk
from "lib/math" import double
from "lib/text" import trim, pad as pad_left

say double with 21
say pad_left with "x"
```

`from` gives the file a namespace based on its stem (`math`, `text`) and
rewrites the chosen names. `as` makes a local alias. Two selective imports
cannot claim the same local name; give one an alias. If two files share a stem,
the second namespace becomes `math_2`, then `math_3`, and so on.

```rusk
import "lib/thing" as thing
say thing.use_hidden with 4   # hidden belongs to thing
```

`export` is the include form that also marks a file's public names:

```rusk
export "lib/greetings"                     # include everything, globally
export "lib/greetings" as greet            # include it under a namespace
export double, greet from "lib/greetings"  # include only those names
export double, greet                       # declare exported names
```

`export "file"` copies the whole file in, just like a plain `import`;
`export ... as` and `export NAME from ...` match the named and selective
import forms. The form without a file checks that each name exists at the top
level.

A file can stop early with a boundary line. A line containing only `end`,
`end.`, `<e>`, `</e>`, or `end of file` ends the file; anything after it is
ignored. This makes it safe to join two files, and it stops an exported file at
its own boundary:

```rusk
# math.rk
to double with n as a number:
    give back n * 2
end

# main.rk
export "math.rk"
say double with 21        # 42
```

## 10. The interactive session

Run `rusk` with no arguments (or `rusk repl`) to start a line-oriented session:

```console
$ rusk
░█▀▄░█░█░█▀▀░█░█
░█▀▄░█░█░▀▀█░█▀▄
░▀░▀░▀▀▀░▀▀▀░▀░▀

Rusk 1.1.3 — an English-like language
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
burnt: line 9: the program ran out of memory
burnt: line 11: I can't list that directory
```

A runtime error stops the program. In the REPL/TUI, the session stays alive
and you can continue typing.

### Common fixes

- **`command not found: rusk`** — use `cargo run -- ...` from the repository,
  or put `target/release/rusk` on your `PATH`.
- **The binary won't run** — baked programs are Linux only, for AArch64 or
  x86-64. Check the target with `file ./program` and the host with `uname -m`;
  cross-baked programs need an emulator or `binfmt_misc`. On a Windows host,
  use the build and run it under WSL or on Linux, because Windows cannot start
  an ELF file directly.
- **`rusk build` writes nothing** — baking failed; read the message. `rusk
  check` is useful for editor/CI loops.
- **A list keeps growing memory usage** — the heap grows in `mmap` chunks.
  Freed list and input buffers are reused, but text joining still allocates.
- **`run command` says 127** — `/bin/sh` could not be started, or the shell
  exited with 127 itself.
- **Text positions look odd** — `length of`, `letter`, and `part` count bytes,
  not Unicode characters.

## 13. Project layout and testing

```text
Cargo.toml            package metadata and the rusk binary
README.md             overview and quick start
docs/SPEC.md          exact language specification
guide/README.md       this guide
examples/             small runnable .rk programs
src/imports.rs        resolve imports and apply module namespaces
src/lexer.rs          tokens, indentation markers
src/parser.rs         recursive-descent parser
src/types.rs          kinds and return-kind inference
src/ownership.rs      single-owner checking
src/codegen.rs        AST to target-independent machine operations
src/backend.rs        target selection and the common assembly interface
src/aarch64.rs        hand-written AArch64 instruction encoder and fixups
src/x86_64.rs         hand-written x86-64 encoder with a register allocator
src/elf.rs            static ELF64 writer for either target
src/repl.rs           tree-walking evaluator and line session
src/tui.rs            native full-screen terminal UI
src/main.rs           command-line interface
tests/e2e.rs          bake and run compiled programs
tests/cli.rs          CLI and piped-session tests
selfhost/             a Rusk interpreter written in Rusk
```

Run the test suite:

```console
cargo test
```

The suite covers unit tests in each compiler pass, CLI behavior, and
end-to-end tests that bake real executables and run them. By default the
end-to-end tests use the host backend. To run them against x86-64 on another
host, set `RUSK_E2E_TARGET` and `RUSK_E2E_RUNNER`:

```console
RUSK_E2E_TARGET=x86_64 RUSK_E2E_RUNNER=qemu-x86_64 cargo test --test e2e
```

The bootstrap interpreter is a Rusk program too, so it runs through the same
pipeline:

```console
cargo run -- run selfhost/interpreter.rk selfhost/demo.rk
```

It prints `42`, `15`, and `51`.

For a quick smoke test:

```console
cargo run -- run examples/hello.rk
cargo run -- run examples/lists.rk
cargo test --quiet
```

## 14. Limits and roadmap

Implemented and checked:

- Static ELF64 output for Linux, on AArch64 or x86-64, with no libc
- English expressions, conditions, loops, and functions
- Constants, text operations, and imports
- Homogeneous lists, including nested lists, with `item`, `set item`, `add`,
  `remove last`, and `remove item N`
- Module namespaces (`import ... as`), selective imports (`from ... import`),
  and `export` content/name selection with `end` / `<e>` file boundaries
- Single-owner checking with `copy of`
- Shared read-only views (`a view of ..., as a view of ...`)
- Function values (`the function NAME`, `call ... with ...`) and closures
- Function-typed parameters, lists of functions, and functions that give back
  functions
- Top-level names visible inside functions, with local shadowing
- A register-allocating x86-64 backend and a relocation-aware data layout
- Arguments, environment, standard input, files, `warn`, and `stop with`
- Directory creation, listing, renaming, removal, and existence checks
- `all arguments` and shell commands with `run command`
- A growable `mmap`-backed heap with a free list
- Line REPL and native full-screen TUI
- A Rusk-in-Rusk interpreter (`selfhost/interpreter.rk`)

Not implemented yet:

- Full compiler self-hosting; the interpreter is the bootstrap stage
- A global x86-64 register allocator that keeps values across branches and
  calls
- Sockets, threads, signals, terminal primitives in baked programs, and GUI
  toolkits
- A package registry

Backend limits worth knowing:

- The image is placed at a fixed base and is not position-independent, but the
  data area follows the final text size, so large programs grow instead of
  colliding.
- Lists and joined text share a growable heap made of mapped chunks. Freed list
  and input buffers are reused; other allocations are not reclaimed.
- List capacity never shrinks.
- A list can hold at most one kind, and lists cannot be compared.
- Function values cannot be compared. Dynamic calls check arity and argument
  kinds.
- At most 128 parameters per function.
- The single load segment is RWX.

For the full details, read [`docs/SPEC.md`](../docs/SPEC.md).
