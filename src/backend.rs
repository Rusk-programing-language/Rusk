//! Target selection and the common assembly interface used by codegen.
//!
//! Rusk has two hand-written machine-code backends. They expose the same small
//! virtual-register API (`X0`/`SP`/`ldr`/`str`/`bcond`/...), so the code
//! generator is written once and the chosen target only changes the encoder and
//! the ELF machine word.

pub use crate::aarch64::{
    CC_EQ, CC_GE, CC_GT, CC_LE, CC_LT, CC_NE, IMAGE_BASE, SP, SYS_EXIT, SYS_WRITE, TEXT_OFFSET, X0,
    X1, X10, X11, X12, X13, X19, X2, X20, X21, X22, X23, X24, X29, X3, X30, X4, X5, X6, X7, X8, X9,
    XZR,
};

use crate::aarch64;
use crate::x86_64;

/// Which instruction set a bake targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    AArch64,
    X86_64,
}

impl Target {
    /// The platform running the compiler. Bakes default to the host so that
    /// `rusk run` can execute what it just produced.
    pub fn host() -> Self {
        if cfg!(target_arch = "x86_64") {
            Target::X86_64
        } else {
            Target::AArch64
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Target::AArch64 => "aarch64",
            Target::X86_64 => "x86_64",
        }
    }

    /// The ELF `e_machine` value.
    pub fn machine(self) -> u16 {
        match self {
            Target::AArch64 => 183, // EM_AARCH64
            Target::X86_64 => 62,   // EM_X86_64
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "aarch64" | "arm64" => Some(Target::AArch64),
            "x86_64" | "x86-64" | "amd64" => Some(Target::X86_64),
            _ => None,
        }
    }
}

impl Default for Target {
    fn default() -> Self {
        Target::host()
    }
}

enum Inner {
    AArch64(aarch64::Asm),
    X86_64(x86_64::Asm),
}

/// A target-independent assembly buffer. Each method forwards to the encoder
/// chosen by `Target`.
pub struct Asm {
    inner: Inner,
}

impl Asm {
    pub fn new(target: Target) -> Self {
        let inner = match target {
            Target::AArch64 => Inner::AArch64(aarch64::Asm::new()),
            Target::X86_64 => Inner::X86_64(x86_64::Asm::new()),
        };
        Asm { inner }
    }

    pub fn label(&mut self, name: &str) {
        match &mut self.inner {
            Inner::AArch64(a) => a.label(name),
            Inner::X86_64(a) => a.label(name),
        }
    }

    pub fn add_string(&mut self, s: &str) -> usize {
        match &mut self.inner {
            Inner::AArch64(a) => a.add_string(s),
            Inner::X86_64(a) => a.add_string(s),
        }
    }

    pub fn mov_imm64(&mut self, rd: u8, value: u64) {
        match &mut self.inner {
            Inner::AArch64(a) => a.mov_imm64(rd, value),
            Inner::X86_64(a) => a.mov_imm64(rd, value),
        }
    }

    pub fn mov_abs(&mut self, rd: u8, string_id: usize) {
        match &mut self.inner {
            Inner::AArch64(a) => a.mov_abs(rd, string_id),
            Inner::X86_64(a) => a.mov_abs(rd, string_id),
        }
    }

    pub fn mov_reg_reg(&mut self, rd: u8, rm: u8) {
        match &mut self.inner {
            Inner::AArch64(a) => a.mov_reg_reg(rd, rm),
            Inner::X86_64(a) => a.mov_reg_reg(rd, rm),
        }
    }

    pub fn movz_reg(&mut self, rd: u8, imm: u16) {
        match &mut self.inner {
            Inner::AArch64(a) => a.movz_reg(rd, imm),
            Inner::X86_64(a) => a.movz_reg(rd, imm),
        }
    }

    pub fn ldr(&mut self, rd: u8, rn: u8, disp: u32) {
        match &mut self.inner {
            Inner::AArch64(a) => a.ldr(rd, rn, disp),
            Inner::X86_64(a) => a.ldr(rd, rn, disp),
        }
    }

    pub fn str(&mut self, rt: u8, rn: u8, disp: u32) {
        match &mut self.inner {
            Inner::AArch64(a) => a.str(rt, rn, disp),
            Inner::X86_64(a) => a.str(rt, rn, disp),
        }
    }

    pub fn ldrb(&mut self, rt: u8, rn: u8, disp: u32) {
        match &mut self.inner {
            Inner::AArch64(a) => a.ldrb(rt, rn, disp),
            Inner::X86_64(a) => a.ldrb(rt, rn, disp),
        }
    }

    pub fn strb(&mut self, rt: u8, rn: u8, disp: u32) {
        match &mut self.inner {
            Inner::AArch64(a) => a.strb(rt, rn, disp),
            Inner::X86_64(a) => a.strb(rt, rn, disp),
        }
    }

