//! Writes an experimental PE32+ executable for Windows x86-64.
//!
//! The layout is deliberately plain: four sections (`.text`, `.rdata`, `.bss`,
//! `.idata`), a fixed image base, and an import table for `kernel32.dll`. There
//! are no base relocations, so Windows must load the image at its preferred
//! base. The generated code also uses absolute addresses, which is why this is
//! an experimental first step rather than a general-purpose Windows backend.
//!
//! The import table is built by hand. Each imported function gets one IAT slot;
//! the code generator patches a `mov rax, [slot]` at finish time, and the
//! Windows loader fills the slot with the real address.

/// Preferred load address. Kept well clear of the low 4 GiB so 64-bit absolute
/// addresses are unambiguous.
pub const IMAGE_BASE: u64 = 0x1_4000_0000;
/// RVA of the code section, which is also the entry point.
pub const TEXT_RVA: u64 = 0x1000;
/// Alignment of sections in memory.
pub const SECTION_ALIGN: u64 = 0x1000;
/// Alignment of sections in the file.
pub const FILE_ALIGN: u64 = 0x200;
/// DOS header, PE headers, and section headers all fit here.
pub const HEADERS_SIZE: u64 = 0x400;
/// Size of the PE32+ optional header.
const OPTIONAL_HEADER_SIZE: u16 = 240;
/// Number of sections we write.
const SECTION_COUNT: u16 = 4;

/// Where each piece of the image lands. Called by both the code generator (to
/// patch absolute addresses) and the PE writer, so the two always agree.
#[derive(Debug, Clone, Copy)]
pub struct Layout {
    pub text_rva: u64,
    pub text_raw: u64,
    pub text_raw_size: u64,
    pub rodata_rva: u64,
    pub rodata_raw: u64,
    pub rodata_raw_size: u64,
    pub bss_rva: u64,
    pub idata_rva: u64,
    pub idata_raw: u64,
}

fn align(value: u64, to: u64) -> u64 {
    value.div_ceil(to) * to
}

/// Compute the layout for an image with `text_len` code bytes, `rodata_len`
/// read-only bytes, `bss` zeroed bytes, and `import_count` imported functions.
pub fn layout(text_len: usize, rodata_len: usize, bss: u64, import_count: usize) -> Layout {
    let text_rva = TEXT_RVA;
    let text_raw = HEADERS_SIZE;
    let text_raw_size = align(text_len as u64, FILE_ALIGN);
    let rodata_rva = align(text_rva + text_len as u64, SECTION_ALIGN);
    let rodata_raw = text_raw + text_raw_size;
    let rodata_raw_size = align(rodata_len as u64, FILE_ALIGN);
    let bss_rva = align(rodata_rva + rodata_len as u64, SECTION_ALIGN);
    let idata_rva = align(bss_rva + bss, SECTION_ALIGN);
    let idata_raw = rodata_raw + rodata_raw_size;
    let _ = import_count;
    Layout {
        text_rva,
        text_raw,
        text_raw_size,
        rodata_rva,
        rodata_raw,
        rodata_raw_size,
        bss_rva,
        idata_rva,
        idata_raw,
    }
}

/// The number of bytes in an import table with `count` functions.
fn import_table_len(count: usize) -> u64 {
    40 + 16 * (count as u64 + 1)
}

/// RVA of the IAT slot for import `index`, given the import count.
pub fn iat_slot_rva_with_count(layout: &Layout, import_count: usize, index: usize) -> u64 {
    let ilt_rva = layout.idata_rva + 40;
    let iat_rva = ilt_rva + 8 * (import_count as u64 + 1);
    iat_rva + 8 * index as u64
}

/// RVA of the IAT, given the import count.
pub fn iat_rva(layout: &Layout, import_count: usize) -> u64 {
    layout.idata_rva + 40 + 8 * (import_count as u64 + 1)
}

