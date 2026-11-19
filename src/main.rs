//! `rusk` — the command line for the Rusk language.
//!
//! ```text
//! rusk                              run the nearby Ruskfile, or start the interactive session
//! rusk repl                         start the interactive session
//! rusk tui                          start the full-screen session
//! rusk tui demo                     open the terminal module demo
//! rusk rdb [file.rk]                debug a file one statement at a time
//! rusk build <file.rk> [-o <out>]   bake a native executable
//! rusk run   [--target <arch>] <file.rk> [args...]
//!                                   bake and run it, passing arguments on
//! rusk check <file.rk>              bake without writing anything
//! rusk version
//! rusk logo                        print the platform logo (SVG on Windows)
//! ```

use rusk::{diagnostics, elf, toast, Target};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            let source = cli_source(&args, &e);
            let reporter = diagnostics::Reporter::new();
            let diagnostic = reporter.report(&e, source.as_deref());
            eprintln!("{}", diagnostic.render(true));
            if let Some(source) = source {
                if diagnostics::wants_to_edit(Some(&source), diagnostic.line) {
                    if let Err(again) = rusk::repl::run_editing(&source) {
                        eprintln!("burnt: {again}");
                    }
                }
            }
            ExitCode::FAILURE
        }
    }
}

/// Find the source behind a command-line mistake so the report can show the
/// line and, when the person says yes, reopen it in the REPL.
fn cli_source(args: &[String], error: &str) -> Option<String> {
    if let Some(rest) = error.strip_prefix("in ") {
        if let Some((path, _)) = rest.split_once(':') {
            if let Ok(source) = std::fs::read_to_string(path.trim()) {
                return Some(source);
            }
        }
    }
    for arg in args {
        if arg.starts_with('-') {
            continue;
        }
        let path = Path::new(arg);
        if path.is_file() {
            if let Ok(source) = std::fs::read_to_string(path) {
                return Some(source);
            }
        }
    }
    None
}

fn run(args: &[String]) -> Result<(), String> {
    let Some(command) = args.first().map(String::as_str) else {
        if let Some(file) = find_ruskfile() {
            return run_baked(&file, &[], Target::host());
        }
        return rusk::repl::run();
    };

    match command {
        "repl" | "console" => rusk::repl::run(),
        "tui" => {
            if args.get(1).map(String::as_str) == Some("demo") {
                rusk::tui::run_demo()
            } else {
                rusk::repl::run_tui()
            }
        }
        "rdb" | "debug" => {
            if args.len() > 2 {
                return Err(format!(
                    "rdb takes one file at a time, but found '{}'",
                    args[2]
                ));
            }
            let file = args.get(1).map(PathBuf::from);
            rusk::debugger::run_cli(file)
        }
        "build" => {
            let (input, output, target) = parse_targets(&args[1..])?;
            if is_toast(&input) {
                return build_toast(&input, output, target);
            }
            let bytes = bake_bytes(&input, target)?;
            let out = output.unwrap_or_else(|| default_output_for(&input, target));
            rusk::write_executable(&out, target, &bytes)?;
            println!(
                "baked {} -> {} ({}, native {}, 0 dependencies)",
                input.display(),
                out.display(),
                size(bytes.len()),
                target.name()
            );
            Ok(())
        }
        "toast" => {
            let (input, output, targets) = parse_toast(&args[1..])?;
            let out = output.unwrap_or_else(|| default_toast_output(&input));
            let toast = rusk::toast_path(&input, &targets)?;
            let bytes = toast.write(&out)?;
            println!(
                "toasted {} -> {} ({}, {}, 0 dependencies)",
                input.display(),
                out.display(),
                size(bytes),
                rusk::target_list(toast.targets().collect())
            );
            Ok(())
        }
        "run" => {
            let (input, program_args, target) = parse_run_target(&args[1..])?;
            run_baked(&input, &program_args, target)
        }
        "check" => {
            let (input, _, target) = parse_targets(&args[1..])?;
            if is_toast(&input) {
                return check_toast(&input);
            }
            let bytes = bake_bytes(&input, target)?;
            println!(
                "{} looks good — would bake to {} (native {}, 0 dependencies)",
                input.display(),
                size(bytes.len()),
                target.name()
            );
            Ok(())
        }
        "version" | "--version" | "-V" => {
            println!("rusk {VERSION} — code, baked ahead of time");
            Ok(())
        }
        "logo" => {
            println!("{}", rusk::logo::text());
            Ok(())
        }
        "help" | "--help" | "-h" => {
            print_help();
            Ok(())
        }
        other => Err(format!(
            "I don't know the command '{other}' — try 'rusk help'"
        )),
    }
}

