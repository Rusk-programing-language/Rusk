//! A tiny x86-64 machine-code emitter — the second Rusk backend.
//!
//! The Rusk code generator is written against a small virtual register machine
//! with `X0..X30`, `SP`, and `XZR`. Each virtual register has a home slot in a
//! fixed block of zeroed memory, and this module *allocates* those virtual
//! registers to real x86-64 registers inside each straight-line block. Values
//! are written back when a block ends, when control leaves the block, or when
//! the small physical register pool needs a slot for something else. `SP` is
//! the real process stack pointer, so frames, pushes, calls, and returns
//! behave the way the AArch64 emitter expects.
//!
//! Target: x86-64 Linux, little endian, `syscall`.

use crate::aarch64::{
    CC_EQ, CC_GE, CC_GT, CC_LE, CC_LT, CC_NE, IMAGE_BASE, SP, TEXT_OFFSET, X0, X1, X2, X3, X4, X5,
    X8, XZR,
};
use crate::backend::{
    WIN_GUI_EVENT_CONTROL_OFFSET, WIN_GUI_EVENT_COUNT_OFFSET, WIN_GUI_EVENT_KIND_OFFSET,
    WIN_GUI_EVENT_VALUE_OFFSET, WIN_GUI_FN_DEF_WINDOW_PROC, WIN_GUI_FN_POST_QUIT_MESSAGE,
    WIN_GUI_TEMP_OFFSET, WIN_GUI_WNDPROC_ARG1_OFFSET, WIN_GUI_WNDPROC_ARG2_OFFSET,
    WIN_GUI_WNDPROC_ARG3_OFFSET, WIN_GUI_WNDPROC_ARG4_OFFSET,
};
use crate::pe;
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
const R11: u8 = 11;

/// Physical registers the allocator may hand to virtual registers. These are
/// caller-saved and not used as scratch by the instruction encoders, so a
/// cached value survives until the next branch, call, or syscall.
const POOL: [u8; 6] = [RSI, RDI, R8, R9, R10, R11];

fn in_pool(phys: u8) -> bool {
    POOL.contains(&phys)
}

/// Offset of the virtual-register home slots inside the writable data area.
/// It sits just past the text heap, so globals and heap growth can never
/// overwrite the register file.
pub const VREG_OFFSET: u64 = 0x11_0000;
/// Bytes reserved after `VREG_OFFSET` for the home slots.
pub const VREG_OFFSET_SLACK: u64 = 256;

/// Scratch slots in the writable data area used by the Windows runtime
/// wrappers: a saved standard handle, the byte count from `WriteFile`, the
/// byte count from `ReadFile`, the console mode saved when the terminal enters
/// raw mode, a working console mode, and a flag that mirrors `RAW_FLAG_OFFSET`.
const WIN_HANDLE_OFFSET: u64 = 0x8100;
const WIN_WRITTEN_OFFSET: u64 = 0x8108;
const WIN_READ_OFFSET: u64 = 0x8110;
pub const WIN_CONSOLE_MODE_OFFSET: u64 = 0x8118;
const WIN_CONSOLE_RAW_OFFSET: u64 = 0x8120;
const WIN_CONSOLE_SAVED_OFFSET: u64 = 0x8128;

/// A synthetic `argc, argv, envp` table, laid out like the Linux initial stack
/// so the rest of the runtime can read it unchanged:
///
/// ```text
/// [argc][argv[0]]...[NULL][envp[0]]...[NULL]
/// ```
///
/// `argv` pointers live in the table; the strings behind them are copied to
/// [`WIN_ARGS_STR_OFFSET`]. Environment entries keep their `NAME=value` form
/// in [`WIN_ENV_BLOCK_OFFSET`] and are pointed at by [`WIN_ENV_PTRS_OFFSET`].
pub const WIN_ARGS_OFFSET: u64 = 0x9000;
/// Up to this many command-line words are exposed. Windows limits a command
/// line to 32 KiB, so this only truncates pathological inputs.
pub const WIN_ARGS_MAX: u64 = 1024;
pub const WIN_ARGS_STR_OFFSET: u64 = 0x2_0000;
/// Argument text lives below the virtual-register file at `0x11_0000`.
pub const WIN_ARGS_STR_CAPACITY: u64 = 0xE_0000;
pub const WIN_ENV_BLOCK_OFFSET: u64 = 0x10_0000;
pub const WIN_ENV_CAPACITY: u64 = 0x6000;
pub const WIN_ENV_MAX: u64 = 0x9000 / 8;

// AArch64 syscall numbers are what the code generator emits. x86-64 uses its
// own numbers, so `mov_imm64(X8, ...)` translates them for us.
const AARCH64_WRITE: u64 = 64;
const AARCH64_EXIT: u64 = 93;
const AARCH64_OPENAT: u64 = 56;
const AARCH64_CLOSE: u64 = 57;
const AARCH64_READ: u64 = 63;
const AARCH64_MKDIRAT: u64 = 34;
const AARCH64_UNLINKAT: u64 = 35;
const AARCH64_RENAMEAT: u64 = 38;
const AARCH64_GETDENTS64: u64 = 61;
const AARCH64_FACCESSAT: u64 = 48;
const AARCH64_CLONE: u64 = 220;
const AARCH64_EXECVE: u64 = 221;
const AARCH64_MMAP: u64 = 222;
const AARCH64_WAIT4: u64 = 260;
const AARCH64_SOCKET: u64 = 198;
const AARCH64_CONNECT: u64 = 203;
const AARCH64_ACCEPT: u64 = 202;
const AARCH64_SENDTO: u64 = 206;
const AARCH64_BIND: u64 = 200;
const AARCH64_LISTEN: u64 = 201;
const AARCH64_SETSOCKOPT: u64 = 208;
const AARCH64_IOCTL: u64 = 29;
const AARCH64_SHUTDOWN: u64 = 210;
/// The terminal ioctl requests the code generator emits.
const TCGETS: u64 = 0x5401;
const TCSETS: u64 = 0x5402;
const X86_WRITE: u64 = 1;
const X86_EXIT: u64 = 60;
const X86_OPENAT: u64 = 257;
const X86_CLOSE: u64 = 3;
const X86_READ: u64 = 0;
const X86_MKDIRAT: u64 = 258;
const X86_UNLINKAT: u64 = 263;
const X86_RENAMEAT: u64 = 264;
const X86_GETDENTS64: u64 = 217;
const X86_FACCESSAT: u64 = 269;
const X86_CLONE: u64 = 56;
const X86_EXECVE: u64 = 59;
const X86_MMAP: u64 = 9;
const X86_WAIT4: u64 = 61;
const X86_SOCKET: u64 = 41;
const X86_CONNECT: u64 = 42;
const X86_ACCEPT: u64 = 43;
const X86_SENDTO: u64 = 44;
const X86_BIND: u64 = 49;
const X86_LISTEN: u64 = 50;
const X86_SETSOCKOPT: u64 = 54;
const X86_IOCTL: u64 = 16;
const X86_SHUTDOWN: u64 = 48;