/// Build a PE32+ executable image.
///
/// The import names must be plain ASCII identifiers; they are written as
/// `kernel32.dll` hint/name entries. The caller is responsible for only asking
/// for functions that actually live in `kernel32.dll`.
pub fn build(text: &[u8], rodata: &[u8], bss: u64, imports: &[String]) -> Result<Vec<u8>, String> {
    for name in imports {
        if name.is_empty()
            || name.len() > 255
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'@' || b == b'?')
        {
            return Err(format!("'{name}' is not a usable Windows import name"));
        }
    }
    let count = imports.len();
    let layout = layout(text.len(), rodata.len(), bss, count);
    let idata = build_import_table(&layout, imports)?;
    let idata_len = idata.len() as u64;
    let idata_raw_size = align(idata_len, FILE_ALIGN);
    let end_rva = layout
        .idata_rva
        .checked_add(idata_len)
        .ok_or_else(|| "the Windows image is too large".to_string())?;
    let end_raw = layout
        .idata_raw
        .checked_add(idata_raw_size)
        .ok_or_else(|| "the Windows image is too large".to_string())?;
    if end_rva > u32::MAX as u64 || end_raw > u32::MAX as u64 {
        return Err("the Windows image is too large for PE32+".to_string());
    }
    let size_of_image = align(end_rva, SECTION_ALIGN);
    let iat = iat_rva(&layout, count);

    let mut out = vec![0u8; HEADERS_SIZE as usize];

    // DOS header: enough to point at the PE header.
    out[0] = b'M';
    out[1] = b'Z';
    put_u32_at(&mut out, 0x3C, 0x80);

    // PE signature and COFF header.
    out[0x80..0x84].copy_from_slice(b"PE\0\0");
    put_u16_at(&mut out, 0x84, 0x8664); // IMAGE_FILE_MACHINE_AMD64
    put_u16_at(&mut out, 0x86, SECTION_COUNT);
    put_u32_at(&mut out, 0x88, 0); // TimeDateStamp
    put_u32_at(&mut out, 0x8C, 0); // PointerToSymbolTable
    put_u32_at(&mut out, 0x90, 0); // NumberOfSymbols
    put_u16_at(&mut out, 0x94, OPTIONAL_HEADER_SIZE);
    // EXECUTABLE_IMAGE | LARGE_ADDRESS_AWARE | RELOCS_STRIPPED
    put_u16_at(&mut out, 0x96, 0x0023);

    // Optional header.
    let opt = 0x98;
    put_u16_at(&mut out, opt, 0x20B); // PE32+
    out[opt + 2] = 0; // linker major
    out[opt + 3] = 0; // linker minor
    put_u32_at(&mut out, opt + 4, layout.text_raw_size as u32); // SizeOfCode
    put_u32_at(
        &mut out,
        opt + 8,
        (layout.rodata_raw_size + idata_raw_size) as u32,
    ); // SizeOfInitializedData
    put_u32_at(&mut out, opt + 12, bss.min(u32::MAX as u64) as u32); // SizeOfUninitializedData
    put_u32_at(&mut out, opt + 16, layout.text_rva as u32); // AddressOfEntryPoint
    put_u32_at(&mut out, opt + 20, layout.text_rva as u32); // BaseOfCode
    put_u64_at(&mut out, opt + 24, IMAGE_BASE);
    put_u32_at(&mut out, opt + 32, SECTION_ALIGN as u32);
    put_u32_at(&mut out, opt + 36, FILE_ALIGN as u32);
    put_u16_at(&mut out, opt + 40, 6); // MajorOperatingSystemVersion
    put_u16_at(&mut out, opt + 42, 0); // MinorOperatingSystemVersion
    put_u16_at(&mut out, opt + 44, 0); // MajorImageVersion
    put_u16_at(&mut out, opt + 46, 0); // MinorImageVersion
    put_u16_at(&mut out, opt + 48, 6); // MajorSubsystemVersion
    put_u16_at(&mut out, opt + 50, 0); // MinorSubsystemVersion
    put_u32_at(&mut out, opt + 52, 0); // Win32VersionValue
    put_u32_at(&mut out, opt + 56, size_of_image as u32); // SizeOfImage
    put_u32_at(&mut out, opt + 60, HEADERS_SIZE as u32); // SizeOfHeaders
    put_u32_at(&mut out, opt + 64, 0); // CheckSum (not required for user images)
    put_u16_at(&mut out, opt + 68, 3); // IMAGE_SUBSYSTEM_WINDOWS_CUI
    put_u16_at(&mut out, opt + 70, 0x8100); // NX_COMPAT | TERMINAL_SERVER_AWARE
    put_u64_at(&mut out, opt + 72, 0x10_0000); // SizeOfStackReserve
    put_u64_at(&mut out, opt + 80, 0x1000); // SizeOfStackCommit
    put_u64_at(&mut out, opt + 88, 0x10_0000); // SizeOfHeapReserve
    put_u64_at(&mut out, opt + 96, 0x1000); // SizeOfHeapCommit
    put_u32_at(&mut out, opt + 104, 0); // LoaderFlags
    put_u32_at(&mut out, opt + 108, 16); // NumberOfRvaAndSizes

    // Data directories: import table (index 1) and IAT (index 12).
    put_u32_at(&mut out, opt + 112 + 8, layout.idata_rva as u32);
    put_u32_at(&mut out, opt + 112 + 12, 40); // two descriptors
    put_u32_at(&mut out, opt + 112 + 96, iat as u32);
    put_u32_at(&mut out, opt + 112 + 100, (8 * count) as u32);

    // Section headers.
    let sections = opt + OPTIONAL_HEADER_SIZE as usize;
    section_header(
        &mut out,
        sections,
        b".text\0\0\0",
        text.len() as u32,
        layout.text_rva as u32,
        layout.text_raw_size as u32,
        layout.text_raw as u32,
        0x6000_0020, // CODE | EXECUTE | READ
    );
    section_header(
        &mut out,
        sections + 40,
        b".rdata\0\0",
        rodata.len() as u32,
        layout.rodata_rva as u32,
        layout.rodata_raw_size as u32,
        layout.rodata_raw as u32,
        0x4000_0040, // INITIALIZED_DATA | READ
    );
    section_header(
        &mut out,
        sections + 80,
        b".bss\0\0\0\0",
        bss.min(u32::MAX as u64) as u32,
        layout.bss_rva as u32,
        0,
        0,
        0xC000_0080, // UNINITIALIZED_DATA | READ | WRITE
    );
    section_header(
        &mut out,
        sections + 120,
        b".idata\0\0",
        idata_len as u32,
        layout.idata_rva as u32,
        idata_raw_size as u32,
        layout.idata_raw as u32,
        0xC000_0040, // INITIALIZED_DATA | READ | WRITE
    );

    // Section contents.
    out.extend_from_slice(text);
    out.resize(layout.text_raw as usize + layout.text_raw_size as usize, 0);
    out.extend_from_slice(rodata);
    out.resize(
        layout.rodata_raw as usize + layout.rodata_raw_size as usize,
        0,
    );
    out.extend_from_slice(&idata);
    out.resize(end_raw as usize, 0);
    debug_assert_eq!(out.len(), end_raw as usize);
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn section_header(
    out: &mut [u8],
    at: usize,
    name: &[u8; 8],
    virtual_size: u32,
    virtual_address: u32,
    raw_size: u32,
    raw_pointer: u32,
    characteristics: u32,
) {
    out[at..at + 8].copy_from_slice(name);
    put_u32_at(out, at + 8, virtual_size);
    put_u32_at(out, at + 12, virtual_address);
    put_u32_at(out, at + 16, raw_size);
    put_u32_at(out, at + 20, raw_pointer);
    put_u32_at(out, at + 24, 0); // PointerToRelocations
    put_u32_at(out, at + 28, 0); // PointerToLinenumbers
    put_u16_at(out, at + 32, 0); // NumberOfRelocations
    put_u16_at(out, at + 34, 0); // NumberOfLinenumbers
    put_u32_at(out, at + 36, characteristics);
}