fn print_help() {
    println!(
        "rusk {VERSION} — code, baked ahead of time

USAGE
  rusk                              run the nearby Ruskfile, or start the interactive session
  rusk repl                         start the interactive session
  rusk tui                          start the full-screen session
  rusk tui demo                     open the terminal module demo
  rusk rdb [file.rk]                debug a file one statement at a time; in the
                                    REPL, :rdb [file.rk] does the same
  rusk build <file.rk|file.tst> [-o <out>] [--target <arch>]
                                    bake a native executable, or wrap a toast image
  rusk toast <file.rk> [-o <out.tst>] [--target <arch>]...
                                    bake a portable precompiled toast file
  rusk run   [--target <arch>] <file.rk|file.tst> [args...]
                                    bake (or open a toast) and run it, passing arguments on
  rusk check <file.rk|file.tst> [--target <arch>]
                                    bake without writing an executable, or verify a toast
  rusk version
  rusk logo                         print the platform logo (SVG on Windows)
  rusk help

Targets: aarch64 or x86_64 (default: the architecture running the compiler).
The experimental x86-64 Windows target is spelled 'windows' and is not
selected by default.
A toast defaults to both Linux targets and can be limited with --target.
`--target all` in a toast also selects both Linux targets.

Rusk bakes straight to machine code in a static ELF file.
No C, no assembler, no linker, no libc — just syscalls.
With no command, rusk looks for a Ruskfile here or in a parent directory
and runs it; otherwise it starts the tree-walking interactive session."
    );
}

/// Look for a `Ruskfile` beside the current directory or in a parent. The
/// capitalised spelling is preferred, but `ruskfile` is accepted for systems
/// where typing capitals is awkward.
fn find_ruskfile() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    for dir in cwd.ancestors() {
        for name in ["Ruskfile", "ruskfile"] {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Bake one file, run it, and pass the remaining command-line arguments on.
/// A `.tst` input uses its already-compiled host image.
fn run_baked(input: &Path, program_args: &[String], target: Target) -> Result<(), String> {
    if target.is_windows() {
        return Err(
            "Windows executables can't be run directly on this machine. \
             Use 'rusk build --target windows' and run the .exe on Windows."
                .to_string(),
        );
    }
    let bytes = if is_toast(input) {
        let toast = toast::read(input)?;
        rusk::executable_from_toast(&toast, target)?
    } else {
        bake_bytes(input, target)?
    };
    run_bytes(&bytes, program_args)
}

fn run_bytes(bytes: &[u8], program_args: &[String]) -> Result<(), String> {
    if cfg!(windows) {
        return Err(
            "Rusk bakes Linux executables, and Windows can't run ELF files directly. \
             Use 'rusk build' and run the result in WSL or on a Linux machine."
                .to_string(),
        );
    }
    let out = std::env::temp_dir().join(format!("rusk-run-{}", std::process::id()));
    elf::write(&out, bytes).map_err(|e| format!("can't write {}: {e}", out.display()))?;
    let status = Command::new(&out)
        .args(program_args)
        .status()
        .map_err(|e| format!("can't run {}: {e}", out.display()))?;
    let _ = std::fs::remove_file(&out);
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

/// `run` takes one file and then any number of program arguments.
fn parse_run_target(args: &[String]) -> Result<(PathBuf, Vec<String>, Target), String> {
    let mut target = Target::host();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--target" => {
                let Some(name) = args.get(i + 1) else {
                    return Err("'--target' needs an architecture after it".to_string());
                };
                target = parse_target(name)?;
                i += 2;
            }
            flag if flag.starts_with('-') => {
                return Err(format!("I don't know the option '{flag}'"));
            }
            first => return Ok((PathBuf::from(first), args[i + 1..].to_vec(), target)),
        }
    }
    Err("I need a .rk file to bake".to_string())
}

fn parse_targets(args: &[String]) -> Result<(PathBuf, Option<PathBuf>, Target), String> {
    let mut input: Option<PathBuf> = None;
    let mut output: Option<PathBuf> = None;
    let mut target = Target::host();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-o" | "--output" => {
                let Some(path) = args.get(i + 1) else {
                    return Err("'-o' needs a path after it".to_string());
                };
                output = Some(PathBuf::from(path));
                i += 2;
            }
            "--target" => {
                let Some(name) = args.get(i + 1) else {
                    return Err("'--target' needs an architecture after it".to_string());
                };
                target = parse_target(name)?;
                i += 2;
            }
            flag if flag.starts_with('-') => {
                return Err(format!("I don't know the option '{flag}'"));
            }
            path => {
                if input.is_some() {
                    return Err("I can only bake one file at a time".to_string());
                }
                input = Some(PathBuf::from(path));
                i += 1;
            }
        }
    }
    let input = input.ok_or_else(|| "I need a .rk file to bake".to_string())?;
    Ok((input, output, target))
}