    pub fn add_reg_reg(&mut self, rd: u8, rn: u8, rm: u8) {
        match &mut self.inner {
            Inner::AArch64(a) => a.add_reg_reg(rd, rn, rm),
            Inner::X86_64(a) => a.add_reg_reg(rd, rn, rm),
        }
    }

    pub fn sub_reg_reg(&mut self, rd: u8, rn: u8, rm: u8) {
        match &mut self.inner {
            Inner::AArch64(a) => a.sub_reg_reg(rd, rn, rm),
            Inner::X86_64(a) => a.sub_reg_reg(rd, rn, rm),
        }
    }

    pub fn mul_reg_reg(&mut self, rd: u8, rn: u8, rm: u8) {
        match &mut self.inner {
            Inner::AArch64(a) => a.mul_reg_reg(rd, rn, rm),
            Inner::X86_64(a) => a.mul_reg_reg(rd, rn, rm),
        }
    }

    pub fn sdiv_reg_reg(&mut self, rd: u8, rn: u8, rm: u8) {
        match &mut self.inner {
            Inner::AArch64(a) => a.sdiv_reg_reg(rd, rn, rm),
            Inner::X86_64(a) => a.sdiv_reg_reg(rd, rn, rm),
        }
    }

    pub fn udiv_reg_reg(&mut self, rd: u8, rn: u8, rm: u8) {
        match &mut self.inner {
            Inner::AArch64(a) => a.udiv_reg_reg(rd, rn, rm),
            Inner::X86_64(a) => a.udiv_reg_reg(rd, rn, rm),
        }
    }

    pub fn msub_reg_reg(&mut self, rd: u8, rn: u8, rm: u8, ra: u8) {
        match &mut self.inner {
            Inner::AArch64(a) => a.msub_reg_reg(rd, rn, rm, ra),
            Inner::X86_64(a) => a.msub_reg_reg(rd, rn, rm, ra),
        }
    }

    pub fn neg_reg(&mut self, rd: u8, rm: u8) {
        match &mut self.inner {
            Inner::AArch64(a) => a.neg_reg(rd, rm),
            Inner::X86_64(a) => a.neg_reg(rd, rm),
        }
    }

    pub fn cmp_reg_reg(&mut self, rn: u8, rm: u8) {
        match &mut self.inner {
            Inner::AArch64(a) => a.cmp_reg_reg(rn, rm),
            Inner::X86_64(a) => a.cmp_reg_reg(rn, rm),
        }
    }

    pub fn add_imm(&mut self, rd: u8, rn: u8, imm: u32) {
        match &mut self.inner {
            Inner::AArch64(a) => a.add_imm(rd, rn, imm),
            Inner::X86_64(a) => a.add_imm(rd, rn, imm),
        }
    }

    pub fn sub_imm(&mut self, rd: u8, rn: u8, imm: u32) {
        match &mut self.inner {
            Inner::AArch64(a) => a.sub_imm(rd, rn, imm),
            Inner::X86_64(a) => a.sub_imm(rd, rn, imm),
        }
    }

    pub fn cset(&mut self, rd: u8, cond: u8) {
        match &mut self.inner {
            Inner::AArch64(a) => a.cset(rd, cond),
            Inner::X86_64(a) => a.cset(rd, cond),
        }
    }

    pub fn push_reg(&mut self, reg: u8) {
        match &mut self.inner {
            Inner::AArch64(a) => a.push_reg(reg),
            Inner::X86_64(a) => a.push_reg(reg),
        }
    }

    pub fn pop_reg(&mut self, reg: u8) {
        match &mut self.inner {
            Inner::AArch64(a) => a.pop_reg(reg),
            Inner::X86_64(a) => a.pop_reg(reg),
        }
    }

    pub fn b_label(&mut self, label: &str) {
        match &mut self.inner {
            Inner::AArch64(a) => a.b_label(label),
            Inner::X86_64(a) => a.b_label(label),
        }
    }

    pub fn bl_label(&mut self, label: &str) {
        match &mut self.inner {
            Inner::AArch64(a) => a.bl_label(label),
            Inner::X86_64(a) => a.bl_label(label),
        }
    }

    pub fn bcond_label(&mut self, cond: u8, label: &str) {
        match &mut self.inner {
            Inner::AArch64(a) => a.bcond_label(cond, label),
            Inner::X86_64(a) => a.bcond_label(cond, label),
        }
    }

    pub fn svc(&mut self) {
        match &mut self.inner {
            Inner::AArch64(a) => a.svc(),
            Inner::X86_64(a) => a.svc(),
        }
    }

    pub fn ret(&mut self) {
        match &mut self.inner {
            Inner::AArch64(a) => a.ret(),
            Inner::X86_64(a) => a.ret(),
        }
    }

    pub fn finish(self) -> (Vec<u8>, Vec<u8>) {
        match self.inner {
            Inner::AArch64(a) => a.finish(),
            Inner::X86_64(a) => a.finish(),
        }
    }
}
