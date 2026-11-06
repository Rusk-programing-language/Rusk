//! A tiny AArch64 machine-code emitter — the "own backend" part.
//!
//! Not an assembler for humans: just the instructions the Rusk code generator
//! needs, encoded by hand as fixed-width 32-bit words. No libc, no linker, no
//! relocations. Branches and absolute string addresses are patched once the
//! whole text section is known.
//!
//! Target: AArch64 Linux, little endian, `svc #0` syscalls.

use std::collections::HashMap;

// General purpose registers. 31 is `xzr` in most contexts and `sp` in
// add/sub-immediate encodings.
pub const X0: u8 = 0;
pub const X1: u8 = 1;
pub const X2: u8 = 2;
pub const X3: u8 = 3;
pub const X4: u8 = 4;
pub const X5: u8 = 5;
pub const X6: u8 = 6;
pub const X7: u8 = 7;
pub const X8: u8 = 8;
pub const X9: u8 = 9;
pub const X10: u8 = 10;
pub const X11: u8 = 11;
pub const X12: u8 = 12;
pub const X13: u8 = 13;
pub const X14: u8 = 14;
pub const X15: u8 = 15;
pub const X16: u8 = 16;
pub const X17: u8 = 17;
pub const X18: u8 = 18;
pub const X19: u8 = 19;
pub const X20: u8 = 20;
pub const X21: u8 = 21;
pub const X22: u8 = 22;
pub const X23: u8 = 23;
pub const X24: u8 = 24;
pub const X25: u8 = 25;
pub const X26: u8 = 26;
pub const X27: u8 = 27;
pub const X28: u8 = 28;
pub const X29: u8 = 29;
pub const X30: u8 = 30;
pub const XZR: u8 = 31;
pub const SP: u8 = 31;

// Condition codes for `b.cond` / `cset`.
pub const CC_EQ: u8 = 0;
pub const CC_NE: u8 = 1;
pub const CC_GE: u8 = 10;
pub const CC_LT: u8 = 11;
pub const CC_GT: u8 = 12;
pub const CC_LE: u8 = 13;

/// Where the loadable image is mapped.
pub const IMAGE_BASE: u64 = 0x40_0000;
/// Size of the ELF header plus one program header.
pub const TEXT_OFFSET: u64 = 0x78;

/// `SYS_write`
pub const SYS_WRITE: u64 = 64;
/// `SYS_exit`
pub const SYS_EXIT: u64 = 93;

enum Target {
    Label(String),
    /// Absolute address of an interned string, spread over four instructions.
    Str(usize),
}

struct Fixup {
    at: usize,
    target: Target,
}

pub struct Asm {
    code: Vec<u32>,
    labels: HashMap<String, u32>,
    fixups: Vec<Fixup>,
    strings: Vec<Vec<u8>>,
}

impl Asm {
    pub fn new() -> Self {
        Asm {
            code: Vec::with_capacity(256),
            labels: HashMap::new(),
            fixups: Vec::new(),
            strings: Vec::new(),
        }
    }

    pub fn label(&mut self, name: &str) {
        self.labels.insert(name.to_string(), self.code.len() as u32);
    }

    /// Intern a piece of text as `[len: u64][bytes...]`, so a string *value* is
    /// just a pointer to its length.
    pub fn add_string(&mut self, s: &str) -> usize {
        self.strings.push(s.as_bytes().to_vec());
        self.strings.len() - 1
    }

    fn emit(&mut self, word: u32) {
        self.code.push(word);
    }

    /// Byte offset of the next instruction.
    fn at(&self) -> usize {
        self.code.len() * 4
    }

    // ---- immediate moves -------------------------------------------------

    fn movz(&mut self, rd: u8, imm: u16, hw: u8) {
        self.emit(0xD280_0000 | ((hw as u32) << 21) | (((imm as u32) & 0xFFFF) << 5) | rd as u32);
    }

    fn movk(&mut self, rd: u8, imm: u16, hw: u8) {
        self.emit(0xF280_0000 | ((hw as u32) << 21) | (((imm as u32) & 0xFFFF) << 5) | rd as u32);
    }

    /// Load any 64-bit constant: one `movz` plus up to three `movk`.
    pub fn mov_imm64(&mut self, rd: u8, value: u64) {
        for hw in 0..4u8 {
            let chunk = ((value >> (16 * hw as u32)) & 0xFFFF) as u16;
            if hw == 0 {
                self.movz(rd, chunk, 0);
            } else if chunk != 0 {
                self.movk(rd, chunk, hw);
            }
        }
    }

    /// Load the absolute address of an interned string, patched at finish.
    pub fn mov_abs(&mut self, rd: u8, string_id: usize) {
        let at = self.at();
        self.movz(rd, 0, 0);
        self.movk(rd, 0, 1);
        self.movk(rd, 0, 2);
        self.movk(rd, 0, 3);
        self.fixups.push(Fixup {
            at,
            target: Target::Str(string_id),
        });
    }

