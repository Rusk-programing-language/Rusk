//! `rusk` — the command line for the Rusk language.
//!
//! ```text
//! rusk                              start the interactive session
//! rusk repl                         start the interactive session
//! rusk tui                          start the full-screen session
//! rusk tui demo                     open the terminal module demo
//! rusk build <file.rk> [-o <out>]   bake a native executable
//! rusk run   [--target <arch>] <file.rk> [args...]
//!                                   bake and run it, passing arguments on
//! rusk check <file.rk>              bake without writing anything
//! rusk version
//! ```

use rusk::{elf, Target};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("burnt: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let Some(command) = args.first().map(String::as_str) else {
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
        "build" => {
            let (input, output, target) = parse_targets(&args[1..])?;
            let bytes = bake_bytes(&input, target)?;
            let out = output.unwrap_or_else(|| default_output(&input));
            elf::write(&out, &bytes).map_err(|e| format!("can't write {}: {e}", out.display()))?;
            println!(
                "baked {} -> {} ({}, native {}, 0 dependencies)",
                input.display(),
                out.display(),
                size(bytes.len()),
                target.name()
            );
            Ok(())
        }
        "run" => {
            let (input, program_args, target) = parse_run_target(&args[1..])?;
            let bytes = bake_bytes(&input, target)?;
            if cfg!(windows) {
                return Err(
                    "Rusk bakes Linux executables, and Windows can't run ELF files directly. \
                     Use 'rusk build' and run the result in WSL or on a Linux machine."
                        .to_string(),
                );
            }
            let out = std::env::temp_dir().join(format!("rusk-run-{}", std::process::id()));
            elf::write(&out, &bytes).map_err(|e| format!("can't write {}: {e}", out.display()))?;
            let status = Command::new(&out)
                .args(&program_args)
                .status()
                .map_err(|e| format!("can't run {}: {e}", out.display()))?;
            let _ = std::fs::remove_file(&out);
            if !status.success() {
                std::process::exit(status.code().unwrap_or(1));
            }
            Ok(())
        }
        "check" => {
            let (input, _, target) = parse_targets(&args[1..])?;
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
  rusk                              start the interactive session
  rusk repl                         start the interactive session
  rusk tui                          start the full-screen session
  rusk tui demo                     open the terminal module demo
  rusk build <file.rk> [-o <out>] [--target <arch>]
                                    bake a native executable
  rusk run   [--target <arch>] <file.rk> [args...]
                                    bake it and run it, passing arguments on
  rusk check <file.rk> [--target <arch>]
                                    bake without writing an executable
  rusk version
  rusk help

Targets: aarch64 or x86_64 (default: the architecture running the compiler).

Rusk bakes straight to machine code in a static ELF file.
No C, no assembler, no linker, no libc — just syscalls.
The REPL is a tree-walking session over the same syntax and type checker."
    );
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

fn parse_target(name: &str) -> Result<Target, String> {
    Target::parse(name)
        .ok_or_else(|| format!("I don't know the target '{name}' — try 'aarch64' or 'x86_64'"))
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
