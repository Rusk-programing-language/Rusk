# The Rusk Language — Specification v0.1

Rusk is a small, English-shaped language with an ahead-of-time compiler that
emits native machine code directly. This document describes exactly what v0.1
accepts and what it guarantees.

Status: **v0.1, AArch64 Linux**. Everything here is implemented and tested;
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

Numbers are integers only. There is no floating point in v0.1.

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
| Comparison | `is more than`, `is less than`, `is equal to`, `is not`, `is at least`, `is at most`, `>`, `<`, `==`, `!=`, `>=`, `<=` | one comparison per expression |
| Sum | `+`, `-` | left |
| Product | `*`, `/` | left |
| Unary | `-`, `not` | right |
| Primary | number, text, `true`, `false`, name, `( ... )` | — |

Examples:

```rusk
say 1 + 2 * 3            # 7
say (1 + 2) * 3          # 9
say 100 / 3              # 33   (integer division truncates toward zero)
say 100 / -7             # -14
say 5 is more than 3     # true
say 5 is at most 5       # true
say not (1 is equal to 2)  # true
```

Rules:

- `+ - * /` require numbers on both sides.
- Comparisons require both sides to be the same kind, and that kind must be a
  number or a truth value. Comparing text is not supported yet.
- `not` requires a truth value.
- Unary `-` requires a number.

## 7. Blocks

`if` and `repeat` introduce a block: a colon, then an indented body. The body
ends when the indentation returns to the enclosing level.

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

`otherwise` is optional. There is no `else if` in v0.1; nest an `if` inside an
`otherwise` block instead.

The condition must be a truth value. A bare number is not accepted as a
condition — write `is not 0` if that is what you mean.

## 9. Repetition

```rusk
repeat 3 times:
    say "one loaf"
```

The count is evaluated once, before the first iteration. A count of zero or less
runs the body zero times.

## 10. Whole programs

A program is executed top to bottom. When the last statement finishes, the
program exits with status 0. There is no `main` and no entry-point ceremony.

```rusk
# hello.rk
say "Hello, world."
```

## 11. Errors

The compiler reports problems with a line number and a sentence:

```console
$ rusk build broken.rk
burnt: in broken.rk: line 3: after 'let x,' I expected 'be', but found 'is'
```

A file that fails to bake produces no output file.

## 12. Compilation model

`rusk build file.rk` writes an executable next to the source with the extension
removed (`-o` overrides the destination).

The output is a static ELF64 executable:

- machine: AArch64 (`EM_AARCH64`), Linux syscall ABI;
- one `PT_LOAD` segment mapping the whole image read/write/execute;
- code at virtual address `0x400000 + 0x78`;
- writable data (variables, number buffer) at `0x410000`, zero-filled by the
  kernel;
- no interpreter, no dynamic section, no relocations, no libc.

Because there is no libc, `say` is a direct `write` syscall (`x8 = 64`) and
program exit is `exit` (`x8 = 93`).

### Guarantees

- A program that bakes will not fail to link or load: there are no external
  symbols.
- Baking is deterministic: the same source produces byte-identical output.
- The compiler never writes a partial executable; failures leave the target
  untouched.

### Known limits of the backend

- The image is placed at a fixed base address and the data area sits at a fixed
  `0x410000`; a program with more than roughly 60 KB of code or data would
  collide. Fine for v0.1, worth relocating later.
- Division by zero traps (SIGFPE analogue) rather than producing a Rusk-level
  error.
- Integer overflow wraps silently, matching two's-complement machine behavior.

## 13. Roadmap (not implemented)

- `to NAME with ARG ...:` function definitions and `give back`
- text comparison and concatenation
- `while`, `for each`, `otherwise if`
- an x86-64 encoder behind the same AST and codegen
- a real relocation-aware data layout
- self-hosting: writing Rusk in Rusk