/// `toast` accepts the usual `-o` and one or more `--target` options. With no
/// target it stores both Linux architectures, which is the point of a portable
/// precompiled file.
fn parse_toast(args: &[String]) -> Result<(PathBuf, Option<PathBuf>, Vec<Target>), String> {
    let mut input: Option<PathBuf> = None;
    let mut output: Option<PathBuf> = None;
    let mut targets: Vec<Target> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-o" | "--output" => {
                let Some(path) = args.get(i + 1) else {
                    return Err("'-o' needs a path after it".to_string());
                };
                output = Some(PathBuf::from(path));
                i += 2;
            }
            "--target" => {
                let Some(name) = args.get(i + 1) else {
                    return Err("'--target' needs an architecture after it".to_string());
                };
                for part in name.split(',') {
                    let part = part.trim();
                    if part == "all" {
                        for target in [Target::AArch64, Target::X86_64] {
                            if !targets.contains(&target) {
                                targets.push(target);
                            }
                        }
                    } else {
                        let target = parse_target(part)?;
                        if !targets.contains(&target) {
                            targets.push(target);
                        }
                    }
                }
                i += 2;
            }
            flag if flag.starts_with('-') => {
                return Err(format!("I don't know the option '{flag}'"));
            }
            path => {
                if input.is_some() {
                    return Err("I can only toast one file at a time".to_string());
                }
                input = Some(PathBuf::from(path));
                i += 1;
            }
        }
    }
    if targets.is_empty() {
        targets = vec![Target::AArch64, Target::X86_64];
    }
    let input = input.ok_or_else(|| "I need a .rk file to toast".to_string())?;
    Ok((input, output, targets))
}

fn build_toast(input: &Path, output: Option<PathBuf>, target: Target) -> Result<(), String> {
    let toast = toast::read(input)?;
    // A single-image toast can pick that image when the host architecture is
    // not the one it holds, so `rusk build app.tst` still works for a .exe.
    let target = if toast.image_for(target).is_none() && toast.images.len() == 1 {
        toast.images[0].target
    } else {
        target
    };
    let bytes = rusk::executable_from_toast(&toast, target)?;
    let out = output.unwrap_or_else(|| default_output_for(input, target));
    rusk::write_executable(&out, target, &bytes)?;
    println!(
        "wrapped {} -> {} ({}, native {}, 0 dependencies)",
        input.display(),
        out.display(),
        size(bytes.len()),
        target.name()
    );
    Ok(())
}

fn check_toast(input: &Path) -> Result<(), String> {
    let toast = toast::read(input)?;
    println!(
        "{} is an intact toast ({}, source {}, built with rusk {})",
        input.display(),
        rusk::target_list(toast.targets().collect()),
        metadata_value(&toast.metadata, "source").unwrap_or("unknown"),
        metadata_value(&toast.metadata, "compiler").unwrap_or("unknown")
    );
    for image in &toast.images {
        println!(
            "  {}: {} code, {} data, {} zeroed",
            image.target.name(),
            size(image.text.len()),
            size(image.rodata.len()),
            size_u64(image.bss)
        );
    }
    Ok(())
}

fn metadata_value<'a>(metadata: &'a str, key: &str) -> Option<&'a str> {
    metadata.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        (name.trim() == key).then(|| value.trim())
    })
}

fn is_toast(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("tst"))
}

fn parse_target(name: &str) -> Result<Target, String> {
    Target::parse(name).ok_or_else(|| {
        format!(
            "I don't know the target '{name}' — try 'aarch64', 'x86_64', or 'windows' \
             (experimental)"
        )
    })
}

fn default_output(input: &Path) -> PathBuf {
    let mut out = input.to_path_buf();
    out.set_extension("");
    if out.as_os_str().is_empty() {
        PathBuf::from("a.out")
    } else {
        out
    }
}

/// The default name for a built program. Windows output gets the `.exe` that
/// Windows itself expects; Linux output keeps the extensionless name.
fn default_output_for(input: &Path, target: Target) -> PathBuf {
    if target.is_windows() {
        let mut out = input.to_path_buf();
        out.set_extension("exe");
        if out.as_os_str().is_empty() {
            PathBuf::from("a.exe")
        } else {
            out
        }
    } else {
        default_output(input)
    }
}

fn default_toast_output(input: &Path) -> PathBuf {
    let mut out = input.to_path_buf();
    out.set_extension("tst");
    if out.as_os_str().is_empty() {
        PathBuf::from("a.tst")
    } else {
        out
    }
}

fn size_u64(bytes: u64) -> String {
    usize::try_from(bytes)
        .map(size)
        .unwrap_or_else(|_| format!("{bytes} B"))
}

fn bake_bytes(input: &Path, target: Target) -> Result<Vec<u8>, String> {
    rusk::bake_path_with_target(input, target).map(|(bytes, _)| bytes)
}

fn size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    }
}
