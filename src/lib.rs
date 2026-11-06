//! Rusk — an English-like language that bakes ahead of time.
//!
//! The pipeline is small and open:
//!
//! ```text
//! .rk source -> imports -> lexer -> parser -> codegen -> ELF bytes -> executable
//! ```
//!
//! `codegen` emits machine code directly through `backend::Asm`; the selected
//! target is AArch64 or x86-64. `elf` wraps that text in a static ELF64 image.
//! There is no assembler, no linker, and no libc anywhere in the chain.

pub mod aarch64;
pub mod ast;
pub mod backend;
pub mod codegen;
pub mod elf;
pub mod imports;
pub mod lexer;
pub mod ownership;
pub mod parser;
pub mod repl;
pub mod tui;
pub mod types;
pub mod x86_64;

pub use backend::Target;

use std::path::Path;

/// The three stages, in one call, for the host architecture. Returns the
/// finished executable bytes and the size of its zeroed data area. Imports are
/// resolved from the current directory.
pub fn bake(source: &str) -> Result<(Vec<u8>, u64), String> {
    bake_with_target(source, Target::host())
}

/// Like [`bake`], but for an explicit target.
pub fn bake_with_target(source: &str, target: Target) -> Result<(Vec<u8>, u64), String> {
    let program = imports::expand(source, None)?;
    let baked = codegen::compile_for(&program, target)?;
    Ok((
        elf::build_for(target, &baked.text, &baked.rodata, baked.bss),
        baked.bss,
    ))
}

/// Bake a `.rk` file for the host architecture, resolving imports relative to
/// it.
pub fn bake_path(input: &Path) -> Result<(Vec<u8>, u64), String> {
    bake_path_with_target(input, Target::host())
}

/// Bake a `.rk` file for an explicit target.
pub fn bake_path_with_target(input: &Path, target: Target) -> Result<(Vec<u8>, u64), String> {
    let program = imports::expand_file(input)?;
    let baked = codegen::compile_for(&program, target)
        .map_err(|e| format!("in {}: {e}", input.display()))?;
    Ok((
        elf::build_for(target, &baked.text, &baked.rodata, baked.bss),
        baked.bss,
    ))
}

/// Read a `.rk` file and bake it to `output`, making it executable.
pub fn bake_file(input: &Path, output: &Path) -> Result<usize, String> {
    bake_file_with_target(input, output, Target::host())
}

/// Read a `.rk` file and bake it to `output` for an explicit target.
pub fn bake_file_with_target(input: &Path, output: &Path, target: Target) -> Result<usize, String> {
    let (bytes, _) = bake_path_with_target(input, target)?;
    elf::write(output, &bytes).map_err(|e| format!("can't write {}: {e}", output.display()))?;
    Ok(bytes.len())
}
