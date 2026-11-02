# rusktime — the Rusk runtime

This folder is for the part of a baked program that is not the user's code: the
runtime. Rusk compiles to a static AArch64 ELF with no libc, so the runtime is
the small set of routines and conventions that every executable carries.

## What belongs here

| Concern | Responsibility |
|---|---|
| entry | `_start`, stack setup, calling the top-level code, exiting |
| syscalls | `write(2)`, `exit(2)`, and any future kernel calls |
| text | the `[len: u64][bytes]` representation, joining, comparing |
| text access | `length of`, `letter N of`, `part A to B of` |
| lists | the `[len: u64][item: i64]...` representation, indexing, item assignment |
| numbers | signed 64-bit arithmetic and the itoa loop used by `say` |
| allocation | the bump pointer and the program heap |
| failure | writing `burnt: line N: ...` to standard error and exiting 1 |

## Where it is today

The runtime is real, but it is not in this folder yet. It lives in the compiler:

- `src/codegen.rs` — text concatenation, itoa, `text_letter`, `text_part`,
  `array_get`, `array_set`, `runtime_fail`, and the function-call frame
  convention;
- `src/aarch64.rs` — the instruction encoder used to build all of it;
- `src/elf.rs` — the static ELF wrapper that puts entry, code, and data in one
  `PT_LOAD` segment.

This folder is the planned home for extracting those routines into their own
Rust modules with their own tests. Moving them must not change the generated
machine code, so the existing end-to-end tests are the guard rail.

## Layout we are moving toward

```text
rusktime/
  entry.rs      _start and process exit
  syscall.rs    write/exit wrappers
  text.rs       join, compare, letter, part
  list.rs       indexing, item assignment, bounds errors
  number.rs     itoa and number formatting
  alloc.rs      the bump heap
  fail.rs       runtime error reporting
```

These will stay Rust files that emit AArch64 through `aarch64::Asm`. They are
not Rusk modules; reusable Rusk code belongs in `ruskmods/`.

## Invariants

- No libc, no external runtime, no dynamic loader.
- One RWX `PT_LOAD` segment at `0x400000`; code starts at `0x400078`.
- Data is at `0x410000`; the bump heap starts at `0x420000` and is 1 MiB.
- Text is `[len: u64][bytes...]`; positions count bytes and start at 1.
- Lists are `[len: u64][item: i64]...`; positions start at 1 and the same array
  is shared by every name that holds it.
- Numbers are signed 64-bit and wrap on overflow.
- Runtime errors print one sentence to file descriptor 2 and exit with 1.
- Baking is deterministic: the same source always gives the same bytes.

The full language-visible contract is in [`../docs/SPEC.md`](../docs/SPEC.md).