enum Fixup {
    Label {
        at: usize,
        label: String,
    },
    /// A near jump or branch whose relative operand ends `next` bytes after
    /// `at`. Used by the hand-written GUI `WndProc`.
    LabelRel32 {
        at: usize,
        next: usize,
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
    /// Absolute address of an offset into the writable data area. The base is
    /// chosen once the text and read-only sections are final.
    Data {
        at: usize,
        offset: u64,
    },
    /// Absolute address of a virtual register's home slot. Patching these
    /// keeps the register file next to the rest of the data even as the text
    /// section grows.
    Vreg {
        at: usize,
        reg: u8,
    },
}

/// A stack argument for a Windows call. `Vreg` reads a virtual register after
/// the caller's cached values have been written back, which is how the GUI
/// runtime passes coordinates and handles in positions five and above.
#[derive(Clone, Copy)]
pub enum WinArg {
    Imm(u32),
    Imm64(u64),
    Reg(u8),
    Vreg(u8),
}

pub struct Asm {
    code: Vec<u8>,
    labels: HashMap<String, usize>,
    fixups: Vec<Fixup>,
    strings: Vec<Vec<u8>>,
    /// Which physical register currently holds each virtual register, or -1.
    owner: [i8; 32],
    /// Which virtual register currently owns each physical register, or -1.
    holder: [i8; 16],
    /// Whether a cached physical register holds a value newer than its home.
    dirty: [bool; 16],
    /// Round-robin starting point when a free pool register is wanted.
    next_victim: usize,
    /// Monotonic clock and last-touch time for least-recently-used spilling.
    use_clock: u64,
    last_used: [u64; 16],
    /// True when this emitter is producing Windows x86-64 instead of Linux.
    windows: bool,
    /// The last AArch64 syscall number placed in X8, if any. Windows uses this
    /// to choose a runtime call at `svc` time.
    last_syscall: Option<u64>,
    /// Functions imported from `kernel32.dll`, in first-use order.
    imports: Vec<String>,
    /// Code offsets of 64-bit placeholders that must point at IAT slots.
    import_fixups: Vec<(usize, usize)>,
    /// Counter for labels inside the Windows runtime helpers.
    win_labels: usize,
}

impl Default for Asm {
    fn default() -> Self {
        Self::new()
    }
}

impl Asm {
    pub fn new() -> Self {
        Self::new_for(false)
    }

    /// An emitter that produces Windows x86-64 code. Syscalls become calls to
    /// small runtime wrappers around `kernel32.dll`.
    pub fn new_windows() -> Self {
        Self::new_for(true)
    }

    fn new_for(windows: bool) -> Self {
        Asm {
            code: Vec::with_capacity(1024),
            labels: HashMap::new(),
            fixups: Vec::new(),
            strings: Vec::new(),
            owner: [-1; 32],
            holder: [-1; 16],
            dirty: [false; 16],
            next_victim: 0,
            use_clock: 0,
            last_used: [0; 16],
            windows,
            last_syscall: None,
            imports: Vec::new(),
            import_fixups: Vec::new(),
            win_labels: 0,
        }
    }

    fn win_label(&mut self, hint: &str) -> String {
        self.win_labels += 1;
        format!("__win_{hint}_{}", self.win_labels)
    }

    /// Imported functions referenced so far.
    pub fn import_names(&self) -> Vec<String> {
        self.imports.clone()
    }

