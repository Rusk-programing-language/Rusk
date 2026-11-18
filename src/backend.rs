//! Target selection and the common assembly interface used by codegen.
//!
//! Rusk has two hand-written machine-code backends. They expose the same small
//! virtual-register API (`X0`/`SP`/`ldr`/`str`/`bcond`/...), so the code
//! generator is written once and the chosen target only changes the encoder and
//! the ELF machine word.

pub use crate::x86_64::WinArg;

pub use crate::aarch64::{
    CC_EQ, CC_GE, CC_GT, CC_LE, CC_LT, CC_NE, IMAGE_BASE, SP, SYS_EXIT, SYS_WRITE, TEXT_OFFSET, X0,
    X1, X10, X11, X12, X13, X14, X15, X16, X17, X18, X19, X2, X20, X21, X22, X23, X24, X25, X26,
    X27, X28, X29, X3, X30, X4, X5, X6, X7, X8, X9, XZR,
};

use crate::aarch64;
use crate::x86_64;

/// Scratch slots for the small native GUI runtime. User32 is loaded at runtime
/// with `LoadLibraryA`/`GetProcAddress`, so the PE still needs only kernel32.
pub const WIN_GUI_FLAG_OFFSET: u64 = 0x9000;
pub const WIN_GUI_USER32_OFFSET: u64 = 0x9008;
pub const WIN_GUI_HINSTANCE_OFFSET: u64 = 0x9010;
pub const WIN_GUI_HWND_OFFSET: u64 = 0x9018;
pub const WIN_GUI_NEXT_ID_OFFSET: u64 = 0x9020;
pub const WIN_GUI_EVENT_COUNT_OFFSET: u64 = 0x9028;
pub const WIN_GUI_EVENT_READ_OFFSET: u64 = 0x9030;
pub const WIN_GUI_EVENT_KIND_OFFSET: u64 = 0x9038;
pub const WIN_GUI_EVENT_CONTROL_OFFSET: u64 = 0x9040;
pub const WIN_GUI_EVENT_VALUE_OFFSET: u64 = 0x9048;
pub const WIN_GUI_WNDPROC_ARG1_OFFSET: u64 = 0x9050;
pub const WIN_GUI_WNDPROC_ARG2_OFFSET: u64 = 0x9058;
pub const WIN_GUI_WNDPROC_ARG3_OFFSET: u64 = 0x9060;
pub const WIN_GUI_WNDPROC_ARG4_OFFSET: u64 = 0x9068;
pub const WIN_GUI_TEMP_OFFSET: u64 = 0x9070;
pub const WIN_GUI_WNDCLASS_OFFSET: u64 = 0x9080;
pub const WIN_GUI_MSG_OFFSET: u64 = 0x90C0;
pub const WIN_GUI_TEXT_OFFSET: u64 = 0x9100;
pub const WIN_GUI_TEXT_CAPACITY: u64 = 4096;
pub const WIN_GUI_END_OFFSET: u64 = 0xA200;

/// Function-pointer slots, filled by `GetProcAddress` during GUI init.
pub const WIN_GUI_FN_BASE: u64 = 0xA200;
pub const WIN_GUI_FN_REGISTER_CLASS: u64 = WIN_GUI_FN_BASE;
pub const WIN_GUI_FN_CREATE_WINDOW: u64 = WIN_GUI_FN_BASE + 8;
pub const WIN_GUI_FN_SHOW_WINDOW: u64 = WIN_GUI_FN_BASE + 16;
pub const WIN_GUI_FN_UPDATE_WINDOW: u64 = WIN_GUI_FN_BASE + 24;
pub const WIN_GUI_FN_PEEK_MESSAGE: u64 = WIN_GUI_FN_BASE + 32;
pub const WIN_GUI_FN_TRANSLATE_MESSAGE: u64 = WIN_GUI_FN_BASE + 40;
pub const WIN_GUI_FN_DISPATCH_MESSAGE: u64 = WIN_GUI_FN_BASE + 48;
pub const WIN_GUI_FN_DEF_WINDOW_PROC: u64 = WIN_GUI_FN_BASE + 56;
pub const WIN_GUI_FN_POST_QUIT_MESSAGE: u64 = WIN_GUI_FN_BASE + 64;
pub const WIN_GUI_FN_MESSAGE_BOX: u64 = WIN_GUI_FN_BASE + 72;
pub const WIN_GUI_FN_GET_DLG_ITEM: u64 = WIN_GUI_FN_BASE + 80;
pub const WIN_GUI_FN_SET_WINDOW_TEXT: u64 = WIN_GUI_FN_BASE + 88;
pub const WIN_GUI_FN_GET_WINDOW_TEXT: u64 = WIN_GUI_FN_BASE + 96;
pub const WIN_GUI_FN_GET_WINDOW_TEXT_LEN: u64 = WIN_GUI_FN_BASE + 104;
pub const WIN_GUI_FN_DESTROY_WINDOW: u64 = WIN_GUI_FN_BASE + 112;
pub const WIN_GUI_FN_END: u64 = WIN_GUI_FN_BASE + 120;

/// Which instruction set and operating system a bake targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    AArch64,
    X86_64,
    /// Experimental Windows x86-64 output. The code generator and runtime are
    /// shared with [`Target::X86_64`], but the ABI and the executable wrapper
    /// are Windows-specific.
    WindowsX86_64,
}

