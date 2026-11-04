//! Rusk — an English-like language that bakes ahead of time.
//!
//! The pipeline is small and open:
//!
//! ```text
//! .rk source -> imports -> lexer -> parser -> codegen -> ELF bytes -> executable
//! ```
//!
//! `codegen` emits AArch64 machine code directly through `aarch64::Asm`;
//! `elf` wraps that text in a static ELF64 image. There is no assembler, no
//! linker, and no libc anywhere in the chain.

pub mod aarch64;
pub mod ast;
pub mod codegen;
pub mod elf;
pub mod imports;
pub mod lexer;
pub mod ownership;
pub mod parser;
pub mod repl;
pub mod tui;
pub mod types;

use std::path::Path;

/// The three stages, in one call. Returns the finished executable bytes and
/// the size of its zeroed data area. Imports are resolved from the current
/// directory.
pub fn bake(source: &str) -> Result<(Vec<u8>, u64), String> {
    let program = imports::expand(source, None)?;
    let baked = codegen::compile(&program)?;
    Ok((elf::build(&baked.text, &baked.rodata, baked.bss), baked.bss))
}

/// Bake a `.rk` file, resolving imports relative to it.
pub fn bake_path(input: &Path) -> Result<(Vec<u8>, u64), String> {
    let program = imports::expand_file(input)?;
    let baked =
        codegen::compile(&program).map_err(|e| format!("in {}: {e}", input.display()))?;
    Ok((elf::build(&baked.text, &baked.rodata, baked.bss), baked.bss))
}

/// Read a `.rk` file and bake it to `output`, making it executable.
pub fn bake_file(input: &Path, output: &Path) -> Result<usize, String> {
    let (bytes, _) = bake_path(input)?;
    elf::write(output, &bytes).map_err(|e| format!("can't write {}: {e}", output.display()))?;
    Ok(bytes.len())
}