fn build_import_table(layout: &Layout, imports: &[String]) -> Result<Vec<u8>, String> {
    let count = imports.len();
    let mut out = Vec::with_capacity(import_table_len(count) as usize);

    // Descriptor (patched below), then the null descriptor.
    out.resize(40, 0);
    let ilt_off = 40u64;
    let iat_off = ilt_off + 8 * (count as u64 + 1);
    out.resize((iat_off + 8 * (count as u64 + 1)) as usize, 0);

    // Hint/name entries.
    let mut name_rvas = Vec::with_capacity(count);
    for name in imports {
        while out.len() % 2 != 0 {
            out.push(0);
        }
        name_rvas.push(layout.idata_rva + out.len() as u64);
        out.extend_from_slice(&0u16.to_le_bytes()); // Hint
        out.extend_from_slice(name.as_bytes());
        out.push(0);
    }
    while out.len() % 2 != 0 {
        out.push(0);
    }
    let dll_rva = layout.idata_rva + out.len() as u64;
    out.extend_from_slice(b"kernel32.dll\0");
    while out.len() % 2 != 0 {
        out.push(0);
    }

    let rva = |value: u64| -> Result<u32, String> {
        u32::try_from(value).map_err(|_| "the Windows import table is too large".to_string())
    };
    let ilt_rva = rva(layout.idata_rva + ilt_off)?;
    let iat_rva = rva(layout.idata_rva + iat_off)?;
    let name_rva = rva(dll_rva)?;

    // Import Directory Table entry.
    put_u32_at(&mut out, 0, ilt_rva); // OriginalFirstThunk
    put_u32_at(&mut out, 4, 0); // TimeDateStamp
    put_u32_at(&mut out, 8, 0); // ForwarderChain
    put_u32_at(&mut out, 12, name_rva);
    put_u32_at(&mut out, 16, iat_rva); // FirstThunk

    for (index, name_rva) in name_rvas.iter().enumerate() {
        let entry = rva(*name_rva)?;
        put_u32_at(&mut out, (ilt_off as usize) + index * 8, entry);
        put_u32_at(&mut out, (ilt_off as usize) + index * 8 + 4, 0);
        put_u32_at(&mut out, (iat_off as usize) + index * 8, entry);
        put_u32_at(&mut out, (iat_off as usize) + index * 8 + 4, 0);
    }

    Ok(out)
}