impl Target {
    /// The platform running the compiler. Bakes default to the host so that
    /// `rusk run` can execute what it just produced. The compiler itself runs
    /// on Windows, but its default output stays Linux ELF; ask for
    /// [`Target::WindowsX86_64`] explicitly to make a `.exe`.
    pub fn host() -> Self {
        if cfg!(target_arch = "x86_64") {
            Target::X86_64
        } else {
            Target::AArch64
        }
    }

    /// The targets a portable toast contains unless the user picks others.
    pub fn linux_all() -> [Target; 2] {
        [Target::AArch64, Target::X86_64]
    }

    pub fn name(self) -> &'static str {
        match self {
            Target::AArch64 => "aarch64",
            Target::X86_64 => "x86_64",
            Target::WindowsX86_64 => "windows-x86_64",
        }
    }

    /// True for the experimental Windows target.
    pub fn is_windows(self) -> bool {
        matches!(self, Target::WindowsX86_64)
    }

    /// The ELF `e_machine` value. Only meaningful for Linux targets.
    pub fn machine(self) -> u16 {
        match self {
            Target::AArch64 => 183,                       // EM_AARCH64
            Target::X86_64 | Target::WindowsX86_64 => 62, // EM_X86_64 / AMD64
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "aarch64" | "arm64" => Some(Target::AArch64),
            "x86_64" | "x86-64" | "amd64" => Some(Target::X86_64),
            "windows" | "windows-x86_64" | "win64" | "pe" => Some(Target::WindowsX86_64),
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
            Target::WindowsX86_64 => Inner::X86_64(x86_64::Asm::new_windows()),
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

    pub fn mov_label_addr(&mut self, rd: u8, label: &str) {
        match &mut self.inner {
            Inner::AArch64(a) => a.mov_label_addr(rd, label),
            Inner::X86_64(a) => a.mov_label_addr(rd, label),
        }
    }

    pub fn mov_data_addr(&mut self, rd: u8, offset: u64) {
        match &mut self.inner {
            Inner::AArch64(a) => a.mov_data_addr(rd, offset),
            Inner::X86_64(a) => a.mov_data_addr(rd, offset),
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

    pub fn ldrh(&mut self, rt: u8, rn: u8, disp: u32) {
        match &mut self.inner {
            Inner::AArch64(a) => a.ldrh(rt, rn, disp),
            Inner::X86_64(a) => a.ldrh(rt, rn, disp),
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

    pub fn and_reg_reg(&mut self, rd: u8, rn: u8, rm: u8) {
        match &mut self.inner {
            Inner::AArch64(a) => a.and_reg_reg(rd, rn, rm),
            Inner::X86_64(a) => a.and_reg_reg(rd, rn, rm),
        }
    }

    pub fn orr_reg_reg(&mut self, rd: u8, rn: u8, rm: u8) {
        match &mut self.inner {
            Inner::AArch64(a) => a.orr_reg_reg(rd, rn, rm),
            Inner::X86_64(a) => a.orr_reg_reg(rd, rn, rm),
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

    pub fn call_reg(&mut self, reg: u8) {
        match &mut self.inner {
            Inner::AArch64(a) => a.call_reg(reg),
            Inner::X86_64(a) => a.call_reg(reg),
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

    /// Finish the image with the final zeroed-data size. The Windows layout
    /// needs the bss size to place its import table after the data area.
    pub fn finish_with_bss(self, bss: u64) -> (Vec<u8>, Vec<u8>) {
        match self.inner {
            Inner::AArch64(a) => a.finish(),
            Inner::X86_64(a) => a.finish_with_bss(bss),
        }
    }

    /// Imported Windows functions referenced by the code, in first-use order.
    pub fn imports(&self) -> Vec<String> {
        match &self.inner {
            Inner::AArch64(_) => Vec::new(),
            Inner::X86_64(a) => a.import_names(),
        }
    }

    /// Emit the Windows entry thunk that builds an initial stack and jumps to
    /// `start`. It is a no-op on AArch64.
    pub fn windows_start(&mut self, start: &str) {
        if let Inner::X86_64(a) = &mut self.inner {
            a.windows_start(start);
        }
    }

    /// Call a `kernel32.dll` function from generated Windows startup code.
    pub fn win_call_args(&mut self, name: &str, argc: usize) {
        if let Inner::X86_64(a) = &mut self.inner {
            a.win_call_args(name, argc);
        }
    }

    /// Move a Windows call's return value into `X0`.
    pub fn win_result(&mut self) {
        if let Inner::X86_64(a) = &mut self.inner {
            a.win_result();
        }
    }

    /// Call an imported function whose address is stored in a writable slot.
    /// Used by the `simple` GUI runtime, which resolves user32 at load time.
    pub fn win_call_ptr(&mut self, slot: u64, argc: usize, stack_args: &[WinArg]) {
        if let Inner::X86_64(a) = &mut self.inner {
            a.win_call_ptr(slot, argc, stack_args);
        }
    }

    /// Emit the hand-written `WndProc` used by the `simple` GUI window class.
    pub fn win_emit_wndproc(&mut self, label: &str) {
        if let Inner::X86_64(a) = &mut self.inner {
            a.win_emit_wndproc(label);
        }
    }
}
