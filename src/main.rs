//! `rusk` — the command line for the Rusk language.
//!
//! ```text
//! rusk build <file.rk> [-o <out>]   bake a native executable
//! rusk run   <file.rk>              bake and run it
//! rusk check <file.rk>              bake without writing anything
//! rusk version
//! ```

use rusk::elf;
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
        print_help();
        return Ok(());
    };

    match command {
        "build" => {
            let (input, output) = parse_targets(&args[1..])?;
            let bytes = bake_bytes(&input)?;
            let out = output.unwrap_or_else(|| default_output(&input));
            elf::write(&out, &bytes).map_err(|e| format!("can't write {}: {e}", out.display()))?;
            println!(
                "baked {} -> {} ({}, native, 0 dependencies)",
                input.display(),
                out.display(),
                size(bytes.len())
            );
            Ok(())
        }
        "run" => {
            let (input, _) = parse_targets(&args[1..])?;
            let bytes = bake_bytes(&input)?;
            let out = std::env::temp_dir().join(format!("rusk-run-{}", std::process::id()));
            elf::write(&out, &bytes).map_err(|e| format!("can't write {}: {e}", out.display()))?;
            let status = Command::new(&out)
                .status()
                .map_err(|e| format!("can't run {}: {e}", out.display()))?;
            let _ = std::fs::remove_file(&out);
            if !status.success() {
                std::process::exit(status.code().unwrap_or(1));
            }
            Ok(())
        }
        "check" => {
            let (input, _) = parse_targets(&args[1..])?;
            let bytes = bake_bytes(&input)?;
            println!(
                "{} looks good — would bake to {} (native, 0 dependencies)",
                input.display(),
                size(bytes.len())
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
  rusk build <file.rk> [-o <out>]   bake a native executable
  rusk run   <file.rk>              bake it and run it
  rusk check <file.rk>              bake without writing an executable
  rusk version
  rusk help

Rusk bakes straight to AArch64 machine code in an ELF file.
No C, no assembler, no linker, no libc — just syscalls."
    );
}

fn parse_targets(args: &[String]) -> Result<(PathBuf, Option<PathBuf>), String> {
    let mut input: Option<PathBuf> = None;
    let mut output: Option<PathBuf> = None;
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
    Ok((input, output))
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

fn bake_bytes(input: &Path) -> Result<Vec<u8>, String> {
    let src = std::fs::read_to_string(input)
        .map_err(|e| format!("can't read {}: {e}", input.display()))?;
    rusk::bake(&src)
        .map(|(bytes, _)| bytes)
        .map_err(|e| format!("in {}: {e}", input.display()))
}

fn size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    }
}
