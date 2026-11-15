# wheat — a small terminal UI library in Rusk

`wheat` is the bundled Rusk library for terminal output: widths, colours,
styles, boxes, tables, progress bars, panels, layout, themes, and the handful of
escape sequences a full-screen program needs. It is inspired by Python's Rich,
but it is much smaller. Every module is ordinary Rusk source under this folder,
with no dependencies and no compiler support: the library builds strings, and
the caller decides where to send them.

```rusk
import "wheat"

say wheat_panel_full with "Installed" and "2 items" and "math 1.2.0\ntext 2.1.0" and 0 and "left" and 1 and "rounded"
say wheat_table with ["name", "version"] and [["math", "1.2.0"], ["text", "2.1.0"]] and "grid"
say wheat_bar_labelled with "build" and 5 and 20 and 40
```

A bare module name is resolved next to the importing file, then in `ruskmods/`
and `rusklib/` beside it and in each parent directory. That makes both of these
work from anywhere in the checkout:

```rusk
import "wheat"          # the whole library
import "wheat/table"    # just one module
```

Importing one module compiles less code. The bundle is a convenience, not a
requirement.

## Colour modes

Colours are encoded according to `wheat_color_system`, which reads the
environment:

| Setting | Result |
|---|---|
| `WHEAT_COLOR=none` (or `off`, `0`) | no escape codes at all |
| `WHEAT_COLOR=standard` (or `16`) | the 16 basic colours |
| `WHEAT_COLOR=256` (or `eight-bit`, `8bit`) | the xterm 256-colour cube |
| `WHEAT_COLOR=truecolor` (or `24bit`, `true`) | 24-bit `38;2;r;g;b` |
| `WHEAT_COLOR=auto` | guess from the variables below |

When `WHEAT_COLOR` is unset, the guess is:

1. `NO_COLOR` set (to anything non-empty) → `none`;
2. `FORCE_COLOR=0` or `FORCE_COLOR=false` → `none`;
3. `COLORTERM=truecolor` or `COLORTERM=24bit` → `truecolor`;
4. `TERM=dumb` or `TERM=unknown` → `none`;
5. `TERM` contains `256color` → `256`;
6. otherwise → `standard`.

The default is `standard`, not "detect a terminal". Rusk cannot portably ask
whether standard output is a tty, so redirecting a wheat program still writes
colour unless you set `WHEAT_COLOR=none`. That is the honest behaviour: the
environment decides, not a guess about the file descriptor.

A colour can be a name (`red`, `bright_blue`, `gray`), a 24-bit hex value
(`#ff8800`), or a 256-colour index (`color(196)`). Styles are plain text and may
combine attributes and colours:

```rusk
say wheat_style_apply with "important" and "bold red"
say wheat_style_apply with "noted" and "white on blue"
say wheat_style_apply with "accent" and "bold #ff8800"
say wheat_theme_render with "[title]installed[/] [number]2[/] packages"
```

Markup tags become segments; `wheat_theme_render` maps a tag through the default
theme when the tag is a theme name, and leaves it as a plain style otherwise.
`wheat_console_print_markup` renders markup and writes it with `say`.

## Modules

### `text.rk` — strings and measuring

Trimming, prefix/suffix tests, search, split, join, replace, upper/lower,
number-to-text and text-to-number, ANSI stripping, padding, cropping, ellipsis,
line cropping, wrapping, markup splitting, and repeat. The legacy `text_*`
helpers (`text_is_empty`, `text_first`, `text_last`, `text_repeat`,
`text_reverse`, `text_shout`, `text_initials`, `text_contains`) also live here,
so a program that used the old `ruskmods/text.rk` can import `"wheat/text"`
instead.

Measurement is in **display cells**. `wheat_text_width` counts the drawing
glyphs wheat itself uses (box corners, blocks, arrows, ticks, and so on) as one
cell each even though they are three bytes in UTF-8. Other non-ASCII text is
measured by its UTF-8 byte length, because Rusk strings are byte-indexed and the
library has no Unicode tables. That is the single most important limitation to
know: a CJK character, an emoji, or a combining mark will be measured as its
bytes and will push padding out of alignment.

### `color.rk` — colours

Colour names, hex parsing, the 256-colour palette, nearest-colour search,
blending, foreground and background codes, and the `wheat_color_system`
detection described above. `wheat_color_demo` prints every basic name.

### `style.rk` — style specs

Splits a spec such as `"bold red"` or `"white on blue"` into ANSI parameters,
applies it to a body, merges two specs, and tests or adds one token. All
functions have an `_in` form that takes an explicit colour system, plus a short
form that uses `wheat_color_system`.

### `segment.rk` — styled text

