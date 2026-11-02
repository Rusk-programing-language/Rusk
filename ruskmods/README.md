# ruskmods — modules written in Rusk

This folder is for reusable Rusk code: the standard modules and any modules that
ship with packages. Everything here is **written in Rusk** (`.rk`), not in the
Rust compiler. If a feature can be expressed in the language, it belongs here
rather than in `src/`.

## Using a module

Import it at the top of a program:

```rusk
import "ruskmods/math"

say math_square with 12
```

An imported file is included once per bake, even when several modules import it.
A path without `.rk` is fine. Imports resolve next to the importing file first,
then in `ruskmods/`.

In the interactive session, `import "ruskmods/text"` works at the prompt, or
load a whole program with:

```console
rusk> :load examples/imports.rk
```

Because all names share one global namespace, module functions use a prefix:
`text_`, `math_`, and so on. The prefix is the namespace until the language
grows real ones.

## Modules

| File | Prefix | What it offers |
|---|---|---|
| `text.rk` | `text_` | emptiness, first/last letter, repeat, reverse, shout, initials, contains |
| `math.rk` | `math_` | square, absolute value, min, max, sum, power, even/odd |

## Rules for adding a module

1. Write it in Rusk, not Rust.
2. Prefix every public function with the module's prefix.
3. Prefer a self-contained module; if it imports a helper, use a relative path
   so the module works wherever it is copied.
4. Keep top-level work to definitions. A module may export constants, but it
   should not `say` anything just because it was imported.
5. Check it with `rusk check ruskmods/<name>.rk`.
6. Add a row to the table above.

Functions that belong to the language's runtime (text allocation, number
printing, syscalls, list indexing) are not modules. Those live in `rusktime/`.

## Roadmap

- per-module namespaces so prefixes can disappear
- selective imports (`import text_is_empty from "ruskmods/text"`)
- a standard library index that `rpms` can resolve packages against
