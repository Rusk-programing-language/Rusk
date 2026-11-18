# fastlib — libraries that bake down to native primaries

`fastlib/` holds libraries whose helpers are written in Rusk but whose small
bottom layer is a language primary. They are still ordinary `.rk` files; the
compiler resolves bare names in `fastlib/` beside `ruskmods/` and `rusklib/`,
so `import "simple/simple"` works from a checkout root, from `examples/`, and
from any other directory parent.

Today the folder contains one library:

- [`simple/`](simple/README.md) — a native GUI library inspired by Tkinter and
  the `ttk` widget set. Its `core.rk` speaks to Win32 through Rusk GUI
  primaries; every other module is plain Rusk code.

```rusk
import "simple/simple"

let window be simple_window with "Hello" and 320 and 200
let button be simple_button with window and "Press" and 10 and 10 and 100 and 30
simple_window_open with window
let event be simple_event_wait
simple_window_exit
```

On Windows x86-64 those primaries create real controls. On Linux and AArch64
they check and bake as safe no-ops, so the same source always compiles. See
[`simple/README.md`](simple/README.md) for the module list and the exact event
vocabulary.