A segment is `[text, style]`; a line is a list of segments; a block is a list of
lines. Segments can be measured, padded, cropped, joined, rendered to ANSI, or
reduced to plain text. `wheat_line_from_markup` turns `[bold]…[/]` text into a
line.

### `measure.rk` — block geometry

Height, width, longest line, wrapping, per-column widths, total width with a
gap, fitting columns into an available width, and a one-line description of a
block. These are the measuring primitives the drawing modules use.

### `box.rk` — frames

Light, heavy, double, rounded, ASCII, and borderless styles. Top and bottom
rules, a title in the top edge, framed content lines, an empty frame, section
dividers, and padding without a border. A width of `0` means "as wide as the
widest line plus the frame".

### `bar.rk` — progress and small charts

Two-glyph and eighth-cell progress bars, labelled bars, markup bars, a travelling
pulse for unknown totals, sparklines, vertical column charts, Braille and ASCII
spinners, and a gauge with the percentage drawn inside it. Progress is whole
numbers from `0` to `total`; `total <= 0` is treated as "nothing done yet".

### `columns.rk` — side by side

Joins blocks into rows, pads shorter blocks so columns keep their width, builds
fixed-column grids, and packs chips into lines of a given width. `pack_rows`
reports how many lines a pack would take without building it.

### `panel.rk` — boxed sections

A panel is a box plus title, padding, alignment (`left`, `center`, `right`), and
an optional right-aligned subtitle. `wheat_panel_full` is the general form;
`wheat_panel` and `wheat_panel_titled` are the common short forms;
`wheat_panel_wide` uses the detected terminal width.

### `table.rk` — rows and columns

A header row plus a list of rows of text cells. Styles are `grid`, `simple`,
`rounded`, `heavy`, `double`, `ascii`, `plain`, and `none`. Columns are sized to
their widest cell; `wheat_table_fitted` narrows columns to a total width and
crops cells; `wheat_table_aligned` takes one alignment per column;
`wheat_table_body` draws rows without a header.

### `layout.rk` — the other axis

Stacks blocks with gaps, splits a block after a line or a display column,
centres or right-aligns a block, stretches it to a rectangle, cuts it into
roughly equal pieces, and builds a header / body / footer page of a fixed width
and height.

### `theme.rk` — named styles

A theme is a list of `[name, style]` pairs. The default covers `title`,
`subtitle`, `muted`, `accent`, `info`, `success`, `warning`, `error`, `danger`,
`border`, `header`, `key`, `value`, `path`, `number`, `ok`, and `fail`. Themes
can be inspected, changed, merged, applied by name, or used as markup tags.

### `console.rk` — the terminal layer

Terminal width and height from `COLUMNS` / `LINES` with an `80x24` fallback,
escape sequences as returned strings (clear, move, save/restore, hide/show
cursor, alternate screen, carriage return), rules with an optional centred
title, printing with and without a newline, markup printing, status marks
(`ok`, `warn`, `fail`, `info`), a one-line progress readout, and a demo.

Control functions **return** their escape sequence; they do not write it. The
`print` functions do write. The exception is `wheat_console_bell`, which is a
single BEL byte.

### `keys.rk` — input

Names for bytes from `read key`, including `enter`, `tab`, `space`, `escape`,
`backspace`, `eof`, and the arrow keys. Arrow and home/end sequences are read
with two extra blocking reads, so a lone Escape cannot be distinguished from
the start of a sequence: Rusk has no timer. `wheat_keys_wait_for` and
`wheat_keys_confirm` handle a small input loop.

### `app.rk` — repeated screen pieces

Menus with a pointer, checklists, prompt and secret lines, a status bar, a
dialog, a full frame, screen clear/refresh helpers, a keymap table, and a
banner. These build strings; pairing them with `console.rk` and `keys.rk` is up
to the program.

### `wheat.rk` — the bundle

`import "wheat"` imports every module above.

## What wheat is not

- **Not a widget toolkit.** There is no retained component tree, no event loop,
  no focus manager. Functions build text and return it.
- **Not Unicode-complete.** Widths are correct for ASCII and for wheat's own
  drawing glyphs. Other multi-byte text is measured by byte length.
- **Not a terminal-size query.** `COLUMNS` and `LINES` are environment
  variables, not an `ioctl`. Unset means `80x24`.
- **Not tty-aware.** Colour follows environment variables, not whether stdout
  is a terminal. Set `WHEAT_COLOR=none` for logs and pipes.
- **Not a raw-mode layer.** `turn on raw terminal` is a language primitive;
  wheat reads and renders, it does not switch the terminal mode.
- **Not dependency-free of the OS in one place.** `console.rk` writes to
  `/dev/stdout`; `keys.rk` reads standard input. Everything else is pure text
  manipulation.

The library favours plain text, whole-number arithmetic, and explicit sizing so
that each helper can be read, copied, and tested on its own.
