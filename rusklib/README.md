# rusklib — higher-level Rusk helpers

`rusklib` sits on top of [`ruskmods/`](../ruskmods/README.md). It holds the
helpers that need to copy whole trees or call standard tools, plus one bundle
that imports every module.

```rusk
import "rusklib/rusklib"

say date_format_iso with 2026 and 11 and 12
say files_exists with "notes.txt"
```

Import one file directly when the bundle is more than a program needs:

```rusk
import "rusklib/files"

files_copy with "src" and "backup/src"
say files_size with "backup/src/notes.txt"
```

Every public name keeps its prefix, because a plain import shares one global
namespace.

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

## `rusklib.rk`

`import "rusklib/rusklib"` pulls in:

- `text`, `math`, `math/math`
- `date/date`, `time/time`, `random/random`, `statistics/statistics`
- `json/json`, `toml/toml`
- `sys/sys`, `os/os`, `path/path`, `shutils/shutils`
- `files`

The bundle uses relative imports, so it works from a checkout and from an
installed copy that keeps the same layout.

## Limits

- `json` and `toml` are small readers and writers, not complete parsers. The
  JSON reader handles one flat object at a time; the TOML reader matches one
  `key = value` line at a time.
- `sys` reads Linux `/proc` files and returns empty text or `0` when a file is
  not readable. `run command` shares output instead of capturing it, so helpers
  that need output read a file.
- File copies keep the file contents in memory as text. They are for ordinary
  files, not multi-gigabyte data.