fn put_u16_at(out: &mut [u8], at: usize, value: u16) {
    out[at..at + 2].copy_from_slice(&value.to_le_bytes());
}

fn put_u32_at(out: &mut [u8], at: usize, value: u32) {
    out[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn put_u64_at(out: &mut [u8], at: usize, value: u64) {
    out[at..at + 8].copy_from_slice(&value.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn imports() -> Vec<String> {
        vec![
            "GetStdHandle".to_string(),
            "WriteFile".to_string(),
            "ExitProcess".to_string(),
        ]
    }

    #[test]
    fn writes_a_pe32_plus_header() {
        let bytes = build(&[0x90, 0xC3], b"hi", 32, &imports()).unwrap();
        assert_eq!(&bytes[0..2], b"MZ");
        let pe = u32::from_le_bytes(bytes[0x3C..0x40].try_into().unwrap()) as usize;
        assert_eq!(&bytes[pe..pe + 4], b"PE\0\0");
        assert_eq!(
            u16::from_le_bytes(bytes[pe + 4..pe + 6].try_into().unwrap()),
            0x8664
        );
        assert_eq!(
            u16::from_le_bytes(bytes[pe + 6..pe + 8].try_into().unwrap()),
            SECTION_COUNT
        );
        let opt = pe + 24;
        assert_eq!(
            u16::from_le_bytes(bytes[opt..opt + 2].try_into().unwrap()),
            0x20B
        );
        assert_eq!(
            u64::from_le_bytes(bytes[opt + 24..opt + 32].try_into().unwrap()),
            IMAGE_BASE
        );
    }

    #[test]
    fn sections_are_aligned_and_ordered() {
        let bytes = build(&[0x90; 0x300], &[0xAA; 0x500], 0x1234, &imports()).unwrap();
        let pe = u32::from_le_bytes(bytes[0x3C..0x40].try_into().unwrap()) as usize;
        let sections = pe + 24 + OPTIONAL_HEADER_SIZE as usize;
        let text_rva = u32::from_le_bytes(bytes[sections + 12..sections + 16].try_into().unwrap());
        let text_size = u32::from_le_bytes(bytes[sections + 16..sections + 20].try_into().unwrap());
        let rdata_rva = u32::from_le_bytes(bytes[sections + 52..sections + 56].try_into().unwrap());
        let bss_rva = u32::from_le_bytes(bytes[sections + 92..sections + 96].try_into().unwrap());
        let idata_rva =
            u32::from_le_bytes(bytes[sections + 132..sections + 136].try_into().unwrap());
        assert_eq!(text_rva, TEXT_RVA as u32);
        assert_eq!(text_size % FILE_ALIGN as u32, 0);
        assert!(rdata_rva >= text_rva + 0x300);
        assert!(bss_rva >= rdata_rva + 0x500);
        assert!(idata_rva >= bss_rva + 0x1234);
        assert_eq!(idata_rva % SECTION_ALIGN as u32, 0);
    }

    #[test]
    fn import_table_names_every_function() {
        let bytes = build(&[0xC3], b"", 0, &imports()).unwrap();
        for name in imports() {
            let needle = name.as_bytes();
            assert!(
                bytes.windows(needle.len()).any(|window| window == needle),
                "missing import {name}"
            );
        }
        assert!(bytes
            .windows(b"kernel32.dll".len())
            .any(|window| window == b"kernel32.dll"));
    }

    #[test]
    fn rejects_bad_import_names() {
        let err = build(&[0xC3], b"", 0, &["bad name".to_string()]).unwrap_err();
        assert!(err.contains("import name"));
    }
}