    pub fn label(&mut self, name: &str) {
        // Another path can arrive here, so cached registers may not describe
        // the values this path knows about.
        self.flush_all();
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

    fn load_abs(&mut self, phys: u8, reg: u8) {
        // The Windows image sits above 4 GiB, so use a full 64-bit address and
        // load through the destination register itself.
        self.u8(0x48 | ((phys >> 3) & 1));
        self.u8(0xB8 | (phys & 7));
        let at = self.code.len();
        self.u64(0);
        self.fixups.push(Fixup::Vreg { at, reg });
        let rex = 0x48 | (((phys >> 3) & 1) << 2) | ((phys >> 3) & 1);
        let modrm = ((phys & 7) << 3) | (phys & 7);
        self.u8(rex);
        self.u8(0x8B);
        self.u8(modrm);
        if phys & 7 == 4 {
            self.u8(0x24); // SIB: base = the register itself
        }
    }

    fn store_abs(&mut self, phys: u8, reg: u8) {
        // See `load_abs`: the data area is not addressable with a disp32. Save
        // a scratch register, form the 64-bit address, store, and restore.
        // None of these instructions touch the flags, so a pending conditional
        // branch is unaffected.
        let scratch = if phys == RAX { RCX } else { RAX };
        self.u8(0x50 | (scratch & 7)); // push scratch
        self.u8(0x48);
        self.u8(0xB8 | (scratch & 7));
        let at = self.code.len();
        self.u64(0);
        self.fixups.push(Fixup::Vreg { at, reg });
        let rex = 0x48 | (((phys >> 3) & 1) << 2) | ((scratch >> 3) & 1);
        self.u8(rex);
        self.u8(0x89);
        self.u8(((phys & 7) << 3) | (scratch & 7));
        self.u8(0x58 | (scratch & 7)); // pop scratch
    }

    /// Write a cached value back to its home slot if it is dirty.
    fn flush_phys(&mut self, phys: u8) {
        let holder = self.holder[phys as usize];
        if holder >= 0 && self.dirty[phys as usize] {
            self.store_abs(phys, holder as u8);
        }
        self.dirty[phys as usize] = false;
    }

    /// Drop a physical register without writing it back. Used when the value
    /// it holds is about to be replaced.
    fn discard_phys(&mut self, phys: u8) {
        if !in_pool(phys) {
            return;
        }
        let holder = self.holder[phys as usize];
        if holder >= 0 {
            self.owner[holder as usize] = -1;
            self.holder[phys as usize] = -1;
        }
        self.dirty[phys as usize] = false;
    }

    /// Drop a physical register from the allocation, writing it back first.
    fn evict_phys(&mut self, phys: u8) {
        if !in_pool(phys) {
            return;
        }
        let holder = self.holder[phys as usize];
        self.flush_phys(phys);
        if holder >= 0 {
            self.owner[holder as usize] = -1;
            self.holder[phys as usize] = -1;
        }
    }

    /// Write every dirty virtual register back and empty the allocation.
    /// Called at block boundaries, calls, returns, and syscalls, where another
    /// path or routine may replace a home slot.
    fn flush_all(&mut self) {
        for &phys in &POOL {
            self.evict_phys(phys);
        }
        self.next_victim = 0;
    }

    /// Forget the allocation without writing anything back. Used after an API
    /// call has already clobbered the volatile pool registers.
    fn discard_all(&mut self) {
        for &phys in &POOL {
            let holder = self.holder[phys as usize];
            if holder >= 0 {
                self.owner[holder as usize] = -1;
                self.holder[phys as usize] = -1;
                self.dirty[phys as usize] = false;
            }
        }
        self.next_victim = 0;
    }

    /// Mark a pool register as freshly used. Spilling the coldest register
    /// keeps short-lived values in registers instead of bouncing them through
    /// their home slots.
    fn touch(&mut self, phys: u8) {
        if in_pool(phys) {
            self.use_clock += 1;
            self.last_used[phys as usize] = self.use_clock;
        }
    }

    /// Find a pool register, spilling the least recently used one if needed.
    fn spare_pool_reg(&mut self) -> u8 {
        let mut free = None;
        for i in 0..POOL.len() {
            let phys = POOL[(self.next_victim + i) % POOL.len()];
            if self.holder[phys as usize] < 0 {
                let better = match free {
                    Some(old) => self.last_used[phys as usize] < self.last_used[old as usize],
                    None => true,
                };
                if better {
                    free = Some(phys);
                }
            }
        }
        if let Some(phys) = free {
            self.next_victim = (self.next_victim + 1) % POOL.len();
            self.touch(phys);
            return phys;
        }
        let mut victim = POOL[0];
        for &phys in &POOL[1..] {
            if self.last_used[phys as usize] < self.last_used[victim as usize] {
                victim = phys;
            }
        }
        self.next_victim = (self.next_victim + 1) % POOL.len();
        self.evict_phys(victim);
        self.touch(victim);
        victim
    }

    fn load_vreg(&mut self, phys: u8, reg: u8) {
        if reg == XZR {
            if in_pool(phys) {
                self.evict_phys(phys);
            }
            self.mov_phys_imm64(phys, 0);
            return;
        }
        self.touch(phys);
        let owner = self.owner[reg as usize];
        if owner == phys as i8 {
            return;
        }
        if in_pool(phys) {
            self.evict_phys(phys);
        }
        match owner {
            -1 => self.load_abs(phys, reg),
            p => self.mov_phys_reg(phys, p as u8),
        }
    }

    fn store_vreg(&mut self, reg: u8, phys: u8) {
        if reg == XZR {
            return;
        }
        if self.windows && reg == X8 {
            self.last_syscall = None;
        }
        // This virtual register is being replaced, so any older cached copy is
        // stale and must not linger.
        let old = self.owner[reg as usize];
        if old >= 0 && old as u8 != phys {
            self.discard_phys(old as u8);
        }
        if in_pool(phys) {
            self.touch(phys);
            if self.holder[phys as usize] != reg as i8 {
                self.evict_phys(phys);
            }
            self.holder[phys as usize] = reg as i8;
            self.owner[reg as usize] = phys as i8;
            self.dirty[phys as usize] = true;
        } else {
            let slot = self.spare_pool_reg();
            self.mov_phys_reg(slot, phys);
            self.holder[slot as usize] = reg as i8;
            self.owner[reg as usize] = slot as i8;
            self.dirty[slot as usize] = true;
        }
    }

    fn mov_phys_imm64(&mut self, phys: u8, value: u64) {
        if value <= u32::MAX as u64 {
            // `mov r32, imm32` zero-extends into the full register and is four
            // bytes shorter than a 64-bit immediate. Most Rusk constants,
            // syscall numbers, and string words fit here.
            if phys >= 8 {
                self.u8(0x41); // REX.B selects r8d-r15d
            }
            self.u8(0xB8 + (phys & 7));
            self.u32(value as u32);
        } else {
            self.u8(0x48 | ((phys >> 3) & 1));
            self.u8(0xB8 + (phys & 7));
            self.u64(value);
        }
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

    fn load_half_sp(&mut self, phys: u8, disp: u32) {
        let rex = 0x48 | (((phys >> 3) & 1) << 2);
        self.u8(rex);
        self.u8(0x0F);
        self.u8(0xB7);
        self.u8(0x84 | ((phys & 7) << 3));
        self.u8(0x24);
        self.u32(disp);
    }

    fn load_half_rcx(&mut self, phys: u8, disp: u32) {
        let rex = 0x48 | (((phys >> 3) & 1) << 2);
        self.u8(rex);
        self.u8(0x0F);
        self.u8(0xB7);
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
        if self.windows {
            let syscall = (rd == X8).then_some(value);
            self.mov_phys_imm64(RAX, value);
            self.store_vreg(rd, RAX);
            if let Some(syscall) = syscall {
                self.last_syscall = Some(syscall);
            }
            return;
        }
        let value = if rd == X8 {
            match value {
                AARCH64_WRITE => X86_WRITE,
                AARCH64_EXIT => X86_EXIT,
                AARCH64_OPENAT => X86_OPENAT,
                AARCH64_CLOSE => X86_CLOSE,
                AARCH64_READ => X86_READ,
                AARCH64_MKDIRAT => X86_MKDIRAT,
                AARCH64_UNLINKAT => X86_UNLINKAT,
                AARCH64_RENAMEAT => X86_RENAMEAT,
                AARCH64_GETDENTS64 => X86_GETDENTS64,
                AARCH64_FACCESSAT => X86_FACCESSAT,
                AARCH64_CLONE => X86_CLONE,
                AARCH64_EXECVE => X86_EXECVE,
                AARCH64_MMAP => X86_MMAP,
                AARCH64_WAIT4 => X86_WAIT4,
                AARCH64_SOCKET => X86_SOCKET,
                AARCH64_CONNECT => X86_CONNECT,
                AARCH64_ACCEPT => X86_ACCEPT,
                AARCH64_SENDTO => X86_SENDTO,
                AARCH64_BIND => X86_BIND,
                AARCH64_LISTEN => X86_LISTEN,
                AARCH64_SETSOCKOPT => X86_SETSOCKOPT,
                AARCH64_IOCTL => X86_IOCTL,
                AARCH64_SHUTDOWN => X86_SHUTDOWN,
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

    /// Load the absolute address of a slot in the writable data area.
    pub fn mov_data_addr(&mut self, rd: u8, offset: u64) {
        if rd == XZR {
            return;
        }
        self.u8(0x48);
        self.u8(0xB8);
        let at = self.code.len();
        self.u64(0);
        self.fixups.push(Fixup::Data { at, offset });
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
        if rd == XZR || rd == rm {
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

    /// `ldrh wt, [rn, #disp]`
    pub fn ldrh(&mut self, rt: u8, rn: u8, disp: u32) {
        if rt == XZR {
            return;
        }
        if rn == SP {
            self.load_half_sp(RAX, disp);
        } else {
            self.load_base(rn);
            self.load_half_rcx(RAX, disp);
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

    pub fn and_reg_reg(&mut self, rd: u8, rn: u8, rm: u8) {
        self.load_vreg(RAX, rn);
        self.load_vreg(RCX, rm);
        self.u8(0x48);
        self.u8(0x21);
        self.u8(0xC8); // and rax, rcx
        self.store_vreg(rd, RAX);
    }

    pub fn orr_reg_reg(&mut self, rd: u8, rn: u8, rm: u8) {
        self.load_vreg(RAX, rn);
        self.load_vreg(RCX, rm);
        self.u8(0x48);
        self.u8(0x09);
        self.u8(0xC8); // or rax, rcx
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
        self.flush_all();
        let at = self.code.len();
        self.u8(0xE9);
        self.u32(0);
        self.fixups.push(Fixup::Label {
            at: at + 1,
            label: label.to_string(),
        });
    }

    pub fn bl_label(&mut self, label: &str) {
        self.flush_all();
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
        self.flush_all();
        self.load_vreg(RAX, reg);
        self.u8(0xFF);
        self.u8(0xD0);
    }

    pub fn bcond_label(&mut self, cond: u8, label: &str) {
        self.flush_all();
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
        self.flush_all();
        if self.windows {
            let which = self.last_syscall.take();
            match which {
                Some(AARCH64_WRITE) => self.win_write(),
                Some(AARCH64_READ) => self.win_read(),
                Some(AARCH64_EXIT) => self.win_exit(),
                Some(AARCH64_MMAP) => self.win_mmap(),
                Some(AARCH64_OPENAT) => self.win_open(),
                Some(AARCH64_CLOSE) => self.win_close(),
                Some(AARCH64_IOCTL) => self.win_ioctl(),
                _ => {
                    // The runtime helper prints a clear message and exits.
                    self.bl_label("windows_unsupported");
                }
            }
            return;
        }
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

    // ---- Windows runtime -------------------------------------------------

    /// Register an import and return its IAT index.
    fn import_index(&mut self, name: &str) -> usize {
        if let Some(index) = self.imports.iter().position(|known| known == name) {
            return index;
        }
        self.imports.push(name.to_string());
        self.imports.len() - 1
    }

    /// Emit `mov phys, [data + offset]` for a writable scratch slot. This is
    /// deliberately raw: it must not disturb the virtual-register allocator.
    fn win_data_addr(&mut self, phys: u8, offset: u64) {
        self.u8(0x48 | ((phys >> 3) & 1));
        self.u8(0xB8 + (phys & 7));
        let at = self.code.len();
        self.u64(0);
        self.fixups.push(Fixup::Data { at, offset });
    }

    /// Set up the Windows x64 stack (16-byte alignment plus 32 bytes of shadow
    /// space), call an imported function through its IAT slot, and restore the
    /// stack exactly. The argument registers must already be loaded. Extra
    /// arguments for the callee are placed at `[rsp+32]`, `[rsp+40]`, ...
    fn win_call_stack_mixed(&mut self, name: &str, stack_args: &[WinArg]) {
        // Any cached virtual values must reach their home slots before the
        // Windows ABI call clobbers the volatile registers.
        self.flush_all();
        self.discard_all();
        let index = self.import_index(name);
        // 32 bytes of shadow space plus the stack arguments, kept 16-byte
        // aligned so the call itself stays aligned.
        let frame = (32 + stack_args.len() * 8).div_ceil(16) * 16;
        self.u8(0x41);
        self.u8(0x54); // push r12
        self.u8(0x4C);
        self.u8(0x8B);
        self.u8(0xE4); // mov r12, rsp
        self.u8(0x48);
        self.u8(0x83);
        self.u8(0xE4);
        self.u8(0xF0); // and rsp, -16
        self.u8(0x48);
        self.u8(0x81);
        self.u8(0xEC);
        self.u32(frame as u32); // sub rsp, frame
        for (i, value) in stack_args.iter().enumerate() {
            let disp = (32 + i * 8) as u8;
            match value {
                WinArg::Imm(value) => {
                    self.u8(0x48);
                    self.u8(0xC7);
                    self.u8(0x44);
                    self.u8(0x24);
                    self.u8(disp);
                    self.u32(*value); // mov qword [rsp+disp], imm32
                }
                WinArg::Imm64(value) => {
                    // Scratch R10 is safe after `discard_all`: no virtual
                    // register is cached at this point.
                    self.mov_phys_imm64(R10, *value);
                    self.store_stack_reg(disp, R10);
                }
                WinArg::Reg(phys) => self.store_stack_reg(disp, *phys),
                WinArg::Vreg(reg) => {
                    self.load_vreg(R10, *reg);
                    self.store_stack_reg(disp, R10);
                }
            }
        }
        self.u8(0x48);
        self.u8(0xB8); // mov rax, [IAT slot]
        let at = self.code.len();
        self.u64(0);
        self.import_fixups.push((at, index));
        self.u8(0x48);
        self.u8(0x8B);
        self.u8(0x00); // mov rax, [rax]
        self.u8(0xFF);
        self.u8(0xD0); // call rax
        self.u8(0x48);
        self.u8(0x81);
        self.u8(0xC4);
        self.u32(frame as u32); // add rsp, frame
        self.u8(0x4C);
        self.u8(0x89);
        self.u8(0xE4); // mov rsp, r12
        self.u8(0x41);
        self.u8(0x5C); // pop r12
    }

    /// Emit `mov [rsp+disp], reg` for a Windows stack argument.
    fn store_stack_reg(&mut self, disp: u8, phys: u8) {
        self.u8(0x48 | ((phys >> 3) & 1));
        self.u8(0x89);
        self.u8(0x44 | ((phys & 7) << 3));
        self.u8(0x24);
        self.u8(disp);
    }

    fn win_call_stack(&mut self, name: &str, stack_args: &[u64]) {
        let args: Vec<WinArg> = stack_args.iter().map(|v| WinArg::Imm(*v as u32)).collect();
        self.win_call_stack_mixed(name, &args);
    }

    fn win_call(&mut self, name: &str) {
        self.win_call_stack(name, &[]);
    }

    /// A call with a fifth argument of zero at `[rsp+32]`.
    fn win_call5_null(&mut self, name: &str) {
        self.win_call_stack(name, &[0]);
    }

    /// Call a `kernel32.dll` function with up to four arguments taken from the
    /// virtual registers `X0..X3`, which the code generator uses for ordinary
    /// helper calls. The result is left in `RAX`; pass it to
    /// [`Asm::win_result`].
    pub fn win_call_args(&mut self, name: &str, argc: usize) {
        const ARG_REGS: [u8; 4] = [RCX, RDX, R8, R9];
        const VREGS: [u8; 4] = [X0, X1, X2, X3];
        for i in 0..argc.min(ARG_REGS.len()) {
            self.load_vreg(ARG_REGS[i], VREGS[i]);
        }
        self.win_call_stack(name, &[]);
    }

    /// Move the Windows call's return value into `X0`.
    pub fn win_result(&mut self) {
        self.store_vreg(X0, RAX);
    }

    /// Call a function whose address was resolved into a data slot. This keeps
    /// the PE import table limited to kernel32 while still reaching user32.
    pub fn win_call_ptr(&mut self, slot: u64, argc: usize, stack_args: &[WinArg]) {
        self.flush_all();
        self.discard_all();
        const ARG_REGS: [u8; 4] = [RCX, RDX, R8, R9];
        const VREGS: [u8; 4] = [X0, X1, X2, X3];
        for i in 0..argc.min(ARG_REGS.len()) {
            self.load_vreg(ARG_REGS[i], VREGS[i]);
        }
        let frame = (32 + stack_args.len() * 8).div_ceil(16) * 16;
        self.u8(0x41);
        self.u8(0x54); // push r12
        self.u8(0x4C);
        self.u8(0x8B);
        self.u8(0xE4); // mov r12, rsp
        self.u8(0x48);
        self.u8(0x83);
        self.u8(0xE4);
        self.u8(0xF0); // and rsp, -16
        self.u8(0x48);
        self.u8(0x81);
        self.u8(0xEC);
        self.u32(frame as u32); // sub rsp, frame
        for (i, value) in stack_args.iter().enumerate() {
            let disp = (32 + i * 8) as u8;
            match value {
                WinArg::Imm(value) => {
                    self.u8(0x48);
                    self.u8(0xC7);
                    self.u8(0x44);
                    self.u8(0x24);
                    self.u8(disp);
                    self.u32(*value);
                }
                WinArg::Imm64(value) => {
                    self.mov_phys_imm64(R10, *value);
                    self.store_stack_reg(disp, R10);
                }
                WinArg::Reg(phys) => self.store_stack_reg(disp, *phys),
                WinArg::Vreg(reg) => {
                    self.load_vreg(R10, *reg);
                    self.store_stack_reg(disp, R10);
                }
            }
        }
        self.raw_call_slot(slot);
        self.u8(0x48);
        self.u8(0x81);
        self.u8(0xC4);
        self.u32(frame as u32); // add rsp, frame
        self.u8(0x4C);
        self.u8(0x89);
        self.u8(0xE4); // mov rsp, r12
        self.u8(0x41);
        self.u8(0x5C); // pop r12
    }

    // ---- raw instruction helpers for the GUI WndProc ---------------------

    fn raw_data_addr(&mut self, phys: u8, offset: u64) {
        self.mov_phys_imm64(phys, 0);
        self.fixups.push(Fixup::Data {
            at: self.code.len() - 8,
            offset,
        });
    }

    fn raw_mov_reg_mem(&mut self, dst: u8, base: u8) {
        let rex = 0x48 | (((dst >> 3) & 1) << 2) | ((base >> 3) & 1);
        self.u8(rex);
        self.u8(0x8B);
        self.u8(((dst & 7) << 3) | (base & 7));
    }

    fn raw_mov_mem_reg(&mut self, base: u8, src: u8) {
        let rex = 0x48 | (((src >> 3) & 1) << 2) | ((base >> 3) & 1);
        self.u8(rex);
        self.u8(0x89);
        self.u8(((src & 7) << 3) | (base & 7));
    }

    fn raw_load_data(&mut self, phys: u8, offset: u64) {
        let addr = if phys == RAX { R11 } else { RAX };
        self.raw_data_addr(addr, offset);
        self.raw_mov_reg_mem(phys, addr);
    }

    fn raw_store_data(&mut self, phys: u8, offset: u64) {
        let addr = if phys == RAX { R11 } else { RAX };
        self.raw_data_addr(addr, offset);
        self.raw_mov_mem_reg(addr, phys);
    }

    fn raw_store_imm64_data(&mut self, offset: u64, value: u64) {
        self.mov_phys_imm64(RAX, value);
        self.raw_store_data(RAX, offset);
    }

    fn raw_cmp_rax_imm32(&mut self, value: u32) {
        self.u8(0x48);
        self.u8(0x81);
        self.u8(0xF8);
        self.u32(value);
    }

    fn raw_jcc(&mut self, opcode: u8, label: &str) {
        self.u8(0x0F);
        self.u8(opcode);
        let at = self.code.len();
        self.u32(0);
        self.fixups.push(Fixup::LabelRel32 {
            at,
            next: 4,
            label: label.to_string(),
        });
    }

    fn raw_jmp(&mut self, label: &str) {
        self.u8(0xE9);
        let at = self.code.len();
        self.u32(0);
        self.fixups.push(Fixup::LabelRel32 {
            at,
            next: 4,
            label: label.to_string(),
        });
    }

    fn raw_call_slot(&mut self, slot: u64) {
        self.raw_data_addr(RAX, slot);
        self.raw_mov_reg_mem(RAX, RAX);
        self.u8(0xFF);
        self.u8(0xD0); // call rax
    }

    /// A tiny `WndProc` for the simple window class. It records button clicks
    /// and text changes in fixed slots, then forwards everything else to
    /// `DefWindowProcA`. Rusk callbacks stay in Rusk: the library reads the
    /// recorded events in its own loop.
    pub fn win_emit_wndproc(&mut self, label: &str) {
        self.label(label);
        let destroy = self.win_label("wnd_destroy");
        let command = self.win_label("wnd_command");
        let set_change_kind = self.win_label("wnd_change_kind");
        let store_event = self.win_label("wnd_store_event");
        let def = self.win_label("wnd_def");
        let done = self.win_label("wnd_done");

        self.u8(0x53); // push rbx
        self.u8(0x48);
        self.u8(0x83);
        self.u8(0xEC);
        self.u8(0x20); // sub rsp, 32 (shadow space, 16-byte aligned)

        self.raw_store_data(RCX, WIN_GUI_WNDPROC_ARG1_OFFSET);
        self.raw_store_data(RDX, WIN_GUI_WNDPROC_ARG2_OFFSET);
        self.raw_store_data(R8, WIN_GUI_WNDPROC_ARG3_OFFSET);
        self.raw_store_data(R9, WIN_GUI_WNDPROC_ARG4_OFFSET);

        // WM_DESTROY (0x0002)
        self.raw_load_data(RAX, WIN_GUI_WNDPROC_ARG2_OFFSET);
        self.raw_cmp_rax_imm32(0x0002);
        self.raw_jcc(0x84, &destroy); // je

        // WM_COMMAND (0x0111)
        self.raw_load_data(RAX, WIN_GUI_WNDPROC_ARG2_OFFSET);
        self.raw_cmp_rax_imm32(0x0111);
        self.raw_jcc(0x85, &command); // jne

        self.raw_jmp(&def);

        self.label(&destroy);
        self.mov_phys_imm64(RCX, 0);
        self.raw_call_slot(WIN_GUI_FN_POST_QUIT_MESSAGE);
        self.u8(0x31);
        self.u8(0xC0); // xor eax, eax
        self.raw_jmp(&done);

        self.label(&command);
        self.raw_store_imm64_data(WIN_GUI_EVENT_KIND_OFFSET, 1); // click
                                                                 // HIWORD(wParam) == 1 means EN_CHANGE for an edit control.
        self.raw_load_data(RAX, WIN_GUI_WNDPROC_ARG3_OFFSET);
        self.u8(0x48);
        self.u8(0xC1);
        self.u8(0xE8);
        self.u8(0x10); // shr rax, 16
        self.raw_store_data(RAX, WIN_GUI_TEMP_OFFSET);
        self.raw_load_data(RAX, WIN_GUI_TEMP_OFFSET);
        self.raw_cmp_rax_imm32(1);
        self.raw_jcc(0x84, &set_change_kind);
        self.raw_jmp(&store_event);

        self.label(&set_change_kind);
        self.raw_store_imm64_data(WIN_GUI_EVENT_KIND_OFFSET, 2); // change

        self.label(&store_event);
        self.raw_load_data(RAX, WIN_GUI_WNDPROC_ARG3_OFFSET);
        self.u8(0x0F);
        self.u8(0xB7);
        self.u8(0xC0); // movzx eax, ax (control id)
        self.raw_store_data(RAX, WIN_GUI_EVENT_CONTROL_OFFSET);
        self.raw_store_imm64_data(WIN_GUI_EVENT_VALUE_OFFSET, 0);
        self.raw_load_data(RAX, WIN_GUI_EVENT_COUNT_OFFSET);
        self.u8(0x48);
        self.u8(0x83);
        self.u8(0xC0);
        self.u8(0x01); // add rax, 1
        self.raw_store_data(RAX, WIN_GUI_EVENT_COUNT_OFFSET);
        self.u8(0x31);
        self.u8(0xC0); // xor eax, eax
        self.raw_jmp(&done);

        self.label(&def);
        self.raw_load_data(RCX, WIN_GUI_WNDPROC_ARG1_OFFSET);
        self.raw_load_data(RDX, WIN_GUI_WNDPROC_ARG2_OFFSET);
        self.raw_load_data(R8, WIN_GUI_WNDPROC_ARG3_OFFSET);
        self.raw_load_data(R9, WIN_GUI_WNDPROC_ARG4_OFFSET);
        self.raw_call_slot(WIN_GUI_FN_DEF_WINDOW_PROC);

        self.label(&done);
        self.u8(0x48);
        self.u8(0x83);
        self.u8(0xC4);
        self.u8(0x20); // add rsp, 32
        self.u8(0x5B); // pop rbx
        self.u8(0xC3); // ret
    }

    fn test_rax(&mut self) {
        self.u8(0x48);
        self.u8(0x85);
        self.u8(0xC0); // test rax, rax
    }

    fn win_exit(&mut self) {
        self.load_vreg(RCX, X0);
        self.win_call("ExitProcess");
        self.u8(0x0F);
        self.u8(0x0B); // ud2: ExitProcess never returns
    }

    fn win_write(&mut self) {
        // fd 1 -> stdout, fd 2 -> stderr, anything larger is a file handle
        // returned by CreateFileA. fd 0 is read-only.
        let stdout = self.win_label("stdout");
        let stderr = self.win_label("stderr");
        let get = self.win_label("get_std_handle");
        let file = self.win_label("write_file_handle");
        let have = self.win_label("write_have_handle");
        let done = self.win_label("write_done");
        let failed = self.win_label("write_failed");
        let store = self.win_label("write_store");

        self.load_vreg(R10, X0);
        self.cmp_phys_imm(R10, 1);
        self.bcond_label(CC_EQ, &stdout);
        self.cmp_phys_imm(R10, 2);
        self.bcond_label(CC_EQ, &stderr);
        self.cmp_phys_imm(R10, 3);
        self.bcond_label(CC_LT, &failed);
        self.b_label(&file);

        self.label(&stdout);
        self.mov_rcx_imm(-11); // STD_OUTPUT_HANDLE
        self.b_label(&get);

        self.label(&stderr);
        self.mov_rcx_imm(-12); // STD_ERROR_HANDLE

        self.label(&get);
        self.win_call("GetStdHandle");
        self.win_data_addr(RDX, WIN_HANDLE_OFFSET);
        self.mov_mem_rax(RDX);
        self.b_label(&have);

        self.label(&file);
        self.load_vreg(RAX, X0);
        self.win_data_addr(RDX, WIN_HANDLE_OFFSET);
        self.mov_mem_rax(RDX);

        self.label(&have);
        // args: RCX=handle, RDX=buffer, R8=length, R9=&written
        self.win_data_addr(RCX, WIN_HANDLE_OFFSET);
        self.load_mem_rcx_ptr(RCX);
        self.load_vreg(RDX, X1);
        self.load_vreg(R8, X2);
        self.win_data_addr(R9, WIN_WRITTEN_OFFSET);
        self.win_call5_null("WriteFile");

        self.u8(0x48);
        self.u8(0x85);
        self.u8(0xC0); // test rax, rax
        self.bcond_label(CC_EQ, &failed);
        self.win_data_addr(RAX, WIN_WRITTEN_OFFSET);
        self.load_mem_rax(RAX);
        self.b_label(&store);
        self.label(&failed);
        self.mov_rax_imm(-1);
        self.label(&store);
        self.store_vreg(X0, RAX);

        self.label(&done);
    }

    fn win_read(&mut self) {
        // fd 0 is the console's standard input; anything else is a file
        // handle returned by CreateFileA.
        let stdin = self.win_label("stdin");
        let have = self.win_label("read_have_handle");
        let failed = self.win_label("read_failed");
        let store = self.win_label("read_store");
        let done = self.win_label("read_done");

        self.load_vreg(R10, X0);
        self.cmp_phys_imm(R10, 0);
        self.bcond_label(CC_EQ, &stdin);
        self.load_vreg(RAX, X0);
        self.win_data_addr(RDX, WIN_HANDLE_OFFSET);
        self.mov_mem_rax(RDX);
        self.b_label(&have);

        self.label(&stdin);
        self.mov_rcx_imm(-10); // STD_INPUT_HANDLE
        self.win_call("GetStdHandle");
        self.win_data_addr(RDX, WIN_HANDLE_OFFSET);
        self.mov_mem_rax(RDX);

        self.label(&have);
        self.win_data_addr(RCX, WIN_HANDLE_OFFSET);
        self.load_mem_rcx_ptr(RCX);
        self.load_vreg(RDX, X1);
        self.load_vreg(R8, X2);
        self.win_data_addr(R9, WIN_READ_OFFSET);
        self.win_call5_null("ReadFile");

        self.u8(0x48);
        self.u8(0x85);
        self.u8(0xC0); // test rax, rax
        self.bcond_label(CC_EQ, &failed);
        self.win_data_addr(RAX, WIN_READ_OFFSET);
        self.load_mem_rax(RAX);
        self.b_label(&store);
        self.label(&failed);
        self.mov_rax_imm(-1);
        self.label(&store);
        self.store_vreg(X0, RAX);

        self.label(&done);
    }

    fn win_open(&mut self) {
        // CreateFileA(path, access, share, NULL, disposition, attrs, NULL).
        // Rusk only opens regular files for reading or writing; directory and
        // pipe flags stay unsupported and simply fail at the API.
        let readonly = self.win_label("open_readonly");
        let no_create = self.win_label("open_no_create");
        let ready = self.win_label("open_ready");
        let failed = self.win_label("open_failed");
        let store = self.win_label("open_store");

        // Access: GENERIC_READ, plus GENERIC_WRITE when O_WRONLY/O_RDWR is set.
        self.load_vreg(R10, X2);
        self.mov_phys_imm64(RDX, 0x8000_0000); // GENERIC_READ
        self.test_phys_imm(R10, 3); // O_WRONLY | O_RDWR
        self.bcond_label(CC_EQ, &readonly);
        self.mov_phys_imm64(RDX, 0xC000_0000); // GENERIC_READ | GENERIC_WRITE

        // Disposition: OPEN_EXISTING, OPEN_ALWAYS with O_CREAT, and
        // CREATE_ALWAYS when O_TRUNC is also set.
        self.label(&readonly);
        self.mov_rax_imm(3); // OPEN_EXISTING
        self.load_vreg(R10, X2);
        self.test_phys_imm(R10, 0x40); // O_CREAT
        self.bcond_label(CC_EQ, &no_create);
        self.mov_rax_imm(4); // OPEN_ALWAYS
        self.load_vreg(R10, X2);
        self.test_phys_imm(R10, 0x200); // O_TRUNC
        self.bcond_label(CC_EQ, &ready);
        self.mov_rax_imm(2); // CREATE_ALWAYS
        self.label(&ready);
        self.label(&no_create);

        // args: RCX=path, RDX=access, R8=share, R9=security, stack=rest.
        self.load_vreg(RCX, X1);
        self.mov_r8_imm(3); // FILE_SHARE_READ | FILE_SHARE_WRITE
        self.mov_r9_imm(0);
        self.win_call_stack_mixed(
            "CreateFileA",
            &[WinArg::Reg(RAX), WinArg::Imm(0x80), WinArg::Imm(0)],
        );

        self.cmp_rax_imm(-1); // INVALID_HANDLE_VALUE
        self.bcond_label(CC_EQ, &failed);
        self.test_rax();
        self.bcond_label(CC_EQ, &failed);
        self.b_label(&store);

        self.label(&failed);
        self.mov_rax_imm(-1);
        self.label(&store);
        self.store_vreg(X0, RAX);
    }

    fn win_close(&mut self) {
        let failed = self.win_label("close_failed");
        let done = self.win_label("close_done");
        self.load_vreg(RCX, X0);
        self.win_call("CloseHandle");
        self.test_rax();
        self.bcond_label(CC_EQ, &failed);
        self.mov_rax_imm(0);
        self.b_label(&done);
        self.label(&failed);
        self.mov_rax_imm(-1);
        self.label(&done);
        self.store_vreg(X0, RAX);
    }

    fn win_ioctl(&mut self) {
        // Linux's TCGETS/TCSETS pair becomes GetConsoleMode/SetConsoleMode.
        // `raw_terminal_on` calls TCGETS and then TCSETS, so the first TCSETS
        // enters raw mode and a later one restores the saved mode.
        let get = self.win_label("ioctl_get");
        let set = self.win_label("ioctl_set");
        let restore = self.win_label("ioctl_restore");
        let failed = self.win_label("ioctl_failed");
        let done = self.win_label("ioctl_done");

        self.load_vreg(R10, X1);
        self.cmp_phys_imm(R10, TCGETS as i32);
        self.bcond_label(CC_EQ, &get);
        self.cmp_phys_imm(R10, TCSETS as i32);
        self.bcond_label(CC_EQ, &set);
        self.b_label(&failed);

        self.label(&get);
        self.win_console_handle();
        self.win_data_addr(RCX, WIN_HANDLE_OFFSET);
        self.load_mem_rcx_ptr(RCX);
        self.win_data_addr(RDX, WIN_CONSOLE_SAVED_OFFSET);
        self.win_call("GetConsoleMode");
        self.test_rax();
        self.bcond_label(CC_EQ, &failed);
        self.mov_rax_imm(0);
        self.b_label(&done);

        self.label(&set);
        self.win_data_addr(RAX, WIN_CONSOLE_RAW_OFFSET);
        self.load_mem_rax(RAX);
        self.test_rax();
        self.bcond_label(CC_NE, &restore);

        // Raw mode: drop ENABLE_PROCESSED_INPUT, ENABLE_LINE_INPUT, and
        // ENABLE_ECHO_INPUT, and ask for VT input sequences.
        self.win_console_handle();
        self.win_data_addr(RAX, WIN_CONSOLE_SAVED_OFFSET);
        self.load_mem_rax(RAX);
        self.mov_phys_imm64(RCX, !0x7u64);
        self.and_phys_reg(RAX, RCX);
        self.mov_phys_imm64(RCX, 0x200); // ENABLE_VIRTUAL_TERMINAL_INPUT
        self.or_phys_reg(RAX, RCX);
        self.win_data_addr(RDX, WIN_CONSOLE_MODE_OFFSET);
        self.mov_mem_rax(RDX);
        self.win_data_addr(RCX, WIN_HANDLE_OFFSET);
        self.load_mem_rcx_ptr(RCX);
        self.win_data_addr(RDX, WIN_CONSOLE_MODE_OFFSET);
        self.load_mem_phys(RDX, RDX);
        self.win_call("SetConsoleMode");
        self.test_rax();
        self.bcond_label(CC_EQ, &failed);
        self.win_store_slot(WIN_CONSOLE_RAW_OFFSET, 1);
        self.mov_rax_imm(0);
        self.b_label(&done);

        self.label(&restore);
        self.win_console_handle();
        self.win_data_addr(RCX, WIN_HANDLE_OFFSET);
        self.load_mem_rcx_ptr(RCX);
        self.win_data_addr(RDX, WIN_CONSOLE_SAVED_OFFSET);
        self.load_mem_phys(RDX, RDX);
        self.win_call("SetConsoleMode");
        self.test_rax();
        self.bcond_label(CC_EQ, &failed);
        self.win_store_slot(WIN_CONSOLE_RAW_OFFSET, 0);
        self.mov_rax_imm(0);
        self.b_label(&done);

        self.label(&failed);
        self.mov_rax_imm(-1);
        self.label(&done);
        self.store_vreg(X0, RAX);
    }

    /// Load `STD_INPUT_HANDLE` into the handle slot.
    fn win_console_handle(&mut self) {
        self.mov_rcx_imm(-10);
        self.win_call("GetStdHandle");
        self.win_data_addr(RDX, WIN_HANDLE_OFFSET);
        self.mov_mem_rax(RDX);
    }

    fn win_store_slot(&mut self, offset: u64, value: i32) {
        self.mov_rax_imm(value);
        self.win_data_addr(RDX, offset);
        self.mov_mem_rax(RDX);
    }

    fn win_mmap(&mut self) {
        // VirtualAlloc(NULL, size, MEM_COMMIT|MEM_RESERVE, PAGE_READWRITE).
        // X0 (the requested address) is ignored; anonymous mappings choose
        // their own address.
        let ok = self.win_label("mmap_ok");
        self.u8(0x48);
        self.u8(0x31);
        self.u8(0xC9); // xor rcx, rcx
        self.load_vreg(RDX, X1);
        self.mov_r8_imm(0x3000); // MEM_COMMIT | MEM_RESERVE
        self.mov_r9_imm(0x04); // PAGE_READWRITE
        self.win_call("VirtualAlloc");
        // Linux code checks for a negative return. Windows returns NULL.
        self.u8(0x48);
        self.u8(0x85);
        self.u8(0xC0); // test rax, rax
        self.bcond_label(CC_NE, &ok);
        self.mov_rax_imm(-1);
        self.label(&ok);
        self.store_vreg(X0, RAX);
    }

    fn mov_rax_imm(&mut self, value: i32) {
        self.u8(0x48);
        self.u8(0xC7);
        self.u8(0xC0);
        self.u32(value as u32);
    }

    fn mov_rcx_imm(&mut self, value: i32) {
        self.u8(0x48);
        self.u8(0xC7);
        self.u8(0xC1);
        self.u32(value as u32);
    }

    fn mov_r8_imm(&mut self, value: i32) {
        self.u8(0x49);
        self.u8(0xC7);
        self.u8(0xC0);
        self.u32(value as u32);
    }

    fn mov_r9_imm(&mut self, value: i32) {
        self.u8(0x49);
        self.u8(0xC7);
        self.u8(0xC1);
        self.u32(value as u32);
    }

    fn cmp_rax_imm(&mut self, value: i32) {
        self.u8(0x48);
        self.u8(0x83);
        self.u8(0xF8);
        self.u8(value as u8);
    }

    /// `cmp r/m64, imm32` for any physical register. Used right before a
    /// `bcond_label`, whose flush only emits moves and preserves the flags.
    fn cmp_phys_imm(&mut self, phys: u8, value: i32) {
        self.u8(0x48 | ((phys >> 3) & 1));
        self.u8(0x81);
        self.u8(0xF8 | (phys & 7));
        self.u32(value as u32);
    }

    /// `test r/m64, imm32` for any physical register.
    fn test_phys_imm(&mut self, phys: u8, value: i32) {
        self.u8(0x48 | ((phys >> 3) & 1));
        self.u8(0xF7);
        self.u8(0xC0 | (phys & 7));
        self.u32(value as u32);
    }

    fn and_phys_reg(&mut self, dst: u8, src: u8) {
        let rex = 0x48 | (((src >> 3) & 1) << 2) | ((dst >> 3) & 1);
        self.u8(rex);
        self.u8(0x21);
        self.u8(0xC0 | ((src & 7) << 3) | (dst & 7));
    }

    fn or_phys_reg(&mut self, dst: u8, src: u8) {
        let rex = 0x48 | (((src >> 3) & 1) << 2) | ((dst >> 3) & 1);
        self.u8(rex);
        self.u8(0x09);
        self.u8(0xC0 | ((src & 7) << 3) | (dst & 7));
    }

    /// `mov dst, [base]` for arbitrary physical registers (base must not be
    /// RSP; the scratch slots are addressed through RAX/RCX/RDX).
    fn load_mem_phys(&mut self, dst: u8, base: u8) {
        let rex = 0x48 | (((dst >> 3) & 1) << 2) | ((base >> 3) & 1);
        self.u8(rex);
        self.u8(0x8B);
        self.u8(((dst & 7) << 3) | (base & 7));
    }

    fn mov_mem_rax(&mut self, base: u8) {
        self.u8(0x48 | ((base >> 3) & 1));
        self.u8(0x89);
        self.u8(base & 7);
    }

    fn load_mem_rax(&mut self, base: u8) {
        self.u8(0x48 | ((base >> 3) & 1));
        self.u8(0x8B);
        self.u8(base & 7);
    }

    fn load_mem_rcx_ptr(&mut self, base: u8) {
        self.u8(0x48 | ((base >> 3) & 1));
        self.u8(0x8B);
        self.u8(0x08 | (base & 7));
    }

    /// Emit the PE entry point. Windows hands a PE entry point no arguments,
    /// so the prologue calls `win_prepare` (emitted by the code generator) to
    /// build the synthetic `argc/argv/envp` table, then jumps into the shared
    /// `_start` path with an aligned stack.
    pub fn windows_start(&mut self, start: &str) {
        self.label("_pe_start");
        self.u8(0x48);
        self.u8(0x83);
        self.u8(0xE4);
        self.u8(0xF0); // and rsp, -16
        self.bl_label("win_prepare");
        self.b_label(start);
    }

    pub fn ret(&mut self) {
        self.flush_all();
        self.u8(0xC3);
    }

    // ---- finishing -------------------------------------------------------

    /// Resolve every fixup and return `(text, rodata)`.
    pub fn finish(self) -> (Vec<u8>, Vec<u8>) {
        self.finish_with_bss(0)
    }

    /// Resolve fixups with the final zeroed-data size. Windows needs it because
    /// its import table (and therefore each IAT slot's address) sits after the
    /// bss section.
    pub fn finish_with_bss(mut self, bss: u64) -> (Vec<u8>, Vec<u8>) {
        let mut rodata = Vec::new();
        let mut offsets = Vec::with_capacity(self.strings.len());
        for s in &self.strings {
            offsets.push(rodata.len());
            rodata.extend_from_slice(&(s.len() as u64).to_le_bytes());
            rodata.extend_from_slice(s);
        }

        let windows_layout = if self.windows {
            Some(pe::layout(
                self.code.len(),
                rodata.len(),
                bss,
                self.imports.len(),
            ))
        } else {
            None
        };
        let (string_base, data_base, label_base) = match windows_layout {
            Some(layout) => (
                pe::IMAGE_BASE + layout.rodata_rva,
                pe::IMAGE_BASE + layout.bss_rva,
                pe::IMAGE_BASE + layout.text_rva,
            ),
            None => (
                IMAGE_BASE + TEXT_OFFSET + self.code.len() as u64,
                crate::elf::data_base_for(self.code.len(), rodata.len()),
                IMAGE_BASE + TEXT_OFFSET,
            ),
        };
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
                Fixup::LabelRel32 { at, next, label } => {
                    let target = *labels
                        .get(label)
                        .unwrap_or_else(|| panic!("internal: branch to unknown label '{label}'"));
                    let delta = target as i64 - (*at as i64 + *next as i64);
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
                    let addr = label_base + target;
                    self.patch_u64(*at, addr);
                }
                Fixup::Data { at, offset } => {
                    self.patch_u64(*at, data_base + offset);
                }
                Fixup::Vreg { at, reg } => {
                    let addr = data_base + VREG_OFFSET + (*reg as u64) * 8;
                    self.patch_u64(*at, addr);
                }
            }
        }

        if let Some(layout) = windows_layout {
            let import_count = self.imports.len();
            let fixups = std::mem::take(&mut self.import_fixups);
            for (at, index) in fixups {
                let slot =
                    pe::IMAGE_BASE + pe::iat_slot_rva_with_count(&layout, import_count, index);
                self.patch_u64(at, slot);
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

    /// Read the constant placed by `mov_imm64`, whichever encoding was used.
    fn imm_value(text: &[u8]) -> u64 {
        if text[0] == 0x48 {
            u64::from_le_bytes(text[2..10].try_into().unwrap())
        } else if text[0] == 0x41 {
            u32::from_le_bytes(text[3..7].try_into().unwrap()) as u64
        } else {
            u32::from_le_bytes(text[1..5].try_into().unwrap()) as u64
        }
    }

    /// True if the text contains the save-scratch / movabs / store / restore
    /// sequence used to write a virtual register back to its home slot.
    fn has_absolute_store(text: &[u8]) -> bool {
        text.windows(15).any(|w| {
            (w[0] == 0x50 || w[0] == 0x51)
                && w[1] == 0x48
                && w[2] == 0xB8
                && w[11] & 0xF8 == 0x48
                && w[12] == 0x89
                && w[13] & 0xC7 == 0
                && (w[14] == 0x58 || w[14] == 0x59)
        })
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
                imm_value(&text),
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
        assert_eq!(imm_value(&text), 0x52_0000);
    }

    #[test]
    fn uses_a_short_immediate_for_small_values() {
        let mut a = Asm::new();
        a.mov_imm64(X0, 42);
        let text = bytes(a);
        assert_eq!(&text[..5], &[0xB8, 42, 0, 0, 0]);
        assert!(text.len() < 10, "a full movabs would be ten bytes");
    }

    #[test]
    fn drops_a_move_to_the_same_register() {
        let mut a = Asm::new();
        a.mov_reg_reg(X0, X0);
        assert!(bytes(a).is_empty());
    }

    #[test]
    fn windows_write_becomes_a_kernel32_call() {
        let mut a = Asm::new_windows();
        a.mov_imm64(X0, 1);
        a.mov_imm64(X1, 0x1000);
        a.mov_imm64(X2, 4);
        a.mov_imm64(X8, AARCH64_WRITE);
        a.svc();

        assert_eq!(
            a.import_names(),
            vec!["GetStdHandle".to_string(), "WriteFile".to_string()]
        );
        let (text, _) = a.finish_with_bss(0x2000);
        assert!(!text.windows(2).any(|w| w == [0x0F, 0x05]), "no syscall");
        assert!(
            text.windows(3).any(|w| w == [0x4C, 0x8B, 0xE4]),
            "the Windows ABI frame should be present"
        );
        assert!(
            text.windows(2).any(|w| w == [0xFF, 0xD0]),
            "the import should be called through rax"
        );
    }

    #[test]
    fn windows_exit_imports_exitprocess() {
        let mut a = Asm::new_windows();
        a.mov_imm64(X0, 7);
        a.mov_imm64(X8, AARCH64_EXIT);
        a.svc();

        assert_eq!(a.import_names(), vec!["ExitProcess".to_string()]);
        let (text, _) = a.finish_with_bss(0);
        assert!(!text.windows(2).any(|w| w == [0x0F, 0x05]), "no syscall");
        assert!(
            text.windows(2).any(|w| w == [0x0F, 0x0B]),
            "ExitProcess never returns, so it is followed by ud2"
        );
    }

    #[test]
    fn windows_start_calls_the_prepare_thunk() {
        let mut a = Asm::new_windows();
        a.windows_start("_start");
        a.label("win_prepare");
        a.ret();
        a.label("_start");
        a.ret();

        assert!(a.import_names().is_empty());
        let (text, _) = a.finish_with_bss(0);
        assert_eq!(&text[0..4], &[0x48, 0x83, 0xE4, 0xF0], "align the stack");
        assert_eq!(text[4], 0xE8, "call win_prepare");
        assert!(!text.windows(2).any(|w| w == [0x0F, 0x05]), "no syscall");
    }

    #[test]
    fn windows_file_calls_become_kernel32_calls() {
        let mut a = Asm::new_windows();
        // openat, write, read, and close through handles.
        a.mov_imm64(X0, 0);
        a.mov_imm64(X1, 0x1000);
        a.mov_imm64(X2, 577);
        a.mov_imm64(X8, AARCH64_OPENAT);
        a.svc();
        a.mov_imm64(X0, 0x100);
        a.mov_imm64(X1, 0x2000);
        a.mov_imm64(X2, 5);
        a.mov_imm64(X8, AARCH64_WRITE);
        a.svc();
        a.mov_imm64(X0, 0x100);
        a.mov_imm64(X1, 0x2000);
        a.mov_imm64(X2, 5);
        a.mov_imm64(X8, AARCH64_READ);
        a.svc();
        a.mov_imm64(X0, 0x100);
        a.mov_imm64(X8, AARCH64_CLOSE);
        a.svc();

        assert_eq!(
            a.import_names(),
            vec![
                "CreateFileA".to_string(),
                "GetStdHandle".to_string(),
                "WriteFile".to_string(),
                "ReadFile".to_string(),
                "CloseHandle".to_string()
            ]
        );
        let (text, _) = a.finish_with_bss(0x3000);
        assert!(!text.windows(2).any(|w| w == [0x0F, 0x05]), "no syscall");
        assert!(
            text.windows(2).any(|w| w == [0xFF, 0xD0]),
            "imports are called through rax"
        );
    }

    #[test]
    fn windows_raw_terminal_uses_console_mode() {
        let mut a = Asm::new_windows();
        a.mov_imm64(X0, 0);
        a.mov_imm64(X1, TCGETS);
        a.mov_imm64(X8, AARCH64_IOCTL);
        a.svc();
        a.mov_imm64(X0, 0);
        a.mov_imm64(X1, TCSETS);
        a.mov_imm64(X8, AARCH64_IOCTL);
        a.svc();

        assert_eq!(
            a.import_names(),
            vec![
                "GetStdHandle".to_string(),
                "GetConsoleMode".to_string(),
                "SetConsoleMode".to_string()
            ]
        );
        let (text, _) = a.finish_with_bss(0x2000);
        assert!(!text.windows(2).any(|w| w == [0x0F, 0x05]), "no syscall");
    }

    #[test]
    fn windows_unknown_syscalls_fall_back_to_the_stub() {
        let mut a = Asm::new_windows();
        a.mov_imm64(X8, AARCH64_SOCKET);
        a.svc();
        a.label("windows_unsupported");
        a.ret();

        assert!(a.import_names().is_empty());
        let (text, _) = a.finish_with_bss(0);
        // The fallback is a relative call to `windows_unsupported`.
        assert!(text.windows(1).any(|w| w == [0xE8]));
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
    fn straight_line_code_keeps_a_value_in_a_register() {
        let mut a = Asm::new();
        a.mov_imm64(X0, 5);
        a.add_imm(X0, X0, 1);
        let text = bytes(a);
        // No home-slot store between the two operations: the allocator kept
        // the value in a physical register.
        assert!(!has_absolute_store(&text));
    }

    #[test]
    fn a_branch_writes_cached_values_back() {
        let mut a = Asm::new();
        a.mov_imm64(X0, 5);
        a.b_label("end");
        a.label("end");
        a.ret();
        let text = bytes(a);
        // The branch flushes the cached value back to its home slot.
        assert!(has_absolute_store(&text));
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
