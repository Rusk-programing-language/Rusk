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
let running be true
while running:
    let event be simple_event_wait
    if event is "close":
        set running to false
    if event is "click":
        if (simple_event_id) is button:
            simple_control_set_text with button and "Thanks"
simple_window_exit
```

Bake it with `rusk build app.rk --target windows -o app.exe`, then run `app.exe`
on Windows. `--target windows` matters: the default target still produces a
Linux ELF, where every GUI primary is a typed no-op. `rusk check` and the REPL
can only check syntax and types, so they never show a window. Keep a
`simple_event_wait` loop alive after opening the window; showing it does not
start a loop for you.

On Windows x86-64 those primaries create real controls. On Linux and AArch64
they check and bake as safe no-ops, so the same source always compiles. See
[`simple/README.md`](simple/README.md) for the module list, the exact event
vocabulary, and a troubleshooting checklist.
