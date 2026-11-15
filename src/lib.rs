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
//!
//! `toast` stores a finished program in a `.tst` container so it can be run or
//! wrapped into a native executable later without compiling again.

pub mod aarch64;
pub mod ast;
pub mod backend;
pub mod codegen;
pub mod elf;
pub mod imports;
pub mod lexer;
pub mod ownership;
pub mod parser;
pub mod pe;
pub mod repl;
pub mod toast;
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
        executable_for(
            target,
            &baked.text,
            &baked.rodata,
            baked.bss,
            &baked.imports,
        )?,
        baked.bss,
    ))
}

/// Wrap finished code in the right executable container: ELF for Linux,
/// PE32+ for Windows.
pub fn executable_for(
    target: Target,
    text: &[u8],
    rodata: &[u8],
    bss: u64,
    imports: &[String],
) -> Result<Vec<u8>, String> {
    if target.is_windows() {
        pe::build(text, rodata, bss, imports)
    } else {
        Ok(elf::build_for(target, text, rodata, bss))
    }
}

/// Write an executable. ELF images are marked executable; PE images are plain
/// files with a `.exe` name, which is how Windows expects them.
pub fn write_executable(path: &Path, target: Target, bytes: &[u8]) -> Result<(), String> {
    if target.is_windows() {
        std::fs::write(path, bytes).map_err(|e| format!("can't write {}: {e}", path.display()))
    } else {
        elf::write(path, bytes).map_err(|e| format!("can't write {}: {e}", path.display()))
    }
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
        executable_for(
            target,
            &baked.text,
            &baked.rodata,
            baked.bss,
            &baked.imports,
        )?,
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
    write_executable(output, target, &bytes)?;
    Ok(bytes.len())
}

/// Compile one source file into a toast for each requested target. Duplicate
/// targets are ignored, so `--target x86_64 --target x86_64` is harmless.
pub fn toast_path(input: &Path, targets: &[Target]) -> Result<toast::Toast, String> {
    if targets.is_empty() {
        return Err("I need at least one target for a toast".to_string());
    }
    let program = imports::expand_file(input)?;
    let mut images = Vec::with_capacity(targets.len());
    for &target in targets {
        if images
            .iter()
            .any(|image: &toast::Image| image.target == target)
        {
            continue;
        }
        let baked = codegen::compile_for(&program, target)
            .map_err(|e| format!("in {}: {e}", input.display()))?;
        images.push(toast::Image {
            target,
            text: baked.text,
            rodata: baked.rodata,
            bss: baked.bss,
            imports: baked.imports,
        });
    }
    let source = input
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| input.display().to_string());
    Ok(toast::Toast::new(
        &source,
        env!("CARGO_PKG_VERSION"),
        images,
    ))
}

/// Wrap one image from a toast into a native executable.
pub fn executable_from_toast(toast: &toast::Toast, target: Target) -> Result<Vec<u8>, String> {
    let image = toast.image_for(target).ok_or_else(|| {
        format!(
            "this toast has no {} image (it has: {})",
            target.name(),
            target_list(toast.targets().collect())
        )
    })?;
    executable_for(
        target,
        &image.text,
        &image.rodata,
        image.bss,
        &image.imports,
    )
}

/// Join target names for messages.
pub fn target_list(targets: Vec<Target>) -> String {
    if targets.is_empty() {
        return "none".to_string();
    }
    targets
        .iter()
        .map(|target| target.name())
        .collect::<Vec<_>>()
        .join(", ")
}
