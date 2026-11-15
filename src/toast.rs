//! The `.tst` "toast" container: a whole program that has already been baked.
//!
//! A toast file is not a linker or an object format. It stores the finished
//! machine code, read-only data, and zeroed-data size for one or more targets,
//! exactly as the code generator produced them. `rusk build` can wrap an image
//! back into a native executable, and `rusk run` can run the host image without
//! touching the source.
//!
//! Layout, all integers little endian:
//!
//! ```text
//! magic           8 bytes   "RTOAST1\n"
//! version         u16       1
//! flags           u16       0
//! image count     u32
//! metadata length u32
//! reserved        u32       0
//! metadata        UTF-8 lines, "key: value\n"
//! images          repeated image records
//! checksum        u64       FNV-1a over every preceding byte
//! ```
//!
//! An image record is:
//!
//! ```text
//! target          u8        1 = aarch64, 2 = x86_64, 3 = windows-x86_64
//! flags           u8        0
//! reserved        u16       0
//! text length     u32
//! rodata length   u32
//! bss size        u64
//! imports length  u32
//! reserved        u32       0
//! text            bytes
//! rodata          bytes
//! imports         UTF-8 names separated by '\n' (empty for Linux)
//! ```
//!
//! The checksum is an integrity check, not a signature. It catches truncation
//! and accidental corruption; it does not prove where a file came from.

use crate::backend::Target;
use std::fs;
use std::path::Path;

/// File magic. The trailing newline makes a toast file easy to recognise in a
/// terminal and keeps the magic at exactly eight bytes.
pub const MAGIC: &[u8; 8] = b"RTOAST1\n";
/// The container version understood by this compiler.
pub const VERSION: u16 = 1;

const HEADER_LEN: usize = 24;
const IMAGE_HEADER_LEN: usize = 28;
const CHECKSUM_LEN: usize = 8;

/// A sane upper bound for a metadata block.
const MAX_METADATA: usize = 1 << 20;
/// A sane upper bound for one code or data section.
const MAX_SECTION: usize = 256 << 20;
/// A sane upper bound for the zeroed data area. It is never allocated by the
/// compiler, only added to the image size.
const MAX_BSS: u64 = 16 << 30;
const MAX_IMAGES: usize = 8;

/// One compiled image for one target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub target: Target,
    pub text: Vec<u8>,
    pub rodata: Vec<u8>,
    pub bss: u64,
    /// Windows functions imported from `kernel32.dll`, in IAT order. Empty for
    /// Linux images.
    pub imports: Vec<String>,
}

/// A decoded toast file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toast {
    pub metadata: String,
    pub images: Vec<Image>,
}

/// Read and validate a toast file.
pub fn read(path: &Path) -> Result<Toast, String> {
    Toast::read(path)
}

impl Toast {
    /// Assemble a toast from already-compiled images. The source name is kept
    /// for humans; it is not used when running or building.
    pub fn new(source: &str, compiler: &str, images: Vec<Image>) -> Self {
        let source = one_line(source);
        Toast {
            metadata: format!("source: {source}\ncompiler: {compiler}\n"),
            images,
        }
    }

    /// The image for a target, if this toast contains one.
    pub fn image_for(&self, target: Target) -> Option<&Image> {
        self.images.iter().find(|image| image.target == target)
    }

