# ruskmods — modules written in Rusk

This folder is for reusable Rusk code: the standard modules and any modules that
ship with packages. Everything here is **written in Rusk** (`.rk`), not in the
Rust compiler. If a feature can be expressed in the language, it belongs here
rather than in `src/`.

## Using a module

Import it at the top of a program:

```rusk
import "math"

say math_square with 12
```

An imported file is included once per bake, even when several modules import it.
A path without `.rk` is fine. Search starts next to the importing file, then in
`ruskmods/`, `rusklib/`, and `fastlib/` beside it and in each parent directory,
and finally beside the compiler. Both `import "math"` and
`import "semver/semver"` find the same-named file inside the module folder. A
path with a slash works the same way, so `import "installwork/cli"` finds
`ruskmods/installwork/cli.rk`, and `import "json/json"` finds
`ruskmods/json/json.rk`.

A plain import shares one global namespace, so the bundled modules keep their
prefixes. You can also give a module a name or pull names out selectively:

```rusk
import "math" as math
say math.math_square with 12

from "wheat/text" import text_is_empty
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

The layout rule is **one folder = one module = one public prefix**. Shared,
generic modules live in `ruskmods/<name>/<name>.rk`; a root-level `<name>.rk`
file is only a compatibility shim that imports the folder module. Installer
frameworks keep their own folder with internal modules.

| File | Prefix | What it offers |
|---|---|---|
| `math.rk`, `math/math.rk` | `math_` | arithmetic, gcd/lcm, clamp, sign, factorial, primes, rounding, integer square root |
| `date/date.rk` | `date_` | leap years, days in a month, validation, formatting, day of year, weekday |
| `time/time.rk` | `time_` | validation, seconds of day, clock and duration formatting, time between |
| `random/random.rk` | `random_` | deterministic LCG seeds, ranges, chance, positions |
| `statistics/statistics.rk` | `statistics_` | sum, mean, min/max, range, sort, median, mode, variance, deviation |
| `json/json.rk` | `json_` | number/bool/string encoding, arrays, objects, escaping, flat-object extraction |
| `toml/toml.rk` | `toml_` | numbers, strings, keys, fields, sections, arrays, line-based extraction |
| `sys/sys.rk` | `sys_` | environment, Linux `/proc` facts, executable and path checks, shell status, sleep |
| `os/os.rk` | `os_` | line splitting/joining, append, file size, home and temp paths, file copy |
| `path/path.rk` | `path_` | join, base, dir, extension, stem, components, normalization (text only) |
| `shutils/shutils.rk` | `shutils_` | shell quoting, silent commands, mkdir/copy/move/remove, word splitting |
| `text/text.rk` | `text_` | trim, split, join, replace, case, padding, number/text conversion, shell quoting |
| `bytes/bytes.rk` | `bytes_` | byte length, slices, find, split, reverse, hex encode/decode |
| `maps/maps.rk` | `maps_` | ordered maps as `[key, value]` pair lists, plus ordered sets |
| `result/result.rk` | `result_` | `["ok", value]` / `["error", message]` results and unwrap helpers |
| `semver/semver.rk` | `semver_` | dotted versions, comparison, requirements (`^`, `~`, ranges, wildcards) |
| `manifest/manifest.rk` | `manifest_` | `Ruskfile` and `package.json` reading, validation, rendering |
| `lock/lock.rk` | `lock_` | installed state and resolution locks as `name version` lines |
| `archive/archive.rk` | `archive_` | text-only RUSKPKG archives and POSIX `tar` command builders |
| `fs/fs.rk` | `fs_` | pure file/tree helpers that use Rusk's file primitives |
| `sha/sha.rk` | `sha_` | SHA-1/256/512 digests through POSIX `sha*sum` tools |
| `gzip/gzip.rk` | `gzip_` | gzip/tar helpers through POSIX `gzip` and `tar` |
| `installwork/*.rk` | `installwork_` | registry, requirements, resolver, state, installer, CLI, `Ruskfile` support |
| `npmwork/*.rk` | `npmwork_` | npm registry URLs, resolver, installer, lock, global prefix, project, CLI |

The shared `text_*` helpers live in `ruskmods/text/text.rk`. `wheat` imports
them explicitly, so `import "wheat/text"` still offers the old names. Most
modules are self-contained; installer frameworks import their own internal
modules and the shared helpers they need.

Useful first imports:

```rusk
import "semver"
import "json/json"
import "manifest"
import "lock"
import "npmwork"
```

The system/IO side lives in [`../rusklib/`](../rusklib/README.md): `http`,
`cache`, `fs_fast`, `platform`, `term`, `bench`, and `files`. The one-import
bundle is `import "rusklib/rusklib"`.

## Rules for adding a module

1. Write it in Rusk, not Rust.
2. Use one folder per module and put the implementation in
   `ruskmods/<name>/<name>.rk`. A root `<name>.rk` may only forward to it.
3. Prefix every public function with that single module prefix (`bytes_`,
   `semver_`, `npmwork_`, ...).
4. Put shared generic code in `ruskmods/`, system/IO code in `rusklib/`, and
   framework internals inside the framework's own folder.
5. Prefer a self-contained module; if it imports a helper, use a bare module
   name so the import keeps working wherever the module is copied.
6. Keep top-level work to definitions. A module may export constants, but it
   should not `say` anything just because it was imported.
7. Check it with `rusk check ruskmods/<name>/<name>.rk`.
8. Add a row to the table above.

Functions that belong to the language's runtime (text allocation, number
printing, syscalls, list indexing) are not modules. Those live in `rusktime/`.

## Roadmap

- a standard-library index that `rpms` can resolve packages against
- re-exports, so a module can hand a name on from another module
