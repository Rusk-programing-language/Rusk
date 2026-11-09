# The Rusk Language — Specification v1.1

Rusk is a small, English-shaped language with an ahead-of-time compiler that
emits native machine code directly. This document describes exactly what v1.1
accepts and what it guarantees.

Status: **v1.1, Linux on AArch64 and x86-64**. Everything here is implemented
and tested; anything not here does not exist yet.

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
| List | `[1, 2, 3]`, `["a", "b"]`, `[[1], [2]]` | Growable, one kind of item, positions count from 1; lists may nest |
| Function | `the function double`, `a function with n as a number: ...` | A named function or closure; called with `call ... with ...` |

Numbers are integers only. There is no floating point in v1.1.

Text and lists live on a heap and are **owned**: a name holds one of them at a
time. Ownership is what makes the language memory-safe without a garbage
collector; see §4.1. A **view** (`a view of VALUE`, or a parameter written
`as a view of ...`) lends a value read-only without moving ownership.

Functions are ordinary values. A function value can be stored, copied with
`copy of`, printed (`<function>`), put in a list, passed as an argument, and
given back. Functions cannot be compared. Kinds can describe function
signatures, so parameters and list items can have function kinds; see §10.

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
  an argument to an ordinary parameter, writing an item into a list, and
  `give back` all **move** the value. The old name is empty afterwards.
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

#### Views

A **view** is a shared, read-only alias. Writing `a view of VALUE` binds or
passes the same value without moving it:

```rusk
let loaves be [3, 6, 9]
let shared be a view of loaves
say item 2 of shared          # 6
say length of loaves          # 3; loaves is still the owner

to total with xs as a view of a list of numbers:
    let sum be 0
    for each x in xs:
        set sum to sum + x
    give back sum

say total with loaves         # 18; the call did not move loaves
say loaves                    # [3, 6, 9]
```

A view may be read, indexed, measured, iterated, and passed on to another view
parameter. It may **not** be moved, changed, grown, shortened, or used to write
an item, because it does not own its value:

```console
burnt: line 3: 'shared' is a view — it shares its value read-only and can't be changed
burnt: line 3: 'shared' is a view — it's shared read-only; use 'copy of shared' to make an owned value
```

Use `copy of` when a function needs to change or keep the value. A view of a
number or truth value is just the number or truth value, since those are copied
freely and cannot be moved.

Rusk has no null, no pointers, and no threads. Apart from read-only views, a
value cannot be observed through two names, so there is no dangling reference,
no double free, and no shared mutable state to race over. The checker runs
before any machine code is produced; it is not a runtime cost.

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

Every item must have the same kind: a list holds numbers, text, truth values,
or other lists. A list of lists may nest to any depth, as long as every sibling
has the same shape. An empty list cannot guess its kind, so write it down:

```rusk
let empty_numbers be an empty list of numbers
let empty_names be an empty list of text
let empty_flags be an empty list of truth values
let grid be an empty list of lists of numbers
let cube be an empty list of lists of lists of truth values
```

`an` is optional: `empty list of text` reads the same. The kind is also written
in function parameters as `a list of numbers`, `a list of lists of numbers`, and
so on. The innermost kind is one of `numbers`, `text`, or `truth values`; each
`lists of` adds one level of nesting.

```rusk
let rows be [[1, 2], [3, 4], an empty list of numbers]
say item 2 of item 1 of rows        # 2
set item 3 of rows to [5]
add [6, 7] to rows
say rows                            # [[1, 2], [3, 4], [5], [6, 7]]
```

`say` prints nested lists with brackets, commas, and spaces. `copy of` is a
deep copy: every nested list and every text item is duplicated, so changing one
copy never changes the other.

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
- Writing `P as a view of KIND` makes a **borrowed** parameter: the caller's
  value is lent read-only instead of being moved. See §4.1.
- A function body is an indented block, just like an `if`.
- `give back VALUE` ends the function and hands `VALUE` to the caller.