    // ---- register moves --------------------------------------------------

    /// `mov rd, rm` (an ORR with the zero register).
    pub fn mov_reg_reg(&mut self, rd: u8, rm: u8) {
        self.emit(0xAA00_03E0 | ((rm as u32) << 16) | rd as u32);
    }

    pub fn movz_reg(&mut self, rd: u8, imm: u16) {
        self.movz(rd, imm, 0);
    }

    // ---- memory ----------------------------------------------------------

    /// `ldr rd, [rn, #disp]` — disp must be a multiple of 8.
    pub fn ldr(&mut self, rd: u8, rn: u8, disp: u32) {
        debug_assert!(disp % 8 == 0 && disp / 8 < 4096);
        self.emit(0xF940_0000 | ((disp / 8) << 10) | ((rn as u32) << 5) | rd as u32);
    }

    /// `str rt, [rn, #disp]` — disp must be a multiple of 8.
    pub fn str(&mut self, rt: u8, rn: u8, disp: u32) {
        debug_assert!(disp % 8 == 0 && disp / 8 < 4096);
        self.emit(0xF900_0000 | ((disp / 8) << 10) | ((rn as u32) << 5) | rt as u32);
    }

    /// `ldrb wt, [rn, #disp]`
    pub fn ldrb(&mut self, rt: u8, rn: u8, disp: u32) {
        debug_assert!(disp < 4096);
        self.emit(0x3940_0000 | (disp << 10) | ((rn as u32) << 5) | rt as u32);
    }

    /// `strb wt, [rn, #disp]`
    pub fn strb(&mut self, rt: u8, rn: u8, disp: u32) {
        debug_assert!(disp < 4096);
        self.emit(0x3900_0000 | (disp << 10) | ((rn as u32) << 5) | rt as u32);
    }

    // ---- arithmetic ------------------------------------------------------

    pub fn add_reg_reg(&mut self, rd: u8, rn: u8, rm: u8) {
        self.emit(0x8B00_0000 | ((rm as u32) << 16) | ((rn as u32) << 5) | rd as u32);
    }

    pub fn sub_reg_reg(&mut self, rd: u8, rn: u8, rm: u8) {
        self.emit(0xCB00_0000 | ((rm as u32) << 16) | ((rn as u32) << 5) | rd as u32);
    }

    pub fn mul_reg_reg(&mut self, rd: u8, rn: u8, rm: u8) {
        self.emit(0x9B00_0000 | ((rm as u32) << 16) | (31 << 10) | ((rn as u32) << 5) | rd as u32);
    }

    pub fn sdiv_reg_reg(&mut self, rd: u8, rn: u8, rm: u8) {
        self.emit(0x9AC0_0C00 | ((rm as u32) << 16) | ((rn as u32) << 5) | rd as u32);
    }

    pub fn udiv_reg_reg(&mut self, rd: u8, rn: u8, rm: u8) {
        self.emit(0x9AC0_0800 | ((rm as u32) << 16) | ((rn as u32) << 5) | rd as u32);
    }

    /// `msub rd, rn, rm, ra` computes `ra - rn * rm`.
    pub fn msub_reg_reg(&mut self, rd: u8, rn: u8, rm: u8, ra: u8) {
        self.emit(
            0x9B00_8000
                | ((rm as u32) << 16)
                | ((ra as u32) << 10)
                | ((rn as u32) << 5)
                | rd as u32,
        );
    }

    pub fn neg_reg(&mut self, rd: u8, rm: u8) {
        self.emit(0xCB00_0000 | ((rm as u32) << 16) | (31 << 5) | rd as u32);
    }

    /// `cmp rn, rm` (a `subs` into xzr).
    pub fn cmp_reg_reg(&mut self, rn: u8, rm: u8) {
        self.emit(0xEB00_0000 | ((rm as u32) << 16) | ((rn as u32) << 5) | 31);
    }

    pub fn add_imm(&mut self, rd: u8, rn: u8, imm: u32) {
        debug_assert!(imm < 4096);
        self.emit(0x9100_0000 | (imm << 10) | ((rn as u32) << 5) | rd as u32);
    }

    pub fn sub_imm(&mut self, rd: u8, rn: u8, imm: u32) {
        debug_assert!(imm < 4096);
        self.emit(0xD100_0000 | (imm << 10) | ((rn as u32) << 5) | rd as u32);
    }

    /// `cset rd, cond` — 1 if the condition holds, else 0.
    pub fn cset(&mut self, rd: u8, cond: u8) {
        self.emit(0x9A9F_07E0 | (((cond ^ 1) as u32) << 12) | rd as u32);
    }

    // ---- stack -----------------------------------------------------------

    pub fn push_reg(&mut self, reg: u8) {
        self.sub_imm(SP, SP, 16);
        self.str(reg, SP, 0);
    }

