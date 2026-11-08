//! A tiny x86-64 machine-code emitter — the second Rusk backend.
//!
//! The Rusk code generator is written against a small virtual register machine
//! with `X0..X30`, `SP`, and `XZR`. This module keeps those virtual registers
//! in a fixed block of zeroed memory and translates each operation into real
//! x86-64 instructions. `SP` is the real process stack pointer, so frames,
//! pushes, calls, and returns behave the way the AArch64 emitter expects.
//!
//! Target: x86-64 Linux, little endian, `syscall`.

use crate::aarch64::{
    CC_EQ, CC_GE, CC_GT, CC_LE, CC_LT, CC_NE, IMAGE_BASE, SP, TEXT_OFFSET, X0, X1, X2, X3, X4, X5,
    X8, XZR,
};
use std::collections::HashMap;

// Physical x86-64 registers used by the emitter.
const RAX: u8 = 0;
const RCX: u8 = 1;
const RDX: u8 = 2;
const RSI: u8 = 6;
const RDI: u8 = 7;
const R8: u8 = 8;
const R9: u8 = 9;
const R10: u8 = 10;

/// Where the virtual registers live. This is inside the zero-filled data area
/// reserved by the code generator (well below the text heap at +0x10000).
const VREG_BASE: u64 = 0x41_8000;

// AArch64 syscall numbers are what the code generator emits. x86-64 uses its
// own numbers, so `mov_imm64(X8, ...)` translates them for us.
const AARCH64_WRITE: u64 = 64;
const AARCH64_EXIT: u64 = 93;
const AARCH64_OPENAT: u64 = 56;
const AARCH64_CLOSE: u64 = 57;
const AARCH64_READ: u64 = 63;
const X86_WRITE: u64 = 1;
const X86_EXIT: u64 = 60;
const X86_OPENAT: u64 = 257;
const X86_CLOSE: u64 = 3;
const X86_READ: u64 = 0;

enum Fixup {
    Label {
        at: usize,
        label: String,
    },
    /// Absolute address of an interned string, patched at finish.
    Str {
        at: usize,
        id: usize,
    },
    /// Absolute address of a code label, patched at finish.
    LabelAddr {
        at: usize,
        label: String,
    },
}

pub struct Asm {
    code: Vec<u8>,
    labels: HashMap<String, usize>,
    fixups: Vec<Fixup>,
    strings: Vec<Vec<u8>>,
}

impl Default for Asm {
    fn default() -> Self {
        Self::new()
    }
}

impl Asm {
    pub fn new() -> Self {
        Asm {
            code: Vec::with_capacity(1024),
            labels: HashMap::new(),
            fixups: Vec::new(),
            strings: Vec::new(),
        }
    }

    pub fn label(&mut self, name: &str) {
        self.labels.insert(name.to_string(), self.code.len());
    }

    /// Intern a piece of text as `[len: u64][bytes...]`, so a string *value* is
    /// just a pointer to its length.
    pub fn add_string(&mut self, s: &str) -> usize {
        self.strings.push(s.as_bytes().to_vec());
        self.strings.len() - 1
    }

    fn u8(&mut self, byte: u8) {
        self.code.push(byte);
    }

    fn u32(&mut self, value: u32) {
        self.code.extend_from_slice(&value.to_le_bytes());
    }

    fn u64(&mut self, value: u64) {
        self.code.extend_from_slice(&value.to_le_bytes());
    }

