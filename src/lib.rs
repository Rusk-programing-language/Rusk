//! Rusk — an English-like language that bakes ahead of time.
//!
//! The pipeline is small and open:
//!
//! ```text
//! .rk source -> lexer -> parser -> codegen -> ELF bytes -> executable
//! ```
//!
//! `codegen` emits AArch64 machine code directly through `aarch64::Asm`;
//! `elf` wraps that text in a static ELF64 image. There is no assembler, no
//! linker, and no libc anywhere in the chain.

pub mod aarch64;
pub mod ast;
pub mod codegen;
pub mod elf;
pub mod lexer;
pub mod parser;
pub mod types;

use std::path::Path;

/// The three stages, in one call. Returns the finished executable bytes and
/// the size of its zeroed data area.
pub fn bake(source: &str) -> Result<(Vec<u8>, u64), String> {
    let tokens = lexer::lex(source)?;
    let program = parser::parse(tokens)?;
    let baked = codegen::compile(&program)?;
    Ok((elf::build(&baked.text, &baked.rodata, baked.bss), baked.bss))
}

/// Read a `.rk` file and bake it to `output`, making it executable.
pub fn bake_file(input: &Path, output: &Path) -> Result<usize, String> {
    let src = std::fs::read_to_string(input)
        .map_err(|e| format!("can't read {}: {e}", input.display()))?;
    let (bytes, _) = bake(&src).map_err(|e| format!("in {}: {e}", input.display()))?;
    elf::write(output, &bytes).map_err(|e| format!("can't write {}: {e}", output.display()))?;
    Ok(bytes.len())
}
