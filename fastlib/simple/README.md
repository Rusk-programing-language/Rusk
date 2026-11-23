# simple — a small native GUI library for Rusk

`simple` is the GUI library in [`fastlib/`](../README.md). It takes its names
from Tkinter and `ttk`, but every module is written in Rusk. The only native
layer is a handful of primaries in `core.rk`; everything above it is ordinary
`.rk` code with the `simple_` prefix.

```rusk
import "simple/simple"

let window be simple_window with "Hello" and 320 and 200
let button be simple_button with window and "Press" and 10 and 10 and 100 and 30
let status be simple_label with window and "Waiting" and 10 and 60 and 280 and 24

simple_window_open with window
let running be true
while running:
    let event be simple_event_wait
    if event is "close":
        set running to false
    if event is "click":
        if (simple_event_id) is button:
            simple_control_set_text with status and "Thank you"
simple_window_exit
```

Bake it for Windows x86-64:

```console
rusk build app.rk --target windows -o app.exe
```

On Windows the window is a real Win32 window and the controls are real button,
static, and edit controls. The PE import table still only names `kernel32.dll`:
the runtime loads `user32.dll` with `LoadLibraryA` and resolves each function
with `GetProcAddress`. On other targets every GUI primary is a safe no-op, so
`rusk check app.rk` and Linux bakes keep working. Window class registration,
message pumping, and the `WndProc` live in the compiler's Windows backend, not
in a Rusk callback.

## If no window appears

- Bake with `rusk build app.rk --target windows -o app.exe`. The default target
  is a Linux architecture even on a Windows host, where GUI primaries are
  no-ops. `--target windows` is not selected automatically.
- Run the resulting `.exe` on Windows. `rusk run --target windows` is refused,
  and `rusk check`, `rusk run` without the target, and the REPL only prove that
  the source parses and type-checks; none of them open a window.
- Keep the event loop from the example above alive after
  `simple_window_open`. Showing the window only calls `ShowWindow` and
  `UpdateWindow`; it does not run a message loop for you.
- If the program uses `import "simple/simple"` from outside a checkout, make
  sure the installed compiler has a `fastlib/` folder beside `rusk.exe` (or
  `rusk`). The POSIX and native Windows installers copy the bundled library
  folders there.
- Only one window is supported at a time. The event slots are shared, so a
  second window reuses the first one's handle.

The emitted PE uses the console subsystem, so a console window may appear next
to the GUI window. That is expected for this experimental target.

## The raw primaries

These are language expressions and statements, not Rusk functions. `core.rk`
wraps them with friendlier names.

| Form | Gives back / does |
|---|---|
| `make simple window titled TITLE wide W high H` | a window number |
| `add simple button to W labelled TEXT at X and Y sized W and H` | a control number |
| `add simple label to W labelled TEXT at X and Y sized W and H` | a control number |
| `add simple field to W labelled TEXT at X and Y sized W and H` | a control number |
| `show simple window W` | show the window |
| `close simple window` | close the window |
| `set simple text of C to TEXT` | change a control's text |
| `simple text of C` | the control's text |
| `show simple message TEXT` | a one-button message box |
| `simple confirm with TEXT` | a true/false yes/no box |
| `pump simple events` | deliver waiting messages once |
| `next simple event` | the next event kind |
| `simple event control` | the control number of the last event |

`next simple event` gives `""`, `"click"`, `"change"`, or `"close"`. Events are
recorded by the native `WndProc` and read by the Rusk event loop, so callbacks
never cross the language boundary.

## Modules

| File | What it holds |
|---|---|
| `core.rk` | Rusk wrappers for the native primaries |
| `constants.rk` | event kinds, alignments, colours, styles, and key names |
| `geometry.rk` | rectangles, points, bounds, grow, inset, centre |
| `window.rk` | create, open, exit, and start a window |
| `control.rk` | text get/set, clear, append, mask, click/change tests |
| `button.rk` | button creation, geometry wrapper, press/reset helpers |
| `label.rk` | label creation, geometry wrapper, word-wrapped labels |
| `field.rk` | edit-field creation, value helpers, change test |
| `frame.rk` | rectangles and title labels for grouped controls |
| `layout.rk` | grids, rows, columns, centre, and splits |
| `event.rk` | wait, wait-for, wait-any, and event kind tests |
| `app.rk` | a tiny run loop and a greeting window |
| `textbox.rk` | join, split, find, pad, trim, wrap, number/text conversion |
| `messagebox.rk` | info, warning, error, question, multiline messages |
| `filedialog.rk` | path filters, file names, and confirm-before-open/save |
| `colorchooser.rk` | named colours, hex colours, RGB conversion, validation |
| `dialog.rk` | common dialogs in one module |
| `commondialog.rk` | short `simple_ask_*` names for those dialogs |
| `theme.rk` | named text themes and decoration helpers |
| `style.rk` | space-separated style specifications |
| `ttk.rk` | themed button, label, and field wrappers |
| `menu.rk` | menu text, lookup, and a menu label |
| `toolbar.rk` | text strips and rows of buttons |
| `statusbar.rk` | a single-line status label |
| `progress.rk` | text progress bars and label updates |
| `listbox.rk` | selectable list rendering and next/previous helpers |
| `canvas.rk` | a character canvas drawn in a label |
| `keys.rk` | key names, tests, and hint text |
| `dnd.rk` | text and path payloads for drag-and-drop-style programs |
| `simple.rk` | bundle that imports every module |
| `demo.rk` | a small window using the library |

Import one file when that is all a program needs:

```rusk
import "simple/event"
let event be simple_event_wait
```

Or import the bundle:

```rusk
import "simple/simple"
```

Every public name keeps its `simple_` prefix because a plain Rusk import shares
one global namespace. Imported files are included at most once, so importing
both `simple/button` and `simple/simple` is harmless.

## What is not native yet

The first release has buttons, labels, and fields. `frame`, `menu`, `toolbar`,
`canvas`, `listbox`, `progress`, and `dnd` are Rusk-level models that render to
labels or buttons; they are useful and fully checkable, but they are not native
OS widgets. There is no native key event, tree view, real drag-and-drop, or
HiDPI layout. Those would need more primaries in the compiler and can be added
without changing the module names above.
