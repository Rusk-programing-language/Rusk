# The Rusk Language — Specification v0.4

Rusk is a small, English-shaped language with an ahead-of-time compiler that
emits native machine code directly. This document describes exactly what v0.4
accepts and what it guarantees.

Status: **v0.4, AArch64 Linux**. Everything here is implemented and tested;
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

Numbers are integers only. There is no floating point in v0.4.

## 4. Naming things

```rusk
let slices be 8
set slices to slices + 1
```

- `let NAME be VALUE` declares a new thing. Declaring a name twice is an error;
  use `set` to change it.
- `set NAME to VALUE` assigns to an existing thing. The new value must be the
  same kind as the old one; Rusk will not change a thing's kind.

Names may contain letters, digits, and underscores, and may not start with a
digit. Keywords are not reserved — `say`, `let`, `if` and the rest are matched
by position, so a variable literally named `say` is legal (if confusing).

## 5. Saying things

```rusk
say "Hello, world."
say 42
say true
```

`say` writes its value to standard output followed by a newline. Text and truth
values print as themselves (`true` / `false`); numbers print in decimal, with a
minus sign when negative.

## 6. Expressions

Operator precedence, loosest first:

| Level | Operators | Associativity |
|---|---|---|
| Comparison | `is`, `is more than`, `is less than`, `is equal to`, `is not`, `is at least`, `is at most`, `>`, `<`, `==`, `!=`, `>=`, `<=` | one comparison per expression |
| Sum | `+`, `-` | left |
| Product | `*`, `/` | left |
| Unary | `-`, `not` | right |
| Primary | number, text, `true`, `false`, name, call, text in pieces, `( ... )` | — |

`+` adds two numbers or joins two pieces of text. `-`, `*`, and `/` are numbers
only. A bare `is` means `is equal to`: `if name is "Rusk":`.

Examples:

```rusk
say 1 + 2 * 3            # 7
say (1 + 2) * 3          # 9
say 100 / 3              # 33   (integer division truncates toward zero)
say 100 / -7             # -14
say 5 is more than 3     # true
say 5 is at most 5       # true
say not (1 is equal to 2)  # true
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
- Unary `-` requires a number.

### Joining text

Text is immutable. `left + right` allocates a new string in a fixed 1 MiB heap
inside the program's data area, copies both byte blobs into it, and hands back
the new value. Nothing is freed: this language is small and programs are short,
but a long-running loop that keeps joining text will eventually run out of heap
(and there is no runtime error for that yet).

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

Kinds are written `a number`, `text`, and `a truth value` (`a boolean` works
too). The `a`/`an` is optional: `n as number` reads the same.

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

## 11. Whole programs

A program is executed top to bottom. When the last statement finishes, the
program exits with status 0. There is no `main` and no entry-point ceremony.

```rusk
# hello.rk
say "Hello, world."
```

## 12. The interactive session

`rusk` with no arguments, or `rusk repl`, starts an interactive session:

```console
$ rusk
Rusk 0.4.0 — an English-like language
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
backend. Every submission is still lexed, parsed, and type checked against the
whole session before any of it runs, so a session accepts the same programs the
compiler does. Text operations use byte positions here too, matching the baked
behavior.

When stdin is not a terminal, the banner and prompts are omitted, so
`rusk < program.rk` works as a pipe. The session is not a `.rk` file compiler:
it does not write an executable.

## 13. Errors

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

## 14. Compilation model

`rusk build file.rk` writes an executable next to the source with the extension
removed (`-o` overrides the destination).

The output is a static ELF64 executable:

- machine: AArch64 (`EM_AARCH64`), Linux syscall ABI;
- one `PT_LOAD` segment mapping the whole image read/write/execute;
- code at virtual address `0x400000 + 0x78`;
- writable data (variables, number buffer, text heap) at `0x410000`,
  zero-filled by the kernel;
- no interpreter, no dynamic section, no relocations, no libc.

Because there is no libc, `say` is a direct `write` syscall (`x8 = 64`) and
program exit is `exit` (`x8 = 93`).

Function calls use the standard AArch64 shape, assembled by hand: arguments in
`X0..X7`, the result in `X0`, the return address in `X30`, and the frame pointer
in `X29`. Each frame stores the caller's `X29`/`X30` at `sp+0`/`sp+8`, then its
parameters and locals above that, so recursion gets a fresh set of slots.

### Guarantees

- A program that bakes will not fail to link or load: there are no external
  symbols.
- Baking is deterministic: the same source produces byte-identical output.
- The compiler never writes a partial executable; failures leave the target
  untouched.

### Known limits of the backend

- The image is placed at a fixed base address and the data area sits at a fixed
  `0x410000`; a program with more than roughly 60 KB of code or data would
  collide. Fine for v0.4, worth relocating later.
- Division by zero traps (SIGFPE analogue) rather than producing a Rusk-level
  error.
- Integer overflow wraps silently, matching two's-complement machine behavior.
- Joined text comes from a 1 MiB bump-allocated heap. Nothing is reclaimed, and
  running past the end of the heap is not detected.
- Recursion depth is bounded only by the process stack; runaway recursion dies
  rather than reporting an error.
- At most 8 parameters per function; no function values, closures, or
  overloading.

## 15. Roadmap (not implemented)

- function values and closures
- an x86-64 encoder behind the same AST and codegen
- a real relocation-aware data layout
- self-hosting: writing Rusk in Rusk
