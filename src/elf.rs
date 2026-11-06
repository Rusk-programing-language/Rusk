//! Writes a static ELF64 executable for Linux (AArch64 or x86-64).
//!
//! One `PT_LOAD` segment covers everything: headers, code, strings, and the
//! zero-filled data area. No interpreter, no dynamic linking, no sections.
//! The kernel loads the file, jumps to `_start`, and Rusk talks to it through
//! raw syscalls.

use crate::aarch64::{IMAGE_BASE, TEXT_OFFSET};
use crate::backend::Target;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// Fixed address of the zeroed data area, above any plausible text size.
pub const DATA_BASE: u64 = 0x41_0000;

fn put_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_u64(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_le_bytes());
}

/// Build the executable bytes for the host's default target.
pub fn build(text: &[u8], rodata: &[u8], bss: u64) -> Vec<u8> {
    build_for(Target::host(), text, rodata, bss)
}

/// Build the executable bytes for a chosen target. `bss` is how many zeroed
/// bytes the program needs above `DATA_BASE`.
pub fn build_for(target: Target, text: &[u8], rodata: &[u8], bss: u64) -> Vec<u8> {
    let filesz = TEXT_OFFSET + text.len() as u64 + rodata.len() as u64;
    let file_end = IMAGE_BASE + filesz;
    // Lay the writable data out above the file so the kernel zero-fills it.
    let data_start = if bss > 0 { DATA_BASE } else { file_end };
    let memsz = if bss > 0 {
        DATA_BASE + bss - IMAGE_BASE
    } else {
        filesz
    };
    let entry = IMAGE_BASE + TEXT_OFFSET;
    let _ = data_start;

    let mut out = Vec::with_capacity(filesz as usize);

    // ---- ELF header (64 bytes) ----
    out.extend_from_slice(&[0x7F, b'E', b'L', b'F']);
    out.push(2); // ELFCLASS64
    out.push(1); // ELFDATA2LSB
    out.push(1); // EV_CURRENT
    out.push(0); // ELFOSABI_SYSV
    out.extend_from_slice(&[0; 8]);
    put_u16(&mut out, 2); // ET_EXEC
    put_u16(&mut out, target.machine());
    put_u32(&mut out, 1); // EV_CURRENT
    put_u64(&mut out, entry);
    put_u64(&mut out, 64); // e_phoff
    put_u64(&mut out, 0); // e_shoff
    put_u32(&mut out, 0); // e_flags
    put_u16(&mut out, 64); // e_ehsize
    put_u16(&mut out, 56); // e_phentsize
    put_u16(&mut out, 1); // e_phnum
    put_u16(&mut out, 0); // e_shentsize
    put_u16(&mut out, 0); // e_shnum
    put_u16(&mut out, 0); // e_shstrndx

    debug_assert_eq!(out.len(), 64);

    // ---- program header (56 bytes) ----
    put_u32(&mut out, 1); // PT_LOAD
    put_u32(&mut out, 7); // PF_R | PF_W | PF_X
    put_u64(&mut out, 0); // p_offset
    put_u64(&mut out, IMAGE_BASE); // p_vaddr
    put_u64(&mut out, IMAGE_BASE); // p_paddr
    put_u64(&mut out, filesz); // p_filesz
    put_u64(&mut out, memsz); // p_memsz
    put_u64(&mut out, 0x1000); // p_align

    debug_assert_eq!(out.len() as u64, TEXT_OFFSET);

    out.extend_from_slice(text);
    out.extend_from_slice(rodata);
    out
}

pub fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    fs::write(path, bytes)?;
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_is_well_formed() {
        let elf = build(&[0xC3], b"", 0);
        assert_eq!(&elf[0..4], &[0x7F, b'E', b'L', b'F']);
        assert_eq!(elf.len(), 0x78 + 1);
        let entry = u64::from_le_bytes(elf[24..32].try_into().unwrap());
        assert_eq!(entry, IMAGE_BASE + TEXT_OFFSET);
        let phoff = u64::from_le_bytes(elf[32..40].try_into().unwrap());
        assert_eq!(phoff, 64);
        let memsz = u64::from_le_bytes(elf[64 + 40..64 + 48].try_into().unwrap());
        assert_eq!(memsz, (TEXT_OFFSET as usize + 1) as u64);
    }

    #[test]
    fn bss_extends_the_mapping() {
        let elf = build(&[0xC3], b"", 16);
        let memsz = u64::from_le_bytes(elf[64 + 40..64 + 48].try_into().unwrap());
        assert_eq!(memsz, DATA_BASE + 16 - IMAGE_BASE);
    }

    #[test]
    fn machine_word_follows_the_target() {
        let aarch64 = build_for(Target::AArch64, &[0xC3], b"", 16);
        let x86 = build_for(Target::X86_64, &[0xC3], b"", 16);
        assert_eq!(u16::from_le_bytes(aarch64[18..20].try_into().unwrap()), 183);
        assert_eq!(u16::from_le_bytes(x86[18..20].try_into().unwrap()), 62);
    }
}
