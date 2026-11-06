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

A plain import shares one global namespace, so the bundled modules keep their
prefixes. You can also give a module a name or pull names out selectively:

```rusk
import "ruskmods/math" as math
say math.math_square with 12

from "ruskmods/text" import text_is_empty
say text_is_empty with ""
```

The bundled modules keep their `text_` / `math_` names for compatibility, but a
module you write yourself can use a plain name when it is imported `as`:

```rusk
# lib/units.rk
to convert with n as a number:
    give back n * 100

# main.rk
import "lib/units" as units
say units.convert with 3
```

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

- package manifests and a standard-library index that `rpms` can resolve
  packages against
- re-exports, so a module can hand a name on from another module