    /// The targets present, in file order.
    pub fn targets(&self) -> impl Iterator<Item = Target> + '_ {
        self.images.iter().map(|image| image.target)
    }

    /// Serialise the whole container.
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        if self.images.is_empty() {
            return Err("a toast needs at least one compiled image".to_string());
        }
        if self.images.len() > MAX_IMAGES {
            return Err(format!("a toast can hold at most {MAX_IMAGES} images"));
        }
        let mut seen = Vec::with_capacity(self.images.len());
        for image in &self.images {
            if seen.contains(&image.target) {
                return Err(format!(
                    "a toast can only hold one {} image",
                    image.target.name()
                ));
            }
            seen.push(image.target);
            if image.text.len() > MAX_SECTION {
                return Err("this image's code is too large for a toast".to_string());
            }
            if image.rodata.len() > MAX_SECTION {
                return Err("this image's data is too large for a toast".to_string());
            }
            if image.bss > MAX_BSS {
                return Err("this image asks for too much zeroed memory".to_string());
            }
            for name in &image.imports {
                if name.is_empty() || name.contains('\n') || name.contains('\0') {
                    return Err(format!(
                        "'{name}' is not a usable import name for the {} image",
                        image.target.name()
                    ));
                }
            }
        }
        let metadata = self.metadata.as_bytes();
        if metadata.len() > MAX_METADATA {
            return Err("this toast's metadata is too large".to_string());
        }

        let mut out = Vec::with_capacity(
            HEADER_LEN
                + metadata.len()
                + self
                    .images
                    .iter()
                    .map(|image| {
                        IMAGE_HEADER_LEN
                            + image.text.len()
                            + image.rodata.len()
                            + image
                                .imports
                                .iter()
                                .map(|name| name.len() + 1)
                                .sum::<usize>()
                    })
                    .sum::<usize>()
                + CHECKSUM_LEN,
        );

        out.extend_from_slice(MAGIC);
        put_u16(&mut out, VERSION);
        put_u16(&mut out, 0); // flags
        put_u32(&mut out, self.images.len() as u32);
        put_u32(&mut out, metadata.len() as u32);
        put_u32(&mut out, 0); // reserved
        out.extend_from_slice(metadata);

        for image in &self.images {
            out.push(target_id(image.target));
            out.push(0); // flags
            put_u16(&mut out, 0); // reserved
            put_u32(&mut out, image.text.len() as u32);
            put_u32(&mut out, image.rodata.len() as u32);
            put_u64(&mut out, image.bss);
            let imports_len: usize = image.imports.iter().map(|name| name.len() + 1).sum();
            put_u32(&mut out, imports_len as u32);
            put_u32(&mut out, 0); // reserved
            out.extend_from_slice(&image.text);
            out.extend_from_slice(&image.rodata);
            for name in &image.imports {
                out.extend_from_slice(name.as_bytes());
                out.push(b'\n');
            }
        }

        let sum = checksum(&out);
        put_u64(&mut out, sum);
        Ok(out)
    }

    /// Parse and validate a whole toast file.
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() < HEADER_LEN + CHECKSUM_LEN {
            return Err("this file is too short to be a toast".to_string());
        }
        if &bytes[..8] != MAGIC {
            return Err("this is not a toast file (the magic doesn't match)".to_string());
        }
        let version = u16::from_le_bytes(bytes[8..10].try_into().unwrap());
        if version != VERSION {
            return Err(format!(
                "this toast uses format version {version}, but this rusk understands version {VERSION}"
            ));
        }
        let flags = u16::from_le_bytes(bytes[10..12].try_into().unwrap());
        if flags != 0 {
            return Err(format!("this toast uses unknown flags ({flags:#06x})"));
        }
        let count = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
        let metadata_len = u32::from_le_bytes(bytes[16..20].try_into().unwrap()) as usize;
        let reserved = u32::from_le_bytes(bytes[20..24].try_into().unwrap());
        if reserved != 0 {
            return Err("this toast has a non-zero reserved header field".to_string());
        }
        if count == 0 {
            return Err("this toast contains no images".to_string());
        }
        if count > MAX_IMAGES {
            return Err(format!("this toast contains too many images ({count})"));
        }
        if metadata_len > MAX_METADATA {
            return Err("this toast's metadata length is not believable".to_string());
        }

        let body_end = bytes.len() - CHECKSUM_LEN;
        let mut cursor = HEADER_LEN;
        let metadata_end = cursor
            .checked_add(metadata_len)
            .ok_or_else(|| "this toast's metadata length overflows".to_string())?;
        if metadata_end > body_end {
            return Err("this toast is truncated inside its metadata".to_string());
        }
        let metadata = std::str::from_utf8(&bytes[cursor..metadata_end])
            .map_err(|_| "this toast's metadata is not valid UTF-8".to_string())?
            .to_string();
        cursor = metadata_end;

        let mut images = Vec::with_capacity(count);
        let mut seen = Vec::with_capacity(count);
        for _ in 0..count {
            let header_end = cursor
                .checked_add(IMAGE_HEADER_LEN)
                .ok_or_else(|| "this toast's image header overflows".to_string())?;
            if header_end > body_end {
                return Err("this toast is truncated inside an image header".to_string());
            }
            let id = bytes[cursor];
            let target = target_from_id(id)
                .ok_or_else(|| format!("this toast names an unknown target ({id})"))?;
            let image_flags = bytes[cursor + 1];
            if image_flags != 0 {
                return Err(format!(
                    "the {} image uses unknown flags ({image_flags:#04x})",
                    target.name()
                ));
            }
            if seen.contains(&target) {
                return Err(format!(
                    "this toast contains more than one {} image",
                    target.name()
                ));
            }
            seen.push(target);
            let text_len =
                u32::from_le_bytes(bytes[cursor + 4..cursor + 8].try_into().unwrap()) as usize;
            let rodata_len =
                u32::from_le_bytes(bytes[cursor + 8..cursor + 12].try_into().unwrap()) as usize;
            let bss = u64::from_le_bytes(bytes[cursor + 12..cursor + 20].try_into().unwrap());
            let imports_len =
                u32::from_le_bytes(bytes[cursor + 20..cursor + 24].try_into().unwrap()) as usize;
            let image_reserved =
                u32::from_le_bytes(bytes[cursor + 24..cursor + 28].try_into().unwrap());
            if image_reserved != 0 {
                return Err(format!(
                    "the {} image has a non-zero reserved header field",
                    target.name()
                ));
            }
            if text_len > MAX_SECTION || rodata_len > MAX_SECTION {
                return Err("this toast claims a section that is too large".to_string());
            }
            if imports_len > MAX_SECTION {
                return Err("this toast claims too many imports".to_string());
            }
            if bss > MAX_BSS {
                return Err("this toast claims too much zeroed memory".to_string());
            }
            cursor = header_end;

            let text_end = cursor
                .checked_add(text_len)
                .ok_or_else(|| "this toast's code length overflows".to_string())?;
            let rodata_end = text_end
                .checked_add(rodata_len)
                .ok_or_else(|| "this toast's data length overflows".to_string())?;
            let imports_end = rodata_end
                .checked_add(imports_len)
                .ok_or_else(|| "this toast's import list overflows".to_string())?;
            if imports_end > body_end {
                return Err("this toast is truncated inside an image".to_string());
            }
            let imports = decode_imports(&bytes[rodata_end..imports_end])?;
            images.push(Image {
                target,
                text: bytes[cursor..text_end].to_vec(),
                rodata: bytes[text_end..rodata_end].to_vec(),
                bss,
                imports,
            });
            cursor = imports_end;
        }

        if cursor != body_end {
            return Err("this toast has trailing bytes before its checksum".to_string());
        }
        let expected = u64::from_le_bytes(bytes[body_end..].try_into().unwrap());
        let actual = checksum(&bytes[..body_end]);
        if actual != expected {
            return Err("this toast's checksum doesn't match (it may be corrupt)".to_string());
        }

        Ok(Toast { metadata, images })
    }

    /// Read and validate a toast file.
    pub fn read(path: &Path) -> Result<Self, String> {
        let bytes = fs::read(path).map_err(|e| format!("can't read {}: {e}", path.display()))?;
        Toast::decode(&bytes).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Write a toast file. Unlike an executable, it is ordinary data, so it is
    /// not marked executable.
    pub fn write(&self, path: &Path) -> Result<usize, String> {
        let bytes = self.encode()?;
        fs::write(path, &bytes).map_err(|e| format!("can't write {}: {e}", path.display()))?;
        Ok(bytes.len())
    }
}