Kinds are written `a number`, `text`, `a truth value` (`a boolean` works too),
and list kinds such as `a list of numbers`, `a list of text`,
`a list of truth values`, or `a list of lists of numbers`. The `a`/`an` is
optional: `n as number` reads the same.

Function kinds are written `a function taking KIND, KIND giving back KIND`.
A function with no parameters is `a function giving back KIND`. Use the plural
`functions` for a list kind: `a list of functions taking a number giving back a
number`, and `an empty list of functions ...` for an empty one.

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

Functions live at the top level; defining one with `to` inside a block is an
error. They may be called before their definition appears in the file.

Top-level names are visible inside functions. A function may read a global and
change it with `set` or `add`, but it may not move it, because the top level
still owns it. A parameter or local may shadow a global:

```rusk
let total be 0

to bump with n as a number:
    set total to total + n

bump with 2
bump with 3
say total                     # 5
```

### Function values

A named function can be used as a value with `the function NAME`:

```rusk
to double with n as a number:
    give back n * 2

let twice be the function double
say call twice with 21        # 42
say twice with 4              # the ordinary call spelling also works
```

- `the function NAME` reads a named function as a function value.
- `call VALUE with ARG and ARG` calls a function value; `call VALUE` calls one
  that takes no arguments.
- The ordinary `NAME with ...` spelling also calls a function value held in a
  variable when there is no named function of that name.
- A function value may be stored with `let` or `constant`, read, copied with
  `copy of`, printed with `say` or `warn` (it prints as `<function>`), put in a
  list, passed as an argument, and given back. It cannot be compared.
- A dynamic call checks both the number of arguments and their kinds.

A function kind names a function value in a parameter or a list item:

```rusk
to double with n as a number:
    give back n * 2

to apply with f as a function taking a number giving back a number and n as a number:
    give back call f with n

say apply with the function double and 21     # 42

let steps be [the function double, the function double]
say call (item 2 of steps) with 3             # 6
```

Because a list item is an expression, parenthesize a call on it:
`call (item 1 of fs) with 3`. A function with no parameters is written
`a function giving back a number`; two parameters are separated by a comma:
`a function taking a number, text giving back a truth value`.

### Closures

A function value written with `a function with ...:` is a **closure**. It
captures the names its body reads from the surrounding scope:

```rusk
let base be 10
let add_base be a function with n as a number:
    give back n + base

say call add_base with 5      # 15
```

- A closure is written `a function with ...:` (the article is optional, so
  `function with ...:` works too) and may take the same parameter list and
  borrowed views as a named function.
- A function literal is a value expression. It can be stored, added to a list,
  passed, given back, and called like any other function value.
- Free names are captured when the closure is made. Numbers and truth values
  are copied into it. Text and lists are captured as shared read-only views:
  the closure may read them but may not move or change them.
- A closure may capture another function value and may create another closure
  inside its body. A function may give back a function:

```rusk
to make_adder with n as a number:
    give back a function with x as a number:
        give back x + n

let add5 be make_adder with 5
say call add5 with 10         # 15
```

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

An absolute path is used as written. A file and a namespace are expanded **at
most once** per bake, so several modules can import the same helper without
defining it twice. Imports are recursive: an imported file may import others. A
cycle is an error and names the chain:

```console
burnt: in lib/b.rk: line 1: this import makes a cycle: main.rk -> lib/a.rk -> lib/b.rk -> lib/a.rk
```

`import` and `from` only work at the top level, not inside a block. A missing
file, an unreadable file, or a syntax error inside an imported file names the
file it came from.

### Exports and file boundaries

`export` includes another file's content or marks names as part of this file's
public surface:

```rusk
export "lib/greetings"                     # include everything, globally
export "lib/greetings" as greet            # include it under a namespace
export double, greet from "lib/greetings"  # include only those names
export double, greet                       # declare exported names
```

`export "file"` behaves like a plain `import "file"`: the file's statements are
copied in at that point and share the one global namespace. `export "file" as
NAME` and `export NAME from "file"` match the named and selective import forms.
The no-file form checks that every name is defined at the top level (including
by an import); it documents which names the file offers to the files that
export it.

