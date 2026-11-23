# wheat

This folder is a signpost. The **wheat** terminal UI library itself lives in
[`rusklib/wheat/`](../rusklib/wheat/), where every module is ordinary Rusk
source with no dependencies and no compiler support.

Start with the [wheat README](../rusklib/wheat/README.md), or open the
[bundle entry point](../rusklib/wheat/wheat.rk) directly.

```rusk
import "wheat"

say wheat_box_around with "hello" and 0 and "rounded"
say wheat_table with ["name", "version"] and [["math", "1.2.0"]] and "grid"
say wheat_bar_labelled with "build" and 5 and 20 and 40
```

From a checkout of `main`, run that with:

```console
cargo run -- run your-program.rk
```

Every v1.2.5 release archive also contains the full `rusklib/wheat/` tree, so
the source is available whether you browse GitHub or download a platform
archive.
