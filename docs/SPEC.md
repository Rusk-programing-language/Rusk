# The Rusk Language — Specification v0.7

Rusk is a small, English-shaped language with an ahead-of-time compiler that
emits native machine code directly. This document describes exactly what v0.7
accepts and what it guarantees.

Status: **v0.7, AArch64 Linux**. Everything here is implemented and tested;
anything not here does not exist yet.

---

## 1. Files

Rusk source files use the extension **`.rk`**. Source is UTF-8; only ASCII is
meaningful today for keywords and operators (text literals may contain any
UTF-8 bytes and are printed as-is).

A file is a sequence of lines. Line endings may be `\n` or `\r\n`.

## 2. Comments

A `#` starts a comment that runs to the end of the line.

```rusk
# the whole line is a comment
say "hi"   # so is this part
```

Blank lines are ignored.

## 3. Values

| Kind | Written as | Notes |
|---|---|---|
| Number | `42`, `-7` | Signed 64-bit integer (two's complement) |
| Text | `"hello"` | Immutable; escapes `\n`, `\t`, `\r`, `\0`, `\\`, `\"` |
| Truth | `true`, `false` | The result of comparisons and `not` |
| List | `[1, 2, 3]`, `["a", "b"]`, `[true, false]` | Growable, one kind of item, positions count from 1 |

Numbers are integers only. There is no floating point in v0.7.

Text and lists live on a heap and are **owned**: a name holds one of them at a
time. Ownership is what makes the language memory-safe without a garbage
collector; see §4.1.

## 4. Naming things

```rusk
let slices be 8
set slices to slices + 1

constant batch_size be 12
variable loaves be 3          # exactly the same as: let loaves be 3
```

- `let NAME be VALUE` declares a new thing. Declaring a name twice is an error;
  use `set` to change it.
- `variable NAME be VALUE` is another spelling of `let`.
- `constant NAME be VALUE` declares a name that can never be replaced with
  `set`. Its value is fixed when the line runs.
- `set NAME to VALUE` assigns to an existing thing. The new value must be the
  same kind as the old one; Rusk will not change a thing's kind.

Names may contain letters, digits, and underscores, and may not start with a
digit. Keywords are not reserved — `say`, `let`, `if` and the rest are matched
by position, so a variable literally named `say` is legal (if confusing).

### 4.1 Ownership and copying

Text and lists are the two **owned** kinds. Numbers and truth values are plain
machine words and are copied freely. For an owned value:

- `let NAME be VALUE`, `constant NAME be VALUE`, `set NAME to VALUE`, passing
  an argument, writing an item into a list, and `give back` all **move** the
  value. The old name is empty afterwards.
- Using a name after its value has moved is a **compile error**:

  ```console
  burnt: in app.rk: line 3: 'greeting' was moved already — 'copy of greeting'
  keeps the original
  ```

- `copy of VALUE` makes a fresh deep copy. The source stays usable. Copying a
  list of text copies the list and every text item in it.
- A `constant` is read-only. It can be read but not moved; copy it instead.
- A move inside one arm of an `if` counts as a move after the `if`, because
  that arm might have run. A `give back` ends the path, so it does not poison
  the code that falls through. A move inside a loop body counts as a move
  after the loop, even if the loop might run zero times.

```rusk
let greeting be "hello"
let echoed be greeting        # moves greeting
say echoed                    # hello
# say greeting               # compile error

let words be ["rye", "wheat"]
let mine be copy of words     # an independent list
say words                     # [rye, wheat]
say mine                      # [rye, wheat]
```

Rusk has no null, no pointers, and no threads. A value cannot be observed
through two names, so there is no dangling reference, no double free, and no
shared mutable state to race over. The checker runs before any machine code is
produced; it is not a runtime cost.

## 5. Saying things

```rusk
say "Hello, world."
say 42
say true
```

`say` writes its value to standard output followed by a newline. Text and truth
values print as themselves (`true` / `false`); numbers print in decimal, with a
minus sign when negative.

### System input and output

Rusk programs reach the operating system through a small set of English
statements and primaries. There is no library to import and no host language
runtime:

```rusk
say number of arguments        # how many arguments follow the program name
say argument 1                 # the first argument, or empty text
say environment "HOME"         # the variable's text, or empty text

let name be read line          # one line from standard input, without newline
let rest be read all input     # everything left on standard input
let text be read file "notes.txt"
write "fresh" to file "loaf.txt"

warn "the oven is hot"         # like `say`, but on standard error
stop with 2                    # end now with exit status 2
```

Rules:

- `argument N` counts from 1. A number below 1 or past the last argument gives
  empty text, not an error.
- `number of arguments` counts only the arguments after the program name.
- `environment NAME` takes the variable name as text. An unset variable gives
  empty text.
- `read line` reads bytes up to a newline or end of input and drops the newline
  (and a carriage return before it). `read all input` reads the rest of
  standard input, including newlines.
- `read file PATH` gives the whole file as text. A missing or unreadable file
  stops the program with `burnt: I can't open that file` and status 1.
- `write TEXT to file PATH` replaces the file's contents, creating it with mode
  `0644` when needed. A failure stops the program with
  `burnt: I can't write to that file` and status 1.
- A path longer than 4095 bytes is reported at runtime as
  `burnt: that file path is too long`.
- `warn VALUE` writes the value and a newline to standard error. It accepts the
  same values as `say`.
- `stop with CODE` ends the program immediately with that exit status. It may
  appear in a block or a function; the ownership checker treats the statement
  as ending its path.
- `read line`, `read all input`, `read file`, and `environment` each produce a
  fresh owned text value, so their results follow the same move and `copy of`
  rules as other text.

## 6. Expressions

Operator precedence, loosest first:

| Level | Operators | Associativity |
|---|---|---|
| Or | `or` | left |
| And | `and` | left |
| Comparison | `is`, `is more than`, `is less than`, `is equal to`, `is not`, `is at least`, `is at most`, `>`, `<`, `==`, `!=`, `>=`, `<=` | one comparison per expression |
| Sum | `+`, `-` | left |
| Product | `*`, `/` | left |
| Unary | `-`, `not` | right |
| Primary | number, text, truth, list, name, call, text in pieces, list item, `copy of`, `argument N`, `number of arguments`, `environment NAME`, `read line`, `read all input`, `read file PATH`, `( ... )` | — |

`+` adds two numbers or joins two pieces of text. `-`, `*`, and `/` are numbers
only. A bare `is` means `is equal to`: `if name is "Rusk":`.

`and` and `or` take truth values on both sides and give one back. They
short-circuit: the right side is not evaluated when the left side already
decides the answer. `and` binds tighter than `or`, so
`a or b and c` means `a or (b and c)`. Inside a call's `with ... and ...`
argument list, a top-level `and` separates arguments; put parentheses around a
conjunction you mean as one argument: `check with (a and b)`.

Examples:

```rusk
say 1 + 2 * 3            # 7
say (1 + 2) * 3          # 9
say 100 / 3              # 33   (integer division truncates toward zero)
say 100 / -7             # -14
say 5 is more than 3     # true
say 5 is at most 5       # true
say not (1 is equal to 2)  # true
say true and false       # false
say false or true        # true
say 5 is more than 3 and 2 is less than 4  # true
say "Hello, " + "world"  # Hello, world
say "apple" < "banana"   # true
say "b" is more than "abc"  # true
```

Rules:

- `+` requires two numbers or two pieces of text; kinds never mix.
- `- * /` require numbers on both sides.
- Comparisons require both sides to be the same kind. Numbers and truth values
  compare directly; text compares byte by byte (unsigned), then by length, so
  `"ab"` is less than `"abc"` and `"b"` is more than `"abc"`.
- `not` requires a truth value.
- `and` and `or` require truth values on both sides.
- Unary `-` requires a number.

### Joining text

Text is immutable. `left + right` allocates a new string in a fixed 1 MiB heap
inside the program's data area, copies both byte blobs into it, and hands back
the new value. Nothing is freed: this language is small and programs are short,
but a long-running loop that keeps joining text will eventually run out of heap
and stop with `burnt: the program ran out of memory`.

### Measuring, indexing, and slicing text

Three forms take text apart. They are primaries, so they bind tightly:

| Form | Gives back | Meaning |
|---|---|---|
| `length of TEXT` | a number | how many bytes `TEXT` holds |
| `letter N of TEXT` | text | the byte at position `N`, counting from 1 |
| `part A to B of TEXT` | text | bytes `A` through `B`, inclusive, counting from 1 |

```rusk
let name be "Rusk"
say length of name          # 4
say letter 1 of name        # R
say letter 4 of name        # k
say part 2 to 3 of name     # us
say "Hello, " + letter 1 of "world" + "!"   # Hello, w!
```

Rules:

- `length of` needs text and gives a number. It counts bytes, which matches
  every letter in ASCII. Text literals may contain UTF-8, so a non-ASCII
  character can count as more than one.
- `letter N of` and `part A to B of` need a number index and text. Indexes start
  at **1**; there is no zero position.
- A part is inclusive at both ends. `part 2 to 1 of "Rusk"` is empty text.
- `part` may start one past the end when the part is empty, so
  `part 5 to 4 of "Rusk"` is allowed and empty.
- Asking for a letter or part outside the text is a **runtime error**. The
  program writes a sentence to standard error and exits with status 1:

  ```console
  burnt: line 3: a letter outside the text
  ```

- The text operand is a single primary. Parenthesize a larger expression:
  `length of (a + b)`, `part 2 to (length of s - 1) of s`.

Because these forms are ordinary primaries, joining and comparing compose
naturally: `letter 1 of a + letter 1 of b`, `part 2 to 4 of a is "ell"`.

### Lists

A list is a homogeneous sequence. Write one with square brackets:

```rusk
let loaves be [3, 6, 9]
let names be ["rye", "wheat"]
let flags be [true, false, true]
```

Every item must have the same kind: a list holds numbers, text, or truth
values, never a mixture and never other lists. An empty list cannot guess its
kind, so write it down:

```rusk
let empty_numbers be an empty list of numbers
let empty_names be an empty list of text
let empty_flags be an empty list of truth values
```

`an` is optional: `empty list of text` reads the same. The kind is also written
in function parameters as `a list of numbers`, `a list of text`, or
`a list of truth values`.

Six forms work on lists, all counting from **1**:

| Form | Gives back | Meaning |
|---|---|---|
| `length of LIST` | a number | how many items the list holds |
| `item N of LIST` | the item kind | the item at position `N` |
| `set item N of LIST to VALUE` | — | replace the item at position `N` |
| `add VALUE to LIST` | — | append `VALUE`, growing the list |
| `remove last from LIST` | — | remove the final item |
| `remove item N from LIST` | — | remove the item at position `N` |

```rusk
let xs be [10, 20, 30]
say length of xs          # 3
say item 2 of xs          # 20
set item 2 of xs to 99
say xs                    # [10, 99, 30]
add 40 to xs
say xs                    # [10, 99, 30, 40]
remove last from xs
say xs                    # [10, 99, 30]
remove item 2 from xs
say xs                    # [10, 30]
```

Rules:

- `length of` accepts text or a list. On a list it counts items, not bytes.
- The item number must be a number, evaluated before the list.
- `item N of` and `set item N of` need a list; the new value's kind must match
  the list's item kind.
- `add VALUE to LIST` needs a list variable whose item kind matches `VALUE`.
  Adding an owned text value moves it into the list; `copy of` keeps the
  original. Adding to a constant is an error.
- `remove last from LIST` and `remove item N from LIST` need a list variable
  that is not a constant. The name keeps ownership, and later items shift down
  to fill the gap.
- Asking for an item outside the list, or removing from an empty list, is a
  **runtime error**:

  ```console
  burnt: line 4: an item outside the list
  burnt: line 5: there's nothing to remove
  ```

- A list value is owned by one name at a time. `let ys be xs` moves the list
  from `xs` to `ys`; `ys` and `xs` never alias. Use `let ys be copy of xs` when
  you want two independent lists. `add` and `remove` change the list in place
  from the program's point of view; the name keeps ownership.
- Lists have a capacity behind the scenes. `add` uses spare capacity when it
  can and otherwise reallocates with at least four slots or twice the old
  capacity. `remove` never shrinks the capacity, and the old block is not
  reclaimed when a list grows.
- Lists do not compare with `is` or the ordering operators yet.

## 7. Blocks

`if`, `repeat`, `while`, and `for each` introduce a block: a colon, then an
indented body. The body ends when the indentation returns to the enclosing
level.

```rusk
if condition:
    say "inside"
    say "still inside"
say "outside"
```

Indentation may use spaces or tab stops of four columns. Indenting to a column
that does not match any open block is an error.

## 8. Conditionals

```rusk
if temperature is more than 150:
    say "Too hot."
otherwise:
    say "Just right."
```

`otherwise` is optional. You can chain conditions with `otherwise if`:

```rusk
if score is at least 90:
    say "A"
otherwise if score is at least 70:
    say "B"
otherwise:
    say "C"
```

`otherwise if CONDITION:` is simply an `if` living in the else branch, so only
the first matching branch runs.

The condition must be a truth value. A bare number is not accepted as a
condition — write `is not 0` if that is what you mean.

## 9. Repetition

`repeat` runs the body a fixed number of times:

```rusk
repeat 3 times:
    say "one loaf"
```

The count is evaluated once, before the first iteration. A count of zero or less
runs the body zero times.

`while` runs the body for as long as the condition stays true:

```rusk
let n be 0
while n is less than 3:
    say n
    set n to n + 1
```

The condition is re-evaluated before each iteration and must be a truth value.

`for each` counts through a range, inclusive at both ends:

```rusk
for each slice from 1 to 3:
    say slice       # 1, 2, 3
```

The start is evaluated first, then the end. If the start is greater than the
end, the body runs zero times. The loop name only exists inside the body; after
the loop, the name means whatever it meant before (or nothing).

`for each ... in` walks a list instead of a range:

```rusk
let names be ["rye", "wheat", "spelt"]
for each name in names:
    say "one " + name + " loaf"
```

The list is evaluated once. Items are visited in order, counting from 1, and
the loop name has the list's item kind. The name only exists inside the body,
just like the range form. Changing an item inside the loop is visible on the
next iteration, because the loop reads from the same list.

`break` leaves the nearest enclosing loop immediately:

```rusk
for each word in ["rye", "wheat", "spelt"]:
    if word is "wheat":
        break
    say word            # rye
```

`continue` skips the rest of the body and starts the next turn:

```rusk
for each n from 1 to 5:
    if n is 3:
        continue
    say n               # 1, 2, 4, 5
```

Both only work inside a loop, and both bind to the innermost loop. In a
`repeat` or `for each`, `continue` still advances the counter before the next
turn.

## 10. Functions

```rusk
to double with n as a number:
    give back n * 2

to greet with who as text:
    say "Hello,"
    say who

to roll:
    say "rolling..."
```

- `to NAME:` starts a function with no parameters.
- `to NAME with P as KIND and P as KIND:` declares parameters; `and` separates
  them. A function may have at most 8 parameters.
- A function body is an indented block, just like an `if`.
- `give back VALUE` ends the function and hands `VALUE` to the caller.

Kinds are written `a number`, `text`, `a truth value` (`a boolean` works too),
and `a list of numbers` / `a list of text` / `a list of truth values`. The
`a`/`an` is optional: `n as number` reads the same.

Calling uses the same `with ... and ...` shape:

```rusk
say double with 21          # 42
say double with 3 + 1       # the argument is the whole expression: 8
greet with "world"
roll                        # a zero-argument call is just the name
```

An argument is a full expression, so `double with 3 + 1` passes `3 + 1`.
Parenthesize if you mean something else: `(double with 3) + 1`.

**What a function gives back is inferred.** The compiler looks at every
`give back` in the body; they must all agree on a kind. A function with no
`give back` gives back *nothing* and cannot be used as a value — call it as a
statement. Recursion is supported, and the compiler works out the return kind
from the base case:

```rusk
to factorial with n as a number:
    if n is at most 1:
        give back 1
    give back n * factorial with n - 1
```

Functions live at the top level; defining one inside a block is an error. They
may be called before their definition appears in the file.

## 11. Imports

`import` pulls another file into the program before type checking:

```rusk
import "lib/greetings"
import "../ruskmods/math"
```

The name is either text in quotes or a bare name; the `.rk` extension is
optional, so `import "lib/greetings"` and `import "lib/greetings.rk"` are the
same. Several imports may appear at the top of one file.

Resolution order, for a relative path:

1. next to the file that wrote the import;
2. inside a `ruskmods/` folder next to that file;
3. relative to the current directory;
4. inside a `ruskmods/` folder in the current directory.

An absolute path is used as written. A file is expanded **at most once** per
bake, so several modules can import the same helper without defining it twice.
Imports are recursive: an imported file may import others. A cycle is an error
and names the chain:

```console
burnt: in lib/b.rk: line 1: this import makes a cycle: main.rk -> lib/a.rk -> lib/b.rk -> lib/a.rk
```

`import` only works at the top level, not inside a block. Imported definitions
join one global namespace, so functions should use a prefix (`text_`, `math_`)
until namespaces exist. A missing file, an unreadable file, or a syntax error
inside an imported file names the file it came from.

## 12. Whole programs

A program is executed top to bottom. When the last statement finishes, the
program exits with status 0. There is no `main` and no entry-point ceremony.
`stop with CODE` chooses a different exit status and ends the program
immediately.

```rusk
# hello.rk
say "Hello, world."
```

`rusk run file.rk one two` bakes the file, runs it, and passes `one` and `two`
to the program; `argument 1` is `one`, and `number of arguments` is 2. The
temporary executable keeps the program's name in `argv[0]`, which commands such
as `argument 1` skip automatically.

## 13. The interactive session

`rusk` with no arguments, or `rusk repl`, starts an interactive session:

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

The prompt remembers names and functions between lines. An expression typed on
its own line shows its value, the way Python's prompt does; `let`, `set`, `say`,
and definitions stay quiet. A line ending in `:` opens a block, and a blank line
finishes it.

```text
:help            show the commands
:quit            leave the session
:reset           forget every name and function
:vars            show the names the session remembers
:functions       show the functions the session remembers
:load <file.rk>  run a file in the session
```

The session is a tree-walking evaluator over the same AST, not the AArch64
backend. Every submission is still lexed, parsed, type checked, and ownership
checked against the whole session before any of it runs, so a session accepts
the same programs the compiler does. Text operations use byte positions here
too, and a moved name is empty here just as it is in a baked program. `import`
works at the prompt (relative to the current directory); inside `:load`,
relative imports resolve from the loaded file's directory.

System primaries work at the prompt as well: `argument N` sees the words after
`rusk repl`, `environment` reads the session's environment, and `read line`,
`read all input`, `read file`, `write ... to file`, and `warn` use the same
standard streams and files as a baked program. `stop with CODE` ends the
session with that status.

When stdin is not a terminal, the banner and prompts are omitted, so
`rusk < program.rk` works as a pipe. The session is not a `.rk` file compiler:
it does not write an executable.

### The native terminal UI

`rusk tui` starts the interactive session in a full-screen interface built on
the tool's own terminal module. It is not a wrapper around an external TUI
crate:

- raw mode is set through `termios` (`tcgetattr` / `tcsetattr`), keeping the
  original settings and restoring them on exit or drop;
- frames are built as ANSI escape strings and written in one pass;
- the input decoder handles printable ASCII, UTF-8 split across reads, control
  letters, arrows, Home/End, Insert/Delete, Page Up/Down, and function keys;
- a 100 ms poll loop notices terminal resizes;
- when stdin or stdout is not a terminal, or `TERM=dumb`, `rusk tui` runs the
  ordinary line session instead of using escapes.

The full-screen session keeps the wordmark, transcript, input line, history,
and command results on one screen. Up/Down recall earlier lines, Page Up/Down
scroll the transcript, Ctrl-C cancels the current line or block, Ctrl-L clears
the transcript, and Ctrl-D leaves. `stop with CODE` restores the terminal
before exiting with that status. `rusk tui demo` opens the smaller demo that
shows the terminal size and the keys you press; press `q` or Ctrl-C to leave
it.

The TUI is part of the `rusk` command-line tool, not a new Rusk statement and
not something a baked program links against. The alternate screen and cursor
state are restored first.

## 14. Errors

The compiler reports problems with a line number and a sentence:

```console
$ rusk build broken.rk
burnt: in broken.rk: line 3: after 'let x,' I expected 'be', but found 'is'
```

A file that fails to bake produces no output file.

Some mistakes can only be caught while the program runs, such as asking for a
letter past the end of a string. Those print a sentence to standard error and
exit with status 1:

```console
burnt: line 3: a letter outside the text
```

Ownership mistakes are compile errors, and they point at the use that came
after the move:

```console
burnt: in app.rk: line 3: 'greeting' was moved already — 'copy of greeting'
keeps the original
```

The runtime also turns machine-level failures into Rusk sentences instead of
crashes:

| Failure | Message | Status |
|---|---|---|
| item or letter out of range | `burnt: line N: a letter outside the text` | 1 |
| division by zero | `burnt: line N: I can't divide by zero` | 1 |
| heap exhausted | `burnt: the program ran out of memory` | 1 |
| recursion deeper than 1000 calls | `burnt: the program recursed too deeply` | 1 |
| unreadable file | `burnt: I can't open that file` | 1 |
| unwritable file | `burnt: I can't write to that file` | 1 |
| file path longer than 4095 bytes | `burnt: that file path is too long` | 1 |

`stop with CODE` is not an error: it exits immediately with the requested
status and prints no message.

## 15. Compilation model

`rusk build file.rk` writes an executable next to the source with the extension
removed (`-o` overrides the destination).

The output is a static ELF64 executable:

- machine: AArch64 (`EM_AARCH64`), Linux syscall ABI;
- one `PT_LOAD` segment mapping the whole image read/write/execute;
- code at virtual address `0x400000 + 0x78`;
- writable data (variables, number buffer, text and list heap) at `0x410000`,
  zero-filled by the kernel;
- no interpreter, no dynamic section, no relocations, no libc.

Because there is no libc, `say` and `warn` are direct `write` syscalls
(`x8 = 64`) to descriptors 1 and 2, and program exit is `exit` (`x8 = 93`).
Standard input and files use `read` (`x8 = 63`); files are opened with
`openat` (`x8 = 56`) and closed with `close` (`x8 = 57`). The initial stack
pointer is saved in the data area at entry so command-line arguments and
environment variables can be walked without a C startup routine.

Function calls use the standard AArch64 shape, assembled by hand: arguments in
`X0..X7`, the result in `X0`, the return address in `X30`, and the frame pointer
in `X29`. Each frame stores the caller's `X29`/`X30` at `sp+0`/`sp+8`, then its
parameters and locals above that, so recursion gets a fresh set of slots.

A list is a pointer to `[len: u64][capacity: u64][item: i64]...`, allocated
from the same bump heap as joined text. The ownership pass guarantees that only
one live name refers to a list, so assigning a list never creates an alias;
`copy of` makes a new list and copies every item. `array_get` and `array_set`
check the 1-based index against the stored length and report `an item outside
the list` on failure. `array_add` writes into spare capacity, or allocates a
larger block and copies the items when the list is full. `array_remove_last`
and `array_remove_at` reduce the length and shift later items left; removing
from an empty list reports `there's nothing to remove`.

### Guarantees

- A program that bakes will not fail to link or load: there are no external
  symbols.
- Baking is deterministic: the same source produces byte-identical output.
- The compiler never writes a partial executable; failures leave the target
  untouched.
- A baked program never uses a moved value, never reads or writes outside a
  list, never divides by zero silently, never overruns the heap, and never
  runs off the process stack. These are checked statically where possible and
  guarded at runtime otherwise.

### Known limits of the backend

- The image is placed at a fixed base address and the data area sits at a fixed
  `0x410000`; a program with more than roughly 60 KB of code or data would
  collide. Fine for v0.7, worth relocating later.
- Integer overflow wraps silently, matching two's-complement machine behavior.
- Joined text and list literals come from a 1 MiB bump-allocated heap. Nothing
  is reclaimed; exhausting it is reported, but memory is not reused.
- Ownership is single-owner, not borrowing: there is no way to lend a value to
  two readers at once. `copy of` is the escape hatch.
- Lists are homogeneous and flat: no nested lists. They can grow with
  `add VALUE to LIST`, which uses spare capacity or copies the whole list to
  fresh heap space, and shrink with `remove last from LIST` or
  `remove item N from LIST`. Capacity never shrinks, and the heap never
  reclaims old blocks.
- System I/O covers text files and the three standard streams. There are no
  directories, sockets, processes, or random-access file positions, and a path
  is limited to 4095 bytes.
- The native TUI uses Linux `termios` and ANSI escapes. It is for the `rusk`
  tool only; baked programs have no terminal primitives yet.
- At most 8 parameters per function; no function values, closures, or
  overloading.
- Imports are textual inclusion in one global namespace; there are no module
  namespaces or selective imports.

## 16. Roadmap (not implemented)

- nested lists
- borrowing and shared read-only views
- module namespaces and selective imports
- function values and closures
- an x86-64 encoder behind the same AST and codegen
- a real relocation-aware data layout
- self-hosting: writing Rusk in Rusk
