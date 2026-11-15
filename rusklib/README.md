# rusklib — higher-level Rusk helpers

`rusklib` sits on top of [`ruskmods/`](../ruskmods/README.md). It holds the
helpers that need to copy whole trees or call standard tools, the inbuilt
terminal UI library, and one bundle that imports every module.

```rusk
import "rusklib/rusklib"

say date_format_iso with 2026 and 11 and 12
say files_exists with "notes.txt"
say wheat_box_around with "hello" and 0 and "rounded"
```

Import one file directly when the bundle is more than a program needs:

```rusk
import "rusklib/files"
import "wheat/table"

files_copy with "src" and "backup/src"
say files_size with "backup/src/notes.txt"
say wheat_table with ["name"] and [["math"]] and "simple"
```

Bare module names resolve next to the importing file, then in `ruskmods/` and
`rusklib/` beside it and in each parent directory. Every public name keeps its
prefix (`files_`, `wheat_`), because a plain import shares one global namespace.

## `files.rk`

| Function | Gives back | What it does |
|---|---|---|
| `files_exists with path` | truth value | whether the path can be reached |
| `files_is_directory with path` | truth value | `test -d` through the shell |
| `files_is_file with path` | truth value | `test -f` through the shell |
| `files_make_dirs with path` | exit status | `mkdir -p` |
| `files_read with path` | text | whole file; stops the program when unreadable |
| `files_write with path and text` | — | replaces the file's contents |
| `files_append with path and text` | — | adds text at the end, creating the file |
| `files_copy_file with from and to` | — | read/write copy of one file |
| `files_copy with from and to` | exit status | file copy, or `cp -R` for a directory |
| `files_move with from and to` | exit status | `mv` |
| `files_delete with path` | exit status | `rm -rf`; missing paths are ignored |
| `files_list with path` | list of text | entry names from `list directory` |
| `files_size with path` | number | bytes read from the file; missing counts as 0 |
| `files_is_empty with path` | truth value | missing or zero bytes |
| `files_archive_create with source and archive` | exit status | `tar -czf`, storing the source's own name |
| `files_archive_extract with archive and destination` | exit status | `tar -xzf`, after `mkdir -p` |

The copy, move, delete, and archive helpers quote every path for `/bin/sh` and
return the command's exit status. They need the usual POSIX tools (`cp`, `mv`,
`rm`, `tar`, `mkdir`). `files_copy_file`, `files_write`, and `files_append` use
the language's own file primitives instead.

## `wheat/` — terminal UI

[`wheat/`](wheat/README.md) is a Rich-inspired terminal library written entirely
in Rusk. It renders text; it does not own a screen or run an event loop.

| Module | What it holds |
|---|---|
| `wheat/text` | trimming, split, join, replace, case, padding, cropping, wrapping, markup; inherited `text_*` helpers |
| `wheat/color` | colour names, hex, the 256 palette, ANSI codes, colour-system detection |
| `wheat/style` | attribute and colour specs such as `bold red` |
| `wheat/segment` | `[text, style]` segments, lines, and blocks |
| `wheat/measure` | widths, heights, column sizing |
| `wheat/box` | frames: light, heavy, double, rounded, ascii, none |
| `wheat/bar` | progress bars, gauges, sparklines, spinners |
| `wheat/columns` | side-by-side blocks, grids, packed chips |
| `wheat/panel` | boxed sections with title, padding, and subtitle |
| `wheat/table` | aligned rows with optional frames |
| `wheat/layout` | vertical stacks, splits, pages |
| `wheat/theme` | named styles and theme markup |
| `wheat/console` | terminal size, escape sequences, printing, rules |
| `wheat/keys` | key names and escape sequences from `read key` |
| `wheat/app` | menus, checklists, dialogs, frames, status bars |
| `wheat/wheat` | the bundle: imports every module above |

```rusk
import "wheat/wheat"

say wheat_panel_full with "Installed" and "2 items" and "math 1.2.0" and 0 and "left" and 1 and "rounded"
say wheat_bar_labelled with "build" and 5 and 20 and 40
```

Colour follows `WHEAT_COLOR`, `NO_COLOR`, `COLORTERM`, and `TERM`; the default
is `standard` rather than "detect a tty", and `WHEAT_COLOR=none` turns escape
codes off. Widths are in display cells, measured correctly for ASCII and for
wheat's own drawing glyphs; other non-ASCII text is measured by UTF-8 byte
length. See [`wheat/README.md`](wheat/README.md) for every module, the colour
modes, and the limits.

## `rusklib.rk`

`import "rusklib/rusklib"` pulls in:

- `math`, `math/math`
- `date/date`, `time/time`, `random/random`, `statistics/statistics`
- `json/json`, `toml/toml`
- `sys/sys`, `os/os`, `path/path`, `shutils/shutils`
- `installwork/installwork`
- `files`
- `wheat/wheat`

The bundle mostly uses explicit relative imports so it works from a checkout and
from an installed copy that keeps the same layout. The `wheat` modules import
each other by bare name.

## Installer framework

`import "installwork"` (source in
[`ruskmods/installwork/`](../ruskmods/installwork/)) is a separate, larger
framework: a package registry, version specs, a dependency resolver, archive
handling, an install prefix, recorded state, an installer, and a CLI. It also
reads `Ruskfile` manifests. It is not part of `files.rk` or `wheat`; the bundle
imports it for convenience, but a program that only wants widgets need not.

## Limits

- `json` and `toml` are small readers and writers, not complete parsers. The
  JSON reader handles one flat object at a time; the TOML reader matches one
  `key = value` line at a time.
- `sys` reads Linux `/proc` files and returns empty text or `0` when a file is
  not readable. `run command` shares output instead of capturing it, so helpers
  that need output read a file.
- File copies keep the file contents in memory as text. They are for ordinary
  files, not multi-gigabyte data.
- `wheat` does not query the terminal, does not detect a tty, and does not
  implement an event loop. It builds strings; the caller writes them.