    fn patch_u32(&mut self, at: usize, value: u32) {
        self.code[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn patch_u64(&mut self, at: usize, value: u64) {
        self.code[at..at + 8].copy_from_slice(&value.to_le_bytes());
    }

    // ---- virtual register storage ---------------------------------------

    fn load_abs(&mut self, phys: u8, addr: u64) {
        let rex = 0x48 | (((phys >> 3) & 1) << 2);
        self.u8(rex);
        self.u8(0x8B);
        self.u8(0x04 | ((phys & 7) << 3));
        self.u8(0x25);
        self.u32(addr as u32);
    }

    fn store_abs(&mut self, phys: u8, addr: u64) {
        let rex = 0x48 | (((phys >> 3) & 1) << 2);
        self.u8(rex);
        self.u8(0x89);
        self.u8(0x04 | ((phys & 7) << 3));
        self.u8(0x25);
        self.u32(addr as u32);
    }

    fn vreg_addr(reg: u8) -> u64 {
        VREG_BASE + (reg as u64) * 8
    }

    fn load_vreg(&mut self, phys: u8, reg: u8) {
        if reg == XZR {
            self.mov_phys_imm64(phys, 0);
        } else {
            self.load_abs(phys, Self::vreg_addr(reg));
        }
    }

    fn store_vreg(&mut self, reg: u8, phys: u8) {
        if reg != XZR {
            self.store_abs(phys, Self::vreg_addr(reg));
        }
    }

    fn mov_phys_imm64(&mut self, phys: u8, value: u64) {
        self.u8(0x48 | ((phys >> 3) & 1));
        self.u8(0xB8 + (phys & 7));
        self.u64(value);
    }

    fn mov_phys_reg(&mut self, dst: u8, src: u8) {
        let rex = 0x48 | (((src >> 3) & 1) << 2) | ((dst >> 3) & 1);
        self.u8(rex);
        self.u8(0x89);
        self.u8(0xC0 | ((src & 7) << 3) | (dst & 7));
    }

    fn mov_phys_from_sp(&mut self, phys: u8) {
        self.mov_phys_reg(phys, 4); // RSP = 4
    }

    fn mov_sp_from_phys(&mut self, phys: u8) {
        self.mov_phys_reg(4, phys);
    }

    /// Load `reg`, treating 31 as `SP`, into a physical register.
    fn load_sp_or_vreg(&mut self, phys: u8, reg: u8) {
        if reg == SP {
            self.mov_phys_from_sp(phys);
        } else {
            self.load_vreg(phys, reg);
        }
    }

    /// Store a physical register into `reg`, treating 31 as `SP`.
    fn store_sp_or_vreg(&mut self, reg: u8, phys: u8) {
        if reg == SP {
            self.mov_sp_from_phys(phys);
        } else {
            self.store_vreg(reg, phys);
        }
    }

    // ---- memory helpers --------------------------------------------------

    fn load_mem_sp(&mut self, phys: u8, disp: u32) {
        let rex = 0x48 | (((phys >> 3) & 1) << 2);
        self.u8(rex);
        self.u8(0x8B);
        self.u8(0x84 | ((phys & 7) << 3));
        self.u8(0x24);
        self.u32(disp);
    }

    fn store_mem_sp(&mut self, phys: u8, disp: u32) {
        let rex = 0x48 | (((phys >> 3) & 1) << 2);
        self.u8(rex);
        self.u8(0x89);
        self.u8(0x84 | ((phys & 7) << 3));
        self.u8(0x24);
        self.u32(disp);
    }

    fn load_mem_rcx(&mut self, phys: u8, disp: u32) {
        let rex = 0x48 | (((phys >> 3) & 1) << 2);
        self.u8(rex);
        self.u8(0x8B);
        self.u8(0x81 | ((phys & 7) << 3));
        self.u32(disp);
    }

    fn store_mem_rcx(&mut self, phys: u8, disp: u32) {
        let rex = 0x48 | (((phys >> 3) & 1) << 2);
        self.u8(rex);
        self.u8(0x89);
        self.u8(0x81 | ((phys & 7) << 3));
        self.u32(disp);
    }

    fn load_byte_sp(&mut self, phys: u8, disp: u32) {
        let rex = 0x48 | (((phys >> 3) & 1) << 2);
        self.u8(rex);
        self.u8(0x0F);
        self.u8(0xB6);
        self.u8(0x84 | ((phys & 7) << 3));
        self.u8(0x24);
        self.u32(disp);
    }

    fn store_byte_sp(&mut self, phys: u8, disp: u32) {
        let rex = 0x48 | (((phys >> 3) & 1) << 2);
        self.u8(rex);
        self.u8(0x88);
        self.u8(0x84 | ((phys & 7) << 3));
        self.u8(0x24);
        self.u32(disp);
    }

    fn load_byte_rcx(&mut self, phys: u8, disp: u32) {
        let rex = 0x48 | (((phys >> 3) & 1) << 2);
        self.u8(rex);
        self.u8(0x0F);
        self.u8(0xB6);
        self.u8(0x81 | ((phys & 7) << 3));
        self.u32(disp);
    }

    fn store_byte_rcx(&mut self, phys: u8, disp: u32) {
        let rex = 0x48 | (((phys >> 3) & 1) << 2);
        self.u8(rex);
        self.u8(0x88);
        self.u8(0x81 | ((phys & 7) << 3));
        self.u32(disp);
    }

    fn load_base(&mut self, reg: u8) {
        if reg == SP {
            // RCX = RSP; the caller uses the RSP-specific memory forms.
        } else {
            self.load_vreg(RCX, reg);
        }
    }

    // ---- immediate moves -------------------------------------------------

    /// Load any 64-bit constant. The code generator uses X8 for syscall
    /// numbers, so translate those from the AArch64 values it knows.
    pub fn mov_imm64(&mut self, rd: u8, value: u64) {
        if rd == XZR {
            return;
        }
        let value = if rd == X8 {
            match value {
                AARCH64_WRITE => X86_WRITE,
                AARCH64_EXIT => X86_EXIT,
                AARCH64_OPENAT => X86_OPENAT,
                AARCH64_CLOSE => X86_CLOSE,
                AARCH64_READ => X86_READ,
                other => other,
            }
        } else {
            value
        };
        self.mov_phys_imm64(RAX, value);
        self.store_vreg(rd, RAX);
    }

    /// Load the absolute address of an interned string, patched at finish.
    pub fn mov_abs(&mut self, rd: u8, string_id: usize) {
        if rd == XZR {
            return;
        }
        self.u8(0x48);
        self.u8(0xB8);
        let at = self.code.len();
        self.u64(0);
        self.fixups.push(Fixup::Str { at, id: string_id });
        self.store_vreg(rd, RAX);
    }

    /// Load the absolute address of a code label, patched at finish.
    pub fn mov_label_addr(&mut self, rd: u8, label: &str) {
        if rd == XZR {
            return;
        }
        self.u8(0x48);
        self.u8(0xB8);
        let at = self.code.len();
        self.u64(0);
        self.fixups.push(Fixup::LabelAddr {
            at,
            label: label.to_string(),
        });
        self.store_vreg(rd, RAX);
    }

    // ---- register moves --------------------------------------------------

    pub fn mov_reg_reg(&mut self, rd: u8, rm: u8) {
        if rd == XZR {
            return;
        }
        self.load_vreg(RAX, rm);
        self.store_vreg(rd, RAX);
    }

    pub fn movz_reg(&mut self, rd: u8, imm: u16) {
        self.mov_imm64(rd, imm as u64);
    }

    // ---- memory ----------------------------------------------------------

    /// `ldr rd, [rn, #disp]`
    pub fn ldr(&mut self, rd: u8, rn: u8, disp: u32) {
        if rd == XZR {
            return;
        }
        if rn == SP {
            self.load_mem_sp(RAX, disp);
        } else {
            self.load_base(rn);
            self.load_mem_rcx(RAX, disp);
        }
        self.store_vreg(rd, RAX);
    }

    /// `str rt, [rn, #disp]`
    pub fn str(&mut self, rt: u8, rn: u8, disp: u32) {
        if rn == SP {
            self.load_vreg(RAX, rt);
            self.store_mem_sp(RAX, disp);
        } else {
            self.load_base(rn);
            self.load_vreg(RAX, rt);
            self.store_mem_rcx(RAX, disp);
        }
    }

    /// `ldrb wt, [rn, #disp]`
    pub fn ldrb(&mut self, rt: u8, rn: u8, disp: u32) {
        if rt == XZR {
            return;
        }
        if rn == SP {
            self.load_byte_sp(RAX, disp);
        } else {
            self.load_base(rn);
            self.load_byte_rcx(RAX, disp);
        }
        self.store_vreg(rt, RAX);
    }

    /// `strb wt, [rn, #disp]`
    pub fn strb(&mut self, rt: u8, rn: u8, disp: u32) {
        if rn == SP {
            self.load_vreg(RAX, rt);
            self.store_byte_sp(RAX, disp);
        } else {
            self.load_base(rn);
            self.load_vreg(RAX, rt);
            self.store_byte_rcx(RAX, disp);
        }
    }

    // ---- arithmetic ------------------------------------------------------

    pub fn add_reg_reg(&mut self, rd: u8, rn: u8, rm: u8) {
        self.load_vreg(RAX, rn);
        self.load_vreg(RCX, rm);
        self.u8(0x48);
        self.u8(0x01);
        self.u8(0xC8); // add rax, rcx
        self.store_vreg(rd, RAX);
    }

    pub fn sub_reg_reg(&mut self, rd: u8, rn: u8, rm: u8) {
        self.load_vreg(RAX, rn);
        self.load_vreg(RCX, rm);
        self.u8(0x48);
        self.u8(0x29);
        self.u8(0xC8); // sub rax, rcx
        self.store_vreg(rd, RAX);
    }

    pub fn mul_reg_reg(&mut self, rd: u8, rn: u8, rm: u8) {
        self.load_vreg(RAX, rn);
        self.load_vreg(RCX, rm);
        self.u8(0x48);
        self.u8(0x0F);
        self.u8(0xAF);
        self.u8(0xC1); // imul rax, rcx
        self.store_vreg(rd, RAX);
    }

    pub fn sdiv_reg_reg(&mut self, rd: u8, rn: u8, rm: u8) {
        self.load_vreg(RAX, rn);
        self.load_vreg(RCX, rm);
        // Match AArch64: INT64_MIN / -1 is INT64_MIN, not a divide fault.
        self.u8(0x48);
        self.u8(0x83);
        self.u8(0xF9);
        self.u8(0xFF); // cmp rcx, -1
        self.u8(0x75);
        self.u8(5); // jne to cqo
        self.u8(0x48);
        self.u8(0xF7);
        self.u8(0xD8); // neg rax
        self.u8(0xEB);
        self.u8(5); // jmp past idiv
        self.u8(0x48);
        self.u8(0x99); // cqo
        self.u8(0x48);
        self.u8(0xF7);
        self.u8(0xF9); // idiv rcx
        self.store_vreg(rd, RAX);
    }

    /// Unsigned division. `print_int` uses this on the magnitude, so
    /// `INT64_MIN` prints correctly.
    pub fn udiv_reg_reg(&mut self, rd: u8, rn: u8, rm: u8) {
        self.load_vreg(RAX, rn);
        self.load_vreg(RCX, rm);
        self.u8(0x31);
        self.u8(0xD2); // xor edx, edx
        self.u8(0x48);
        self.u8(0xF7);
        self.u8(0xF1); // div rcx
        self.store_vreg(rd, RAX);
    }

    /// `msub rd, rn, rm, ra` computes `ra - rn * rm`.
    pub fn msub_reg_reg(&mut self, rd: u8, rn: u8, rm: u8, ra: u8) {
        self.load_vreg(RAX, ra);
        self.load_vreg(RCX, rn);
        self.load_vreg(RDX, rm);
        self.u8(0x48);
        self.u8(0x0F);
        self.u8(0xAF);
        self.u8(0xCA); // imul rcx, rdx
        self.u8(0x48);
        self.u8(0x29);
        self.u8(0xC8); // sub rax, rcx
        self.store_vreg(rd, RAX);
    }

    pub fn neg_reg(&mut self, rd: u8, rm: u8) {
        self.load_vreg(RAX, rm);
        self.u8(0x48);
        self.u8(0xF7);
        self.u8(0xD8); // neg rax
        self.store_vreg(rd, RAX);
    }

    pub fn cmp_reg_reg(&mut self, rn: u8, rm: u8) {
        self.load_vreg(RAX, rn);
        self.load_vreg(RCX, rm);
        self.u8(0x48);
        self.u8(0x39);
        self.u8(0xC8); // cmp rax, rcx
    }

    pub fn add_imm(&mut self, rd: u8, rn: u8, imm: u32) {
        self.load_sp_or_vreg(RAX, rn);
        if imm != 0 {
            self.u8(0x48);
            self.u8(0x05);
            self.u32(imm); // add rax, imm32
        }
        self.store_sp_or_vreg(rd, RAX);
    }

    pub fn sub_imm(&mut self, rd: u8, rn: u8, imm: u32) {
        self.load_sp_or_vreg(RAX, rn);
        if imm != 0 {
            self.u8(0x48);
            self.u8(0x2D);
            self.u32(imm); // sub rax, imm32
        }
        self.store_sp_or_vreg(rd, RAX);
    }

    /// `cset rd, cond` — 1 if the condition holds, else 0.
    pub fn cset(&mut self, rd: u8, cond: u8) {
        if rd == XZR {
            return;
        }
        let op = match cond {
            CC_EQ => 0x94,
            CC_NE => 0x95,
            CC_LT => 0x9C,
            CC_GE => 0x9D,
            CC_LE => 0x9E,
            CC_GT => 0x9F,
            _ => 0x94,
        };
        self.u8(0x0F);
        self.u8(op);
        self.u8(0xC0); // setcc al
        self.u8(0x0F);
        self.u8(0xB6);
        self.u8(0xC0); // movzx eax, al
        self.store_vreg(rd, RAX);
    }

    // ---- stack -----------------------------------------------------------

    pub fn push_reg(&mut self, reg: u8) {
        self.u8(0x48);
        self.u8(0x83);
        self.u8(0xEC);
        self.u8(16); // sub rsp, 16
        self.load_vreg(RAX, reg);
        self.store_mem_sp(RAX, 0);
    }

    pub fn pop_reg(&mut self, reg: u8) {
        self.load_mem_sp(RAX, 0);
        self.u8(0x48);
        self.u8(0x83);
        self.u8(0xC4);
        self.u8(16); // add rsp, 16
        self.store_vreg(reg, RAX);
    }

    // ---- control flow ----------------------------------------------------

    pub fn b_label(&mut self, label: &str) {
        let at = self.code.len();
        self.u8(0xE9);
        self.u32(0);
        self.fixups.push(Fixup::Label {
            at: at + 1,
            label: label.to_string(),
        });
    }

    pub fn bl_label(&mut self, label: &str) {
        let at = self.code.len();
        self.u8(0xE8);
        self.u32(0);
        self.fixups.push(Fixup::Label {
            at: at + 1,
            label: label.to_string(),
        });
    }

    /// `call reg` — call the address held in a virtual register.
    pub fn call_reg(&mut self, reg: u8) {
        self.load_vreg(RAX, reg);
        self.u8(0xFF);
        self.u8(0xD0);
    }

    pub fn bcond_label(&mut self, cond: u8, label: &str) {
        let at = self.code.len();
        let op = match cond {
            CC_EQ => 0x84,
            CC_NE => 0x85,
            CC_LT => 0x8C,
            CC_GE => 0x8D,
            CC_LE => 0x8E,
            CC_GT => 0x8F,
            _ => 0x84,
        };
        self.u8(0x0F);
        self.u8(op);
        self.u32(0);
        self.fixups.push(Fixup::Label {
            at: at + 2,
            label: label.to_string(),
        });
    }

    pub fn svc(&mut self) {
        self.load_vreg(RDI, X0);
        self.load_vreg(RSI, X1);
        self.load_vreg(RDX, X2);
        self.load_vreg(R10, X3);
        self.load_vreg(R8, X4);
        self.load_vreg(R9, X5);
        self.load_vreg(RAX, X8);
        self.u8(0x0F);
        self.u8(0x05); // syscall
        self.store_vreg(X0, RAX);
    }

    pub fn ret(&mut self) {
        self.u8(0xC3);
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

        let string_base = IMAGE_BASE + TEXT_OFFSET + self.code.len() as u64;
        let labels = self.labels.clone();
        let fixups = std::mem::take(&mut self.fixups);
        for f in &fixups {
            match f {
                Fixup::Label { at, label } => {
                    let target = *labels
                        .get(label)
                        .unwrap_or_else(|| panic!("internal: branch to unknown label '{label}'"));
                    let delta = target as i64 - (*at as i64 + 4);
                    self.patch_u32(*at, delta as i32 as u32);
                }
                Fixup::Str { at, id } => {
                    let addr = string_base + offsets[*id] as u64;
                    self.patch_u64(*at, addr);
                }
                Fixup::LabelAddr { at, label } => {
                    let target = *labels
                        .get(label)
                        .unwrap_or_else(|| panic!("internal: address of unknown label '{label}'"))
                        as u64;
                    let addr = IMAGE_BASE + TEXT_OFFSET + target;
                    self.patch_u64(*at, addr);
                }
            }
        }

        (self.code, rodata)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(a: Asm) -> Vec<u8> {
        let (text, _) = a.finish();
        text
    }

    #[test]
    fn moves_a_full_64_bit_constant() {
        let mut a = Asm::new();
        a.mov_imm64(X0, 0x1122_3344_5566_7788);
        let text = bytes(a);
        assert_eq!(&text[0..2], &[0x48, 0xB8]);
        assert_eq!(
            u64::from_le_bytes(text[2..10].try_into().unwrap()),
            0x1122_3344_5566_7788
        );
    }

    #[test]
    fn translates_aarch64_syscall_numbers() {
        let cases = [(64u64, 1u64), (93, 60), (56, 257), (57, 3), (63, 0)];
        for (aarch64, x86) in cases {
            let mut a = Asm::new();
            a.mov_imm64(X8, aarch64);
            let text = bytes(a);
            assert_eq!(
                u64::from_le_bytes(text[2..10].try_into().unwrap()),
                x86,
                "syscall {aarch64} should become {x86}"
            );
        }
    }

    #[test]
    fn leaves_non_syscall_constants_alone() {
        let mut a = Asm::new();
        a.mov_imm64(X8, 0x52_0000);
        let text = bytes(a);
        assert_eq!(
            u64::from_le_bytes(text[2..10].try_into().unwrap()),
            0x52_0000
        );
    }

    #[test]
    fn forward_jump_is_patched() {
        let mut a = Asm::new();
        a.b_label("end");
        a.ret();
        a.label("end");
        let text = bytes(a);
        // E9 at 0, displacement at 1, ret at 5, target at 6.
        assert_eq!(&text[0..1], &[0xE9]);
        assert_eq!(i32::from_le_bytes(text[1..5].try_into().unwrap()), 1);
    }

    #[test]
    fn forward_conditional_jump_is_patched() {
        let mut a = Asm::new();
        a.bcond_label(CC_EQ, "end");
        a.ret();
        a.label("end");
        let text = bytes(a);
        // 0F 84 at 0, displacement at 2, ret at 6, target at 7.
        assert_eq!(&text[0..2], &[0x0F, 0x84]);
        assert_eq!(i32::from_le_bytes(text[2..6].try_into().unwrap()), 1);
    }

    #[test]
    fn call_is_patched() {
        let mut a = Asm::new();
        a.bl_label("target");
        a.label("target");
        a.ret();
        let text = bytes(a);
        assert_eq!(&text[0..1], &[0xE8]);
        assert_eq!(i32::from_le_bytes(text[1..5].try_into().unwrap()), 0);
    }

    #[test]
    fn string_address_lands_after_the_code() {
        let mut a = Asm::new();
        let id = a.add_string("hi");
        a.mov_abs(X0, id);
        let (text, rodata) = a.finish();
        let expected = IMAGE_BASE + TEXT_OFFSET + text.len() as u64;
        assert_eq!(
            u64::from_le_bytes(text[2..10].try_into().unwrap()),
            expected
        );
        assert_eq!(rodata.len(), 8 + 2);
        assert_eq!(u64::from_le_bytes(rodata[0..8].try_into().unwrap()), 2);
        assert_eq!(&rodata[8..], b"hi");
    }

    #[test]
    fn compares_and_sets_a_boolean() {
        let mut a = Asm::new();
        a.cmp_reg_reg(X0, X1);
        a.cset(X0, CC_LT);
        let text = bytes(a);
        // The comparison ends with `cmp rax, rcx`.
        assert!(text.windows(3).any(|w| w == [0x48, 0x39, 0xC8]));
        // `cset` lowers to `setl al; movzx eax, al`.
        assert!(text
            .windows(6)
            .any(|w| w == [0x0F, 0x9C, 0xC0, 0x0F, 0xB6, 0xC0]));
    }

    #[test]
    fn pushes_and_pops_a_full_slot() {
        let mut a = Asm::new();
        a.push_reg(X0);
        a.pop_reg(X0);
        let text = bytes(a);
        assert_eq!(&text[0..4], &[0x48, 0x83, 0xEC, 16]);
        assert!(text.windows(4).any(|w| w == [0x48, 0x83, 0xC4, 16]));
    }
}
