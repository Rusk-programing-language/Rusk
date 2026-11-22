# rusklib — higher-level Rusk helpers

`rusklib` sits on top of [`ruskmods/`](../ruskmods/README.md). It holds the
system/IO helpers that shell out or touch the environment, the higher-level file
helpers, the inbuilt terminal UI library, and one bundle that imports every
module. The layout rule is the same as in `ruskmods`: one folder, one module, one
public prefix, with the implementation in `<name>/<name>.rk`.

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
`rusklib/` beside it and in each parent directory, and finally beside the
compiler. A path such as `rusklib/http` also finds `rusklib/http/http.rk`. Every
public name keeps its prefix (`files_`, `http_`, `wheat_`), because a plain
import shares one global namespace.

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
the language's own file primitives instead. The canonical source is
`rusklib/files/files.rk`; `rusklib/files.rk` is only a forwarding shim, so both
`import "rusklib/files"` and `import "rusklib/files/files"` work.

## System and IO modules

| Module | Prefix | What it offers |
|---|---|---|
| `http/http.rk` | `http_` | `curl`-backed GET, retries, status, headers, ETag, cache key |
| `cache/cache.rk` | `cache_` | metadata and tarball cache with atomic writes |
| `fs_fast/fs_fast.rk` | `fs_fast_` | hardlink/copy trees, atomic rename, temp files |
| `platform/platform.rk` | `platform_` | OS, arch, libc, home, temp, shell, environment |
| `term/term.rk` | `term_` | colour, progress bars, gauges, spinners, rules |
| `bench/bench.rk` | `bench_` | nanosecond timers and phase reports via POSIX `date` |

Rusk has no native TLS, DNS, clock, or hashing primitive. `http` uses `curl`
for DNS and TLS; `bench` uses `date +%s%N`; the generic `sha` and `gzip`
modules in `ruskmods/` use the matching POSIX commands. Raw Rusk sockets are
IPv4-only with no DNS, TLS, or timeouts, so the HTTP module deliberately does
not use them. These helpers are honest about the tools they need.

```rusk
import "rusklib/http"
import "rusklib/cache"
import "rusklib/term"

say http_cache_key with "https://example.com"
say cache_root
say term_progress with 3 and 10 and 20
```

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

- `math`, `date`, `time`, `random`, `statistics`
- `json`, `toml`, `sys`, `os`, `path`, `shutils`
- `bytes`, `text`, `semver`, `maps`, `result`
- `manifest`, `lock`, `archive`, `fs`, `sha`, `gzip`
- `installwork` and `npmwork`
- the system modules `http`, `cache`, `fs_fast`, `platform`, `term`, `bench`
- `files` and `wheat/wheat`

The bundle mostly uses explicit relative imports so it works from a checkout and
from an installed copy that keeps the same layout. The `wheat` and installer
modules import their helpers by bare name.

## Installer frameworks

`import "installwork"` (source in
[`ruskmods/installwork/`](../ruskmods/installwork/)) is a separate, larger
framework: a package registry, version specs, a dependency resolver, archive
handling, an install prefix, recorded state, an installer, and a CLI. It also
reads `Ruskfile` manifests.

`import "npmwork"` (source in
[`ruskmods/npmwork/`](../ruskmods/npmwork/)) is the JS/TS installer framework:
`package.json` manifests, npm registry URLs through `rusklib/http`, dependency
resolution, tarball installation into `node_modules`, a lock file, global
prefixes, project commands, and a CLI. It does not run npm lifecycle scripts and
does not parse the full npm metadata format.

Neither framework is part of `files.rk` or `wheat`; the bundle imports them for
convenience, but a program that only wants widgets need not.

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
