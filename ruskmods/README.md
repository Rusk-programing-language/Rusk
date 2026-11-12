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
| `math/math.rk` | `math_` | gcd, lcm, clamp, sign, factorial, primes, digit sum, rounding, integer square root |
| `date/date.rk` | `date_` | leap years, days in a month, validation, ISO and day-first formatting, day of year, weekday |
| `time/time.rk` | `time_` | validation, seconds of day, clock and duration formatting, time between |
| `random/random.rk` | `random_` | deterministic LCG seeds, ranges, chance, positions |
| `statistics/statistics.rk` | `statistics_` | sum, mean, min/max, range, sort, median, mode, variance, deviation, counts |
| `json/json.rk` | `json_` | number/bool/string encoding, arrays, objects, escaping, flat-object extraction, structural validity |
| `toml/toml.rk` | `toml_` | numbers, strings, keys, fields, sections, arrays, line-based extraction |
| `sys/sys.rk` | `sys_` | environment, Linux `/proc` facts, executable and path checks, shell status, sleep |
| `os/os.rk` | `os_` | line splitting/joining, append, first/last line, file size, home and temp paths, file copy |
| `path/path.rk` | `path_` | join, base, dir, extension, stem, components, normalization (text only) |
| `shutils/shutils.rk` | `shutils_` | shell quoting, silent commands, mkdir/copy/move/remove, word splitting |
| `../rusklib/files.rk` | `files_` | file read/write/append, file and tree copy, move, delete, size, tar.gz create/extract |
| `../rusklib/rusklib.rk` | — | one import for every module above plus `rusklib/files.rk` |

The subfolders are ordinary modules, not packages: `import "date/date"` finds
`ruskmods/date/date.rk`, and `import "rusklib/rusklib"` finds the bundle at the
repository root. Modules never import each other, so each one can be copied or
checked on its own.

## Rules for adding a module

1. Write it in Rusk, not Rust.
2. Prefix every public function with the module's prefix.
3. Prefer a self-contained module; if it imports a helper, use a relative path
   so the module works wherever it is copied.
4. Keep top-level work to definitions. A module may export constants, but it
   should not `say` anything just because it was imported.
5. Check it with `rusk check ruskmods/<name>.rk` (or the nested path, such as
   `rusk check ruskmods/date/date.rk`).
6. Add a row to the table above.
7. Keep file and archive work that shells out in `rusklib/files.rk`, so the
   `ruskmods/` modules stay pure text-and-number code.

Functions that belong to the language's runtime (text allocation, number
printing, syscalls, list indexing) are not modules. Those live in `rusktime/`.

## Roadmap

- package manifests and a standard-library index that `rpms` can resolve
  packages against
- re-exports, so a module can hand a name on from another module