fn one_line(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

fn target_id(target: Target) -> u8 {
    match target {
        Target::AArch64 => 1,
        Target::X86_64 => 2,
        Target::WindowsX86_64 => 3,
    }
}

fn target_from_id(id: u8) -> Option<Target> {
    match id {
        1 => Some(Target::AArch64),
        2 => Some(Target::X86_64),
        3 => Some(Target::WindowsX86_64),
        _ => None,
    }
}

/// Decode the newline-separated import names for one image. An empty blob means
/// no imports; otherwise every name ends with a newline.
fn decode_imports(bytes: &[u8]) -> Result<Vec<String>, String> {
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|_| "a toast's import list is not valid UTF-8".to_string())?;
    let body = text
        .strip_suffix('\n')
        .ok_or_else(|| "a toast's import list is missing its final newline".to_string())?;
    let mut names = Vec::new();
    for name in body.split('\n') {
        if name.is_empty() {
            return Err("a toast contains an empty import name".to_string());
        }
        if name.contains('\0') {
            return Err("a toast contains an import name with a NUL byte".to_string());
        }
        names.push(name.to_string());
    }
    Ok(names)
}

fn put_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

/// FNV-1a, 64 bit. Small, dependency-free, and good enough to catch accidental
/// damage. This is not a cryptographic hash.
fn checksum(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in bytes {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Toast {
        Toast::new(
            "hello.rk",
            "1.1.6",
            vec![
                Image {
                    target: Target::AArch64,
                    text: vec![0xAA; 8],
                    rodata: b"arm".to_vec(),
                    bss: 16,
                    imports: Vec::new(),
                },
                Image {
                    target: Target::X86_64,
                    text: vec![0xBB; 12],
                    rodata: b"x86".to_vec(),
                    bss: 32,
                    imports: Vec::new(),
                },
            ],
        )
    }

    fn windows() -> Image {
        Image {
            target: Target::WindowsX86_64,
            text: vec![0x90, 0xC3],
            rodata: b"pe".to_vec(),
            bss: 64,
            imports: vec!["ExitProcess".to_string(), "WriteFile".to_string()],
        }
    }

    #[test]
    fn round_trips_one_and_two_targets() {
        let toast = sample();
        let bytes = toast.encode().unwrap();
        assert_eq!(&bytes[..8], MAGIC);
        assert_eq!(Toast::decode(&bytes).unwrap(), toast);

        let one = Toast::new(
            "hello.rk",
            "1.1.6",
            vec![Image {
                target: Target::X86_64,
                text: vec![0xC3],
                rodata: Vec::new(),
                bss: 0,
                imports: Vec::new(),
            }],
        );
        assert_eq!(Toast::decode(&one.encode().unwrap()).unwrap(), one);
    }

    #[test]
    fn round_trips_windows_imports() {
        let toast = Toast::new("win.rk", "1.1.6", vec![windows()]);
        let bytes = toast.encode().unwrap();
        assert_eq!(Toast::decode(&bytes).unwrap(), toast);
    }

    #[test]
    fn rejects_a_bad_magic() {
        let mut bytes = sample().encode().unwrap();
        bytes[0] = b'X';
        assert!(Toast::decode(&bytes).unwrap_err().contains("not a toast"));
    }

    #[test]
    fn rejects_a_truncated_file() {
        let bytes = sample().encode().unwrap();
        assert!(Toast::decode(&bytes[..bytes.len() - 20]).is_err());
    }

    #[test]
    fn rejects_a_bad_checksum() {
        let mut bytes = sample().encode().unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF;
        assert!(Toast::decode(&bytes).unwrap_err().contains("checksum"));
    }

    #[test]
    fn rejects_trailing_bytes() {
        let mut bytes = sample().encode().unwrap();
        bytes.insert(bytes.len() - CHECKSUM_LEN, 0);
        assert!(Toast::decode(&bytes).unwrap_err().contains("trailing"));
    }

    #[test]
    fn rejects_duplicate_targets() {
        let image = Image {
            target: Target::X86_64,
            text: vec![0xC3],
            rodata: Vec::new(),
            bss: 0,
            imports: Vec::new(),
        };
        let toast = Toast::new("dupe.rk", "1.1.6", vec![image.clone(), image]);
        assert!(toast.encode().unwrap_err().contains("one x86_64"));
    }

    #[test]
    fn finds_the_host_image() {
        let toast = sample();
        assert_eq!(toast.image_for(Target::AArch64).unwrap().bss, 16);
        assert_eq!(toast.image_for(Target::X86_64).unwrap().bss, 32);
    }
}
