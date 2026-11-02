# rpms — the Rusk package manager

`rpms` is the planned package manager for Rusk. The name is a nod to the bakery:
a rusk keeps, so packages should be plain files that can be installed, copied,
and removed without a network service or a daemon.

Status: **scaffold only**. There is no `rpms` binary yet. This folder holds the
format and the plan so the compiler, runtime, and modules can grow toward it.

## Principles

- **Offline first.** Installing a package means unpacking a file, not calling a
  registry.
- **Plain text.** Manifests are line-based and readable without a parser
  library; the source and modules inside a package are just `.rk` files.
- **No hidden state.** A package installs into a directory you can inspect and
  delete by hand.
- **One format.** A package is a directory with a `package.rpms` manifest at its
  root, optionally packed into a `.rpms` archive later.

## Manifest

`package.rpms` is one `key: value` pair per line. Blank lines and lines starting
with `#` are ignored.

```text
# package.rpms
name: text-tools
version: 0.1.0
summary: Text helpers written in Rusk
license: MIT
entry: src/text.rk
```

A runnable package may also declare a program entry point:

```text
kind: program
entry: src/main.rk
```

`rpms/example.rpms` is a complete example manifest.

## Planned commands

```console
rpms init <name>          create package.rpms and src/
rpms check                validate the manifest and every .rk file
rpms pack                 write <name>-<version>.rpms
rpms install <file.rpms>  unpack into ~/.rusk/packages/<name>/<version>/
rpms list                 show installed packages
rpms remove <name>        delete an installed package
```

`install` should run `rusk check` on every module first and refuse to install a
package that does not bake.

## Install layout

```text
~/.rusk/
  packages/
    text-tools/
      0.1.0/
        package.rpms
        src/text.rk
  bin/
    <programs linked from installed packages>
```

There is no global mutable registry and no version solving yet. A package that
depends on another will name an exact version; a lock file can come later.

## Relationship to the rest of the tree

- `src/` — the compiler that `rpms check` calls.
- `ruskmods/` — modules written in Rusk, the main thing packages will contain.
- `rusktime/` — runtime support that is part of the compiler, never a package.

## Roadmap

- a `rpms` Rust binary with `init` and `check`
- the `.rpms` archive format (a simple header plus files; no compression first)
- `install`, `list`, and `remove`
- a lock file and exact-version dependencies
- a local directory index for offline discovery