A line containing only `end`, `end.`, `<e>`, `</e>`, or `end of file` (with an
optional `#` comment) ends the file. Everything after that line is ignored.
Because `export` reads the other file through the same lexer, the boundary also
stops an exported file, so files can be concatenated or chained without one
swallowing the other:

```rusk
# math.rk
to double with n as a number:
    give back n * 2
end

# main.rk
export "math.rk"
say double with 21        # 42
```

### Module namespaces

A plain `import` still joins one global namespace. Two forms keep modules apart:

```rusk
import "math" as math           # math.double, math.answer, ...
from "math" import double       # bring double in without a prefix
from "math" import double as twice
```

With `import ... as NAME`, every top-level definition in the file is given the
`NAME.` prefix and must be referred to that way. With `from`, the compiler
imports the file under a namespace made from its file stem (`math` here), then
rewrites the chosen names to their qualified form. `as` gives a local alias;
the original name is not introduced. Several names may follow one `from`:

```rusk
from "math" import double, add as plus
say double with 21
say plus with 2 and 3
```

A namespaced module's own plain imports are **private**: they inherit the same
prefix, so a helper cannot leak into the program that imported the module. If
two `from` imports resolve to files with the same stem, the second namespace
gets `_2`, `_3`, and so on. Two selective imports cannot introduce the same
local name — give one an `as` alias — and two `as` aliases must be distinct.

```rusk
import "lib/thing" as thing
say thing.use_hidden with 4     # hidden lives inside thing, not here
```

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

The session is a tree-walking evaluator over the same AST, not a machine-code
backend. Every submission is still lexed, parsed, type checked, and ownership
checked against the whole session before any of it runs, so a session accepts
the same programs the compiler does. Function values and closures work at the
prompt too: `the function NAME` produces a value, `call ... with ...` runs it,
and a closure remembers the session names that were visible when it was made.
Views evaluate to the value they alias and keep the same read-only rules. Text
operations use byte positions here too, and a moved name is empty here just as
it is in a baked program. `import` works at the prompt (relative to the current
directory); inside `:load`, relative imports resolve from the loaded file's
directory.

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

`rusk build file.rk [-o OUT] [--target aarch64|x86_64]` writes an executable
next to the source with the extension removed (`-o` overrides the destination).
`rusk check` accepts the same target flag. With no `--target`, Rusk bakes for
the architecture running the compiler, so `rusk run` can execute the result.
`rusk run` also accepts `--target`, but then the host must be able to execute
that architecture, for example through `binfmt_misc` or QEMU.

Release builds are published both as glibc-linked and statically linked musl
binaries for AArch64 and x86-64, so the compiler also runs on Termux, Alpine,
and other systems without a glibc loader.

The output is a static ELF64 executable for the selected target:

- machine: `EM_AARCH64` (183) or `EM_X86_64` (62), Linux syscall ABI;
- one `PT_LOAD` segment mapping the whole image read/write/execute;
- code at virtual address `0x400000 + 0x78`;
- writable data (top-level variables, the number buffer, file-path scratch
  space, the text and list heap, and the x86-64 virtual-register homes) placed
  on the next 4 KiB boundary after the text and read-only data, zero-filled by
  the kernel, with its address patched into the code after layout;
- no interpreter, no dynamic section, no load-time relocations, no libc.

Because there is no libc, output and file I/O are direct Linux syscalls.
AArch64 uses `x8` for the number: `write` is `64`, `exit` is `93`, `read` is
`63`, `openat` is `56`, and `close` is `57`. x86-64 uses `rax`: `write` is `1`,
`exit` is `60`, `read` is `0`, `openat` is `257`, and `close` is `3`. The
initial stack pointer is saved in the data area at entry so command-line
arguments and environment variables can be walked without a C startup routine.