    pub fn pop_reg(&mut self, reg: u8) {
        self.ldr(reg, SP, 0);
        self.add_imm(SP, SP, 16);
    }

    // ---- control flow ----------------------------------------------------

    fn branch(&mut self, base: u32, label: &str) {
        let at = self.at();
        self.emit(base);
        self.fixups.push(Fixup {
            at,
            target: Target::Label(label.to_string()),
        });
    }

    pub fn b_label(&mut self, label: &str) {
        self.branch(0x1400_0000, label);
    }

    pub fn bl_label(&mut self, label: &str) {
        self.branch(0x9400_0000, label);
    }

    pub fn bcond_label(&mut self, cond: u8, label: &str) {
        self.branch(0x5400_0000 | cond as u32, label);
    }

    pub fn svc(&mut self) {
        self.emit(0xD400_0001);
    }

    pub fn ret(&mut self) {
        self.emit(0xD65F_03C0);
    }

    // ---- finishing -------------------------------------------------------

    /// Resolve every fixup and return `(text, rodata)`.
    pub fn finish(mut self) -> (Vec<u8>, Vec<u8>) {
        let mut rodata = Vec::new();
        let mut offsets = Vec::with_capacity(self.strings.len());
        for s in &self.strings {
            offsets.push(rodata.len());
            rodata.extend_from_slice(&(s.len() as u64).to_le_bytes());
            rodata.extend_from_slice(s);
        }

        let string_base = IMAGE_BASE + TEXT_OFFSET + self.code.len() as u64 * 4;

        for f in &self.fixups {
            match &f.target {
                Target::Label(name) => {
                    let target = *self
                        .labels
                        .get(name)
                        .unwrap_or_else(|| panic!("internal: branch to unknown label '{name}'"))
                        as i64
                        * 4;
                    let delta = target - f.at as i64;
                    debug_assert_eq!(delta % 4, 0);
                    let imm = delta / 4;
                    let word = self.code[f.at / 4];
                    let patched = match word & 0x7C00_0000 {
                        0x1400_0000 | 0x9400_0000 => {
                            (word & 0xFC00_0000) | ((imm as u32) & 0x03FF_FFFF)
                        }
                        _ => {
                            // b.cond: imm19 in bits 5..23
                            (word & !(0x7FFFF << 5)) | (((imm as u32) & 0x7FFFF) << 5)
                        }
                    };
                    self.code[f.at / 4] = patched;
                }
                Target::Str(id) => {
                    let addr = string_base + offsets[*id] as u64;
                    for hw in 0..4u32 {
                        let word = self.code[f.at / 4 + hw as usize];
                        let chunk = ((addr >> (16 * hw)) & 0xFFFF) as u32;
                        self.code[f.at / 4 + hw as usize] = (word & !(0xFFFF << 5)) | (chunk << 5);
                    }
                }
            }
        }

        let mut text = Vec::with_capacity(self.code.len() * 4);
        for word in self.code {
            text.extend_from_slice(&word.to_le_bytes());
        }
        (text, rodata)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(a: Asm) -> Vec<u32> {
        let (text, _) = a.finish();
        text.chunks(4)
            .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
            .collect()
    }

    #[test]
    fn encodes_movz() {
        let mut a = Asm::new();
        a.movz_reg(X0, 1);
        assert_eq!(words(a), vec![0xD280_0020]);
    }

    #[test]
    fn encodes_mov_reg_reg() {
        let mut a = Asm::new();
        a.mov_reg_reg(X1, X0);
        assert_eq!(words(a), vec![0xAA00_03E1]);
    }

    #[test]
    fn encodes_prologue_and_syscall() {
        let mut a = Asm::new();
        a.push_reg(X19);
        a.svc();
        a.ret();
        assert_eq!(
            words(a),
            vec![0xD100_43FF, 0xF900_03F3, 0xD400_0001, 0xD65F_03C0]
        );
    }

    #[test]
    fn encodes_cset() {
        let mut a = Asm::new();
        a.cset(X0, CC_EQ);
        assert_eq!(words(a), vec![0x9A9F_17E0]);
    }

    #[test]
    fn forwards_branch_is_patched() {
        let mut a = Asm::new();
        a.b_label("end");
        a.ret();
        a.label("end");
        // Branch from word 0 to word 2: offset 2.
        assert_eq!(words(a)[0] & 0x03FF_FFFF, 2);
    }

    #[test]
    fn string_addresses_land_after_the_code() {
        let mut a = Asm::new();
        let id = a.add_string("hi");
        a.mov_abs(X0, id);
        let (text, rodata) = a.finish();
        assert_eq!(rodata.len(), 8 + 2);
        let expected = IMAGE_BASE + TEXT_OFFSET + text.len() as u64;
        let low = ((u32::from_le_bytes(text[0..4].try_into().unwrap()) >> 5) & 0xFFFF) as u64;
        assert_eq!(low, expected & 0xFFFF);
    }
}