The code generator is written against one small virtual-register language:
arguments in `X0..X7`, the result in `X0`, the frame pointer in `X29`, and a
zero register `XZR`. On AArch64 those are the real registers, the return
address rides in `X30`, and each frame stores the caller's `X29`/`X30` at
`sp+0`/`sp+8`, then its parameters and locals above that. On x86-64 the encoder
maps virtual registers onto a pool of six caller-saved physical registers
inside each straight-line block, writes them back to home slots in the data
area at labels, branches, calls, returns, and syscalls, and reloads them where
needed. It uses the real `rsp` for `SP` and the processor's `call`/`ret` stack
for return addresses. Syscall arguments are loaded into `rdi`, `rsi`, `rdx`,
`r10`, `r8`, and `r9`; AArch64 syscall numbers written by the code generator
are translated to their x86-64 equivalents. Both backends therefore run the
same generated program logic and get the same runtime behavior.

A list is a pointer to `[len: u64][capacity: u64][item: i64]...`, allocated
from the same bump heap as joined text. The ownership pass guarantees that only
one mutable name refers to a list (apart from read-only views), so changing a
list never surprises another owner; `copy of` makes a new list and copies every
item. For nested lists the code generator emits one recursive copy routine and
one recursive printer per list shape the program actually uses. `array_get` and
`array_set` check the 1-based index against the stored length and report `an
item outside the list` on failure. `array_add` writes into spare capacity, or
allocates a larger block and copies the items when the list is full.
`array_remove_last` and `array_remove_at` reduce the length and shift later
items left; removing from an empty list reports `there's nothing to remove`.

A function value is a pointer to `[code: u64][capture count: u64][capture:
i64]...`. A dynamic call checks the argument count and kinds, reads the first
word into the register holding the callee address, and passes `record + 16` as
an environment pointer. Named
functions and closures share one frame layout: slot 0 is the environment
pointer, parameters follow, then captured values, then ordinary locals. A
closure's prologue copies its captures out of the environment into frame slots,
so the body reads them like any other name. Named functions are called with a
zero environment pointer and the same layout. A view compiles to the underlying
value; the ownership pass is what prevents it from being moved or changed.

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

- The image is still placed at a fixed base address and is not
  position-independent, but the data area follows the final text and read-only
  size, so growing a program moves its data instead of colliding with it.
- Integer overflow wraps silently, matching two's-complement machine behavior.
- Joined text and list literals come from a 1 MiB bump-allocated heap. Nothing
  is reclaimed; exhausting it is reported, but memory is not reused.
- Ownership is single-owner. `a view of VALUE` and `as a view of ...` lend a
  value read-only, but views cannot be moved, changed, or stored, and they do
  not outlive the owner's frame. `copy of` is the escape hatch.
- Function values cannot be compared. Dynamic calls check both the number of
  arguments and their kinds. Closures capture numbers and truth values by
  value, and text and lists as shared read-only views.
- Lists are homogeneous but may nest to any depth. They can grow with
  `add VALUE to LIST`, which uses spare capacity or copies the whole list to
  fresh heap space, and shrink with `remove last from LIST` or
  `remove item N from LIST`. Capacity never shrinks, and the heap never
  reclaims old blocks.
- System I/O covers text files and the three standard streams. There are no
  directories, sockets, processes, or random-access file positions, and a path
  is limited to 4095 bytes.
- The native TUI uses Linux `termios` and ANSI escapes. It is for the `rusk`
  tool only; baked programs have no terminal primitives yet.
- Only Linux is supported. The two backends are AArch64 and x86-64; there is no
  macOS, Windows, or 32-bit output. The x86-64 backend allocates registers
  within straight-line blocks and spills at branches and calls, so it is close
  to the AArch64 output but is not a global register allocator yet.
- At most 8 parameters per function. There is no function overloading.

## 16. Roadmap

v1.1 implements everything the v1.0 roadmap listed: function-typed parameters,
function values in lists, functions that give back functions, a
register-allocating x86-64 encoder, a relocation-aware data layout, and a
self-hosting interpreter. What remains:

- full self-hosting: writing the Rusk compiler itself in Rusk. The current
  bootstrap stage is `selfhost/interpreter.rk`, an integer-expression
  interpreter written in Rusk and compiled by Rusk.
- a global x86-64 register allocator that keeps values in registers across
  branches and calls.
- reclaiming heap memory instead of the current 1 MiB bump allocator.
