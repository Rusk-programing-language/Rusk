//! Walks the AST and emits machine code through the selected backend.
//!
//! Expressions are evaluated with a stack of temporaries: every expression
//! leaves its value in `X0`, binary operators push the left side, evaluate the
//! right side, then pop. Top-level variables live in fixed 8-byte slots above
//! the file image; a function's parameters and locals live in a stack frame
//! addressed from `X29`.
//!
//! Calling convention: arguments in `X0..X7`, result in `X0`, return address in
//! `X30`. The frame is:
//!
//! ```text
//! sp+0     saved X29
//! sp+8     saved X30
//! sp+16    parameter 0
//! sp+24    parameter 1 ...
//! ```
//!
//! No libc, no runtime: `say` is a `write(2)` syscall, `print_int` is a
//! hand-built itoa loop at the end of the text section, and the program ends
//! with `exit(2)`.

use crate::ast::{
    lambda_free_names, Base, BinOp, Elem, Expr, FuncTy, Param, Program, Stmt, Ty, UnOp,
};
use crate::backend::*;
use crate::types::Sig;
use std::collections::HashMap;

/// Offset of the itoa scratch buffer inside the data area.
const ITOA_OFFSET: u64 = 0x2000;
const ITOA_CAPACITY: u64 = 32;
/// Fixed slots in the zeroed data area for the heap allocator.
const HEAP_PTR_OFFSET: u64 = 0x1000;
const HEAP_END_OFFSET: u64 = 0x1008;
const HEAP_FREE_OFFSET: u64 = 0x1010;
const HEAP_CHUNKS_OFFSET: u64 = 0x1018;
/// The first heap chunk. More chunks are mapped when this one fills up.
const HEAP_CHUNK_SIZE: u64 = 0x10_0000;
/// Where the runtime keeps the current call depth.
const DEPTH_OFFSET: u64 = 0x1800;
/// Where the initial stack pointer (argc/argv/envp) is remembered.
const ARGS_SP_OFFSET: u64 = 0x1808;
/// Scratch space for turning Rusk text into a NUL-terminated path.
const PATH_OFFSET: u64 = 0x3000;
const PATH_CAPACITY: u64 = 4096;
/// A second path scratch area, for rename and other two-path calls.
const PATH2_OFFSET: u64 = 0x4000;
/// Scratch used to launch a child process: `/bin/sh`, `-c`, argv, status.
const RUN_SH_OFFSET: u64 = 0x5000;
const RUN_DASH_C_OFFSET: u64 = 0x5008;
const RUN_ARGV_OFFSET: u64 = 0x5010;
const RUN_STATUS_OFFSET: u64 = 0x5030;
/// Directory entries are read into this fixed buffer, one batch at a time.
const DENTS_OFFSET: u64 = 0x6000;
/// `SYS_read`
const SYS_READ: u64 = 63;
/// `SYS_openat`
const SYS_OPENAT: u64 = 56;
/// `SYS_close`
const SYS_CLOSE: u64 = 57;
/// `SYS_mkdirat`
const SYS_MKDIRAT: u64 = 34;
/// `SYS_unlinkat`
const SYS_UNLINKAT: u64 = 35;
/// `SYS_renameat`
const SYS_RENAMEAT: u64 = 38;
/// `SYS_getdents64`
const SYS_GETDENTS64: u64 = 61;
/// `SYS_faccessat`
const SYS_FACCESSAT: u64 = 48;
/// `SYS_clone`
const SYS_CLONE: u64 = 220;
/// `SYS_execve`
const SYS_EXECVE: u64 = 221;
/// `SYS_mmap`
const SYS_MMAP: u64 = 222;
/// `SYS_wait4`
const SYS_WAIT4: u64 = 260;
/// `AT_FDCWD`, as a two's-complement value.
const AT_FDCWD: u64 = (-100i64) as u64;
/// Directory entries fit in a modest buffer; getdents64 is called until it
/// reports no more.
const DENTS_CAPACITY: u64 = 8192;
/// Deepest call nesting the runtime will allow before it reports an error.
const MAX_DEPTH: u64 = 1000;
const HEAP_ERROR: &str = "burnt: the program ran out of memory\n";
const DEPTH_ERROR: &str = "burnt: the program recursed too deeply\n";
/// Frame slots start after the saved frame pointer and return address.
const FRAME_HEADER: u64 = 16;

#[derive(Clone)]
struct Var {
    slot: u64,
    ty: Ty,
    local: bool,
    constant: bool,
    /// Declared at the top level rather than inside a function.
    global: bool,
}

struct Frame {
    ret_label: String,
    ret_ty: Ty,
    next_slot: u64,
}

#[derive(Debug)]
pub struct Baked {
    pub text: Vec<u8>,
    pub rodata: Vec<u8>,
    pub bss: u64,
    pub target: Target,
}

pub fn compile(program: &Program) -> Result<Baked, String> {
    compile_for(program, Target::host())
}

pub fn compile_for(program: &Program, target: Target) -> Result<Baked, String> {
    let funcs = crate::types::check(program)?;
    crate::ownership::check(program, &funcs)?;
    let mut cg = Codegen::new(funcs, target);
    cg.heap_error = cg.intern(HEAP_ERROR);

    cg.asm.label("_start");
    // Remember where the kernel put argc, argv, and envp before using the stack.
    // `str` cannot name SP, so copy it through a scratch register first.
    cg.asm.mov_data_addr(X1, ARGS_SP_OFFSET);
    cg.asm.add_imm(X2, SP, 0);
    cg.asm.str(X2, X1, 0);
    // `run command` launches `/bin/sh -c`; the two words it needs never move.
    cg.asm.mov_data_addr(X1, RUN_SH_OFFSET);
    cg.asm.mov_imm64(X2, 0x0068_732f_6e69_622f); // "/bin/sh\0"
    cg.asm.mov_imm64(X3, 0x0000_0000_0000_632d); // "-c\0"
    cg.asm.str(X2, X1, 0);
    cg.asm.str(X3, X1, 8);
    // The heap is a list of mapped chunks; the first one is claimed now and
    // every allocation goes through `heap_alloc`, which reuses freed blocks.
    cg.asm.mov_data_addr(X1, HEAP_PTR_OFFSET);
    cg.asm.str(XZR, X1, 0);
    cg.asm.mov_data_addr(X1, HEAP_FREE_OFFSET);
    cg.asm.str(XZR, X1, 0);
    cg.asm.mov_data_addr(X1, HEAP_CHUNKS_OFFSET);
    cg.asm.str(XZR, X1, 0);
    cg.asm.mov_imm64(X0, HEAP_CHUNK_SIZE);
    cg.asm.bl_label("heap_grow");

    for stmt in &program.stmts {
        if matches!(stmt, Stmt::FuncDef { .. }) {
            continue;
        }
        cg.stmt(stmt)?;
    }
    // exit(0)
    cg.asm.mov_imm64(X8, SYS_EXIT);
    cg.asm.mov_imm64(X0, 0);
    cg.asm.svc();

    // Functions live after the exit, so top-level execution never falls in.
    for stmt in &program.stmts {
        if let Stmt::FuncDef {
            name, params, body, ..
        } = stmt
        {
            cg.function(name, params, body)?;
        }
    }

    // Closure bodies come next. A closure can create another closure, so keep
    // draining the queue. This happens before the list helpers so any nested
    // shapes the bodies copy or print are discovered in time.
    while !cg.pending_lambdas.is_empty() {
        let pending = cg.pending_lambdas.remove(0);
        cg.lambda_body(pending)?;
    }

    cg.emit_nested_helpers();

    cg.emit_print_int();
    cg.emit_text_concat();
    cg.emit_text_compare();
    cg.emit_text_letter();
    cg.emit_text_part();
    cg.emit_array_get();
    cg.emit_array_set();
    cg.emit_array_add();
    cg.emit_array_remove_last();
    cg.emit_array_remove_at();
    cg.emit_empty_text();
    cg.emit_cstr_copy();
    cg.emit_path_copy();
    cg.emit_arg_text();
    cg.emit_env_text();
    cg.emit_read_stream();
    cg.emit_read_file();
    cg.emit_write_file();
    cg.emit_make_directory();
    cg.emit_remove_path();
    cg.emit_rename_path();
    cg.emit_list_directory();
    cg.emit_file_exists();
    cg.emit_all_arguments();
    cg.emit_run_command();
    cg.emit_text_copy();
    cg.emit_array_copy_scalar();
    cg.emit_array_copy_text();
    cg.emit_heap_grow();
    cg.emit_heap_alloc();
    cg.emit_heap_free();
    cg.emit_heap_fail();
    cg.emit_runtime_fail();

    if cg.next_global * 8 > HEAP_PTR_OFFSET {
        return Err("this program has too many top-level things to keep track of".to_string());
    }

    let bss = cg.bss_size();
    let (text, rodata) = cg.asm.finish();
    Ok(Baked {
        text,
        rodata,
        bss,
        target,
    })
}

struct Codegen {
    asm: Asm,
    vars: HashMap<String, Var>,
    funcs: HashMap<String, Sig>,
    frame: Option<Frame>,
    next_global: u64,
    label_counter: u64,
    strings: HashMap<String, usize>,
    /// The source line of the statement being compiled, for runtime errors.
    line: usize,
    /// Interned sentence printed when the heap runs out of room.
    heap_error: usize,
    /// Labels for the loops we are inside, innermost last.
    loops: Vec<LoopLabels>,
    /// Nested list shapes whose recursive copy/print helpers are needed.
    copy_shapes: Vec<Elem>,
    show_shapes: Vec<Elem>,
    /// Closure bodies waiting to be emitted, in discovery order.
    pending_lambdas: Vec<PendingLambda>,
}

#[derive(Clone)]
struct LoopLabels {
    /// Where `break` jumps: the instruction after the loop.
    after: String,
    /// Where `continue` jumps: this loop's next-turn label.
    next: String,
}

/// A closure body is emitted after the program and its named functions, so its
/// record can be built while the normal expression walk is running.
struct PendingLambda {
    label: String,
    ret_label: String,
    params: Vec<Param>,
    body: Vec<Stmt>,
    /// Captured names with the kind they had in the enclosing scope. The
    /// enclosing locals are gone by the time this body is emitted.
    captures: Vec<(String, Ty)>,
    ret: Ty,
}

impl Codegen {
    fn new(funcs: HashMap<String, Sig>, target: Target) -> Self {
        Codegen {
            asm: Asm::new(target),
            vars: HashMap::new(),
            funcs,
            frame: None,
            next_global: 0,
            label_counter: 0,
            strings: HashMap::new(),
            line: 0,
            heap_error: 0,
            loops: Vec::new(),
            copy_shapes: Vec::new(),
            show_shapes: Vec::new(),
            pending_lambdas: Vec::new(),
        }
    }

    fn new_label(&mut self, hint: &str) -> String {
        self.label_counter += 1;
        format!("{hint}_{}", self.label_counter)
    }

    fn shape_tag(elem: &Elem) -> String {
        elem.tag()
    }

    fn show_array_label(&mut self, elem: &Elem) -> String {
        self.ensure_show_shape(elem.clone());
        format!("show_array_{}", Self::shape_tag(elem))
    }

    fn ensure_show_shape(&mut self, elem: Elem) {
        if self.show_shapes.contains(&elem) {
            return;
        }
        let inner = elem.inner();
        self.show_shapes.push(elem);
        if let Some(inner) = inner {
            self.ensure_show_shape(inner);
        }
    }

    /// The routine that deep-copies a list whose items have this shape.
    /// A depth of 0 means the items are plain values.
    fn list_copy_label(&mut self, elem: &Elem) -> String {
        match (&elem.func(), elem.depth()) {
            (None, 0) => match elem.base() {
                Base::Int | Base::Bool => "array_copy_scalar".to_string(),
                Base::Str => "array_copy_text".to_string(),
            },
            _ => {
                self.ensure_copy_shape(elem.clone());
                format!("array_copy_{}", Self::shape_tag(elem))
            }
        }
    }

    fn ensure_copy_shape(&mut self, elem: Elem) {
        let inner = elem.inner();
        if inner.is_none() || self.copy_shapes.contains(&elem) {
            return;
        }
        self.copy_shapes.push(elem);
        self.ensure_copy_shape(inner.unwrap());
    }

    fn bss_size(&self) -> u64 {
        (self.next_global * 8)
            .max(ITOA_OFFSET + ITOA_CAPACITY)
            .max(PATH_OFFSET + PATH_CAPACITY)
            .max(PATH2_OFFSET + PATH_CAPACITY)
            .max(RUN_STATUS_OFFSET + 8)
            .max(DENTS_OFFSET + DENTS_CAPACITY)
            .max(crate::x86_64::VREG_OFFSET + crate::x86_64::VREG_OFFSET_SLACK)
    }

    /// Reserve an 8-byte slot, in the current frame or at top level.
    fn alloc_var(&mut self, ty: Ty, constant: bool) -> Var {
        if let Some(frame) = self.frame.as_mut() {
            let slot = frame.next_slot;
            frame.next_slot += 1;
            Var {
                slot,
                ty,
                local: true,
                constant,
                global: false,
            }
        } else {
            let slot = self.next_global;
            self.next_global += 1;
            Var {
                slot,
                ty,
                local: false,
                constant,
                global: true,
            }
        }
    }

    fn alloc_int_slot(&mut self) -> Var {
        self.alloc_var(Ty::Int, false)
    }

    fn intern(&mut self, s: &str) -> usize {
        if let Some(id) = self.strings.get(s) {
            return *id;
        }
        let id = self.asm.add_string(s);
        self.strings.insert(s.to_string(), id);
        id
    }

    /// The sentence a runtime failure should print, tied to the current line.
    fn runtime_message(&self, what: &str) -> String {
        format!("burnt: line {}: {what}\n", self.line)
    }

    fn store_var(&mut self, var: &Var) {
        if var.local {
            self.asm.str(X0, X29, frame_offset(var.slot));
        } else {
            self.asm.mov_data_addr(X1, var.slot * 8);
            self.asm.str(X0, X1, 0);
        }
    }

    fn load_var(&mut self, var: &Var) {
        if var.local {
            self.asm.ldr(X0, X29, frame_offset(var.slot));
        } else {
            self.asm.mov_data_addr(X1, var.slot * 8);
            self.asm.ldr(X0, X1, 0);
        }
    }

    // ---- statements ------------------------------------------------------

    fn stmt(&mut self, stmt: &Stmt) -> Result<(), String> {
        self.line = stmt.line();
        match stmt {
            Stmt::Say(e, line) => {
                let ty = self.expr(e).map_err(|m| at(*line, m))?;
                if ty == Ty::Nothing {
                    return Err(at(
                        *line,
                        "that function doesn't give anything back, so there's nothing to say"
                            .to_string(),
                    ));
                }
                self.emit_say(ty);
                Ok(())
            }
            Stmt::Let { name, value, line } => {
                self.declare(name, value, false).map_err(|m| at(*line, m))
            }
            Stmt::Const { name, value, line } => {
                self.declare(name, value, true).map_err(|m| at(*line, m))
            }
            Stmt::Set { name, value, line } => self.set(name, value).map_err(|m| at(*line, m)),
            Stmt::SetItem {
                index,
                array,
                value,
                line,
            } => self.set_item(index, array, value).map_err(|m| at(*line, m)),
            Stmt::AddTo { value, name, line } => self.add_to(value, name).map_err(|m| at(*line, m)),
            Stmt::Remove { index, name, line } => {
                self.remove(index.as_ref(), name).map_err(|m| at(*line, m))
            }
            Stmt::Warn(e, line) => {
                let ty = self.expr(e).map_err(|m| at(*line, m))?;
                if ty == Ty::Nothing {
                    return Err(at(
                        *line,
                        "that function doesn't give anything back, so there's nothing to warn about"
                            .to_string(),
                    ));
                }
                self.emit_show(ty, 2);
                Ok(())
            }
            Stmt::Stop(e, line) => {
                let ty = self.expr(e).map_err(|m| at(*line, m))?;
                if ty != Ty::Int {
                    return Err(at(
                        *line,
                        format!("'stop with' needs an exit code as a number, but this is {ty}"),
                    ));
                }
                self.asm.mov_imm64(X8, SYS_EXIT);
                self.asm.svc();
                Ok(())
            }
            Stmt::WriteFile { text, path, line } => {
                self.write_file(text, path).map_err(|m| at(*line, m))
            }
            Stmt::MakeDirectory { path, line } => {
                self.make_directory(path).map_err(|m| at(*line, m))
            }
            Stmt::RemoveFile { path, line } => self.remove_path(path, 0).map_err(|m| at(*line, m)),
            Stmt::RemoveDirectory { path, line } => {
                self.remove_path(path, 0x200).map_err(|m| at(*line, m))
            }
            Stmt::RenamePath { from, to, line } => {
                self.rename_path(from, to).map_err(|m| at(*line, m))
            }
            Stmt::If {
                cond,
                then,
                other,
                line,
            } => self.if_(cond, then, other).map_err(|m| at(*line, m)),
            Stmt::Repeat { count, body, line } => {
                self.repeat(count, body).map_err(|m| at(*line, m))
            }
            Stmt::While { cond, body, line } => self.while_(cond, body).map_err(|m| at(*line, m)),
            Stmt::ForEach {
                name,
                from,
                to,
                body,
                line,
            } => self
                .for_each(name, from, to, body)
                .map_err(|m| at(*line, m)),
            Stmt::ForEachIn {
                name,
                array,
                body,
                line,
            } => self
                .for_each_in(name, array, body)
                .map_err(|m| at(*line, m)),
            Stmt::GiveBack(e, line) => self.give_back(e).map_err(|m| at(*line, m)),
            Stmt::Break(line) => match self.loops.last() {
                Some(labels) => {
                    let after = labels.after.clone();
                    self.asm.b_label(&after);
                    Ok(())
                }
                None => Err(at(*line, "'break' only works inside a loop".to_string())),
            },
            Stmt::Continue(line) => match self.loops.last() {
                Some(labels) => {
                    let next = labels.next.clone();
                    self.asm.b_label(&next);
                    Ok(())
                }
                None => Err(at(*line, "'continue' only works inside a loop".to_string())),
            },
            Stmt::Expr(e, line) => self.expr(e).map(|_| ()).map_err(|m| at(*line, m)),
            Stmt::FuncDef { line, .. } => Err(at(
                *line,
                "functions have to live at the top level".to_string(),
            )),
            Stmt::Import { line, .. } => Err(at(
                *line,
                "this import wasn't resolved — imports only work at the top level".to_string(),
            )),
            Stmt::Export { line, .. } => Err(at(
                *line,
                "this export wasn't resolved — exports only work at the top level".to_string(),
            )),
        }
    }

    fn declare(&mut self, name: &str, value: &Expr, constant: bool) -> Result<(), String> {
        // Inside a function, a local may shadow a global. Two locals with the
        // same name are still an error.
        if self
            .vars
            .get(name)
            .is_some_and(|old| !(old.global && self.frame.is_some()))
        {
            return Err(format!(
                "'{name}' already exists — use 'set {name} to ...' to change it"
            ));
        }
        let ty = self.expr(value)?;
        let var = self.alloc_var(ty, constant);
        self.store_var(&var);
        self.vars.insert(name.to_string(), var);
        Ok(())
    }

    fn set(&mut self, name: &str, value: &Expr) -> Result<(), String> {
        let Some(var) = self.vars.get(name).cloned() else {
            return Err(format!(
                "I don't know a thing called '{name}' — declare it with 'let {name} be ...' first"
            ));
        };
        if var.constant {
            return Err(format!(
                "'{name}' is a constant — it can't be changed with 'set'"
            ));
        }
        let ty = self.expr(value)?;
        if ty != var.ty {
            return Err(format!(
                "'{name}' holds {}, but that's {ty} — Rusk won't change a thing's kind",
                var.ty
            ));
        }
        self.store_var(&var);
        Ok(())
    }

    fn set_item(&mut self, index: &Expr, array: &Expr, value: &Expr) -> Result<(), String> {
        let it = self.expr(index)?;
        if it != Ty::Int {
            return Err(format!(
                "the item number must be a number, but this is {it}"
            ));
        }
        self.asm.push_reg(X0);
        let at = self.expr(array)?;
        let Some(elem) = at.elem() else {
            return Err(format!("'set item' needs a list, but this is {at}"));
        };
        self.asm.push_reg(X0);
        let vt = self.expr(value)?;
        if vt != elem.ty() {
            return Err(format!(
                "this list holds {}, but the new item is {vt}",
                elem.ty()
            ));
        }
        self.asm.push_reg(X0);
        self.asm.pop_reg(X2); // value
        self.asm.pop_reg(X1); // array
        self.asm.pop_reg(X0); // index
        let msg = self.runtime_message("an item outside the list");
        let id = self.intern(&msg);
        self.asm.mov_abs(X4, id);
        self.asm.bl_label("array_set");
        Ok(())
    }

    fn add_to(&mut self, value: &Expr, name: &str) -> Result<(), String> {
        let Some(var) = self.vars.get(name).cloned() else {
            return Err(format!("I don't know a thing called '{name}'"));
        };
        if var.constant {
            return Err(format!("'{name}' is a constant — it can't be grown"));
        }
        let Some(elem) = var.ty.elem() else {
            return Err(format!("'add' needs a list, but '{name}' holds {}", var.ty));
        };
        let ty = self.expr(value)?;
        if ty != elem.ty() {
            return Err(format!(
                "this list holds {}, but the new item is {ty}",
                elem.ty()
            ));
        }
        self.asm.push_reg(X0);
        self.load_var(&var);
        self.asm.pop_reg(X1);
        self.asm.bl_label("array_add");
        self.store_var(&var);
        Ok(())
    }

    fn remove(&mut self, index: Option<&Expr>, name: &str) -> Result<(), String> {
        let Some(var) = self.vars.get(name).cloned() else {
            return Err(format!("I don't know a thing called '{name}'"));
        };
        if var.constant {
            return Err(format!("'{name}' is a constant — it can't be changed"));
        }
        if var.ty.elem().is_none() {
            return Err(format!(
                "'remove' needs a list, but '{name}' holds {}",
                var.ty
            ));
        }

        match index {
            Some(index) => {
                let it = self.expr(index)?;
                if it != Ty::Int {
                    return Err(format!(
                        "the item number must be a number, but this is {it}"
                    ));
                }
                self.asm.push_reg(X0);
                self.load_var(&var);
                self.asm.mov_reg_reg(X1, X0);
                self.asm.pop_reg(X0);
                let msg = self.runtime_message("an item outside the list");
                let id = self.intern(&msg);
                self.asm.mov_abs(X4, id);
                self.asm.bl_label("array_remove_at");
            }
            None => {
                self.load_var(&var);
                let msg = self.runtime_message("there's nothing to remove");
                let id = self.intern(&msg);
                self.asm.mov_abs(X4, id);
                self.asm.bl_label("array_remove_last");
            }
        }
        self.store_var(&var);
        Ok(())
    }

    fn write_file(&mut self, text: &Expr, path: &Expr) -> Result<(), String> {
        let tt = self.expr(text)?;
        if tt != Ty::Str {
            return Err(format!(
                "'write ... to file' needs the text as text, but this is {tt}"
            ));
        }
        self.asm.push_reg(X0);
        let pt = self.expr(path)?;
        if pt != Ty::Str {
            return Err(format!(
                "'write ... to file' needs the path as text, but this is {pt}"
            ));
        }
        self.asm.mov_reg_reg(X1, X0);
        self.asm.pop_reg(X0);
        self.asm.bl_label("write_file");
        Ok(())
    }

    fn path_expr(&mut self, expr: &Expr, what: &str) -> Result<(), String> {
        let ty = self.expr(expr)?;
        if ty != Ty::Str {
            return Err(format!("'{what}' needs a path as text, but this is {ty}"));
        }
        Ok(())
    }

    fn make_directory(&mut self, path: &Expr) -> Result<(), String> {
        self.path_expr(path, "make directory")?;
        self.asm.bl_label("make_directory");
        Ok(())
    }

    fn remove_path(&mut self, path: &Expr, flags: u64) -> Result<(), String> {
        self.path_expr(path, "remove")?;
        self.asm.mov_imm64(X1, flags);
        self.asm.bl_label("remove_path");
        Ok(())
    }

    fn rename_path(&mut self, from: &Expr, to: &Expr) -> Result<(), String> {
        let from_ty = self.expr(from)?;
        if from_ty != Ty::Str {
            return Err(format!(
                "'rename' needs the old path as text, but this is {from_ty}"
            ));
        }
        self.asm.push_reg(X0);
        let to_ty = self.expr(to)?;
        if to_ty != Ty::Str {
            return Err(format!(
                "'rename' needs the new path as text, but this is {to_ty}"
            ));
        }
        self.asm.mov_reg_reg(X1, X0);
        self.asm.pop_reg(X0);
        self.asm.bl_label("rename_path");
        Ok(())
    }

    fn if_(&mut self, cond: &Expr, then: &[Stmt], other: &[Stmt]) -> Result<(), String> {
        let ty = self.expr(cond)?;
        if ty != Ty::Bool {
            return Err(format!(
                "'if' needs a condition that is true or false, but this is {ty}"
            ));
        }
        let l_else = self.new_label("else");
        let l_end = self.new_label("endif");
        self.asm.cmp_reg_reg(X0, XZR);
        self.asm.bcond_label(CC_EQ, &l_else);
        for s in then {
            self.stmt(s)?;
        }
        self.asm.b_label(&l_end);
        self.asm.label(&l_else);
        for s in other {
            self.stmt(s)?;
        }
        self.asm.label(&l_end);
        Ok(())
    }

    fn repeat(&mut self, count: &Expr, body: &[Stmt]) -> Result<(), String> {
        let ty = self.expr(count)?;
        if ty != Ty::Int {
            return Err(format!("'repeat' needs a number, but the count is {ty}"));
        }
        // The counter is local to the current function or to the program.
        let counter = self.alloc_int_slot();
        self.store_var(&counter);

        let l_loop = self.new_label("loop");
        let l_next = self.new_label("next");
        let l_end = self.new_label("after");
        self.loops.push(LoopLabels {
            after: l_end.clone(),
            next: l_next.clone(),
        });
        self.asm.label(&l_loop);
        self.load_var(&counter);
        self.asm.cmp_reg_reg(X0, XZR);
        self.asm.bcond_label(CC_LE, &l_end);
        for s in body {
            self.stmt(s)?;
        }
        self.asm.label(&l_next);
        self.load_var(&counter);
        self.asm.mov_imm64(X2, 1);
        self.asm.sub_reg_reg(X0, X0, X2);
        self.store_var(&counter);
        self.asm.b_label(&l_loop);
        self.loops.pop();
        self.asm.label(&l_end);
        Ok(())
    }

    fn while_(&mut self, cond: &Expr, body: &[Stmt]) -> Result<(), String> {
        let l_loop = self.new_label("while");
        let l_end = self.new_label("after");
        self.loops.push(LoopLabels {
            after: l_end.clone(),
            next: l_loop.clone(),
        });
        self.asm.label(&l_loop);
        let ty = self.expr(cond)?;
        if ty != Ty::Bool {
            return Err(format!(
                "'while' needs a condition that is true or false, but this is {ty}"
            ));
        }
        self.asm.cmp_reg_reg(X0, XZR);
        self.asm.bcond_label(CC_EQ, &l_end);
        for s in body {
            self.stmt(s)?;
        }
        self.asm.b_label(&l_loop);
        self.loops.pop();
        self.asm.label(&l_end);
        Ok(())
    }

    fn for_each(
        &mut self,
        name: &str,
        from: &Expr,
        to: &Expr,
        body: &[Stmt],
    ) -> Result<(), String> {
        let ty = self.expr(from)?;
        if ty != Ty::Int {
            return Err(format!(
                "the start of a 'for each' range must be a number, but this is {ty}"
            ));
        }
        let item = self.alloc_int_slot();
        self.store_var(&item);

        let ty = self.expr(to)?;
        if ty != Ty::Int {
            return Err(format!(
                "the end of a 'for each' range must be a number, but this is {ty}"
            ));
        }
        let limit = self.alloc_int_slot();
        self.store_var(&limit);

        // The loop variable only exists inside the body; put back whatever the
        // name meant before.
        let previous = self.vars.insert(name.to_string(), item.clone());

        let l_loop = self.new_label("each");
        let l_next = self.new_label("next");
        let l_end = self.new_label("after");
        self.loops.push(LoopLabels {
            after: l_end.clone(),
            next: l_next.clone(),
        });
        self.asm.label(&l_loop);
        self.load_var(&item);
        self.asm.mov_reg_reg(X2, X0);
        self.load_var(&limit);
        self.asm.cmp_reg_reg(X2, X0);
        self.asm.bcond_label(CC_GT, &l_end);
        for s in body {
            self.stmt(s)?;
        }
        self.asm.label(&l_next);
        self.load_var(&item);
        self.asm.mov_imm64(X2, 1);
        self.asm.add_reg_reg(X0, X0, X2);
        self.store_var(&item);
        self.asm.b_label(&l_loop);
        self.loops.pop();
        self.asm.label(&l_end);

        match previous {
            Some(var) => {
                self.vars.insert(name.to_string(), var);
            }
            None => {
                self.vars.remove(name);
            }
        }
        Ok(())
    }

    fn for_each_in(&mut self, name: &str, array: &Expr, body: &[Stmt]) -> Result<(), String> {
        let at = self.expr(array)?;
        let Some(elem) = at.elem() else {
            return Err(format!("'for each ... in' needs a list, but this is {at}"));
        };

        let array_var = self.alloc_var(at, false);
        self.store_var(&array_var);
        let index = self.alloc_int_slot();
        self.asm.mov_imm64(X0, 0);
        self.store_var(&index);
        let item = self.alloc_var(elem.ty(), false);

        let previous = self.vars.insert(name.to_string(), item.clone());

        let l_loop = self.new_label("each_in");
        let l_next = self.new_label("next");
        let l_end = self.new_label("after");
        self.loops.push(LoopLabels {
            after: l_end.clone(),
            next: l_next.clone(),
        });
        self.asm.label(&l_loop);
        self.load_var(&index);
        self.asm.mov_reg_reg(X2, X0);
        self.load_var(&array_var);
        self.asm.ldr(X3, X0, 0); // length
        self.asm.cmp_reg_reg(X2, X3);
        self.asm.bcond_label(CC_GE, &l_end);

        // array_get takes a 1-based index; array_get is used even though the
        // loop index is always valid, which keeps one bounds check in one place.
        // Keep the index in X2: loading a variable may use X0 and X1.
        self.load_var(&index);
        self.asm.mov_imm64(X2, 1);
        self.asm.add_reg_reg(X2, X0, X2);
        self.load_var(&array_var);
        self.asm.mov_reg_reg(X1, X0);
        self.asm.mov_reg_reg(X0, X2);
        let msg = self.runtime_message("an item outside the list");
        let id = self.intern(&msg);
        self.asm.mov_abs(X4, id);
        self.asm.bl_label("array_get");
        self.store_var(&item);

        for s in body {
            self.stmt(s)?;
        }

        self.asm.label(&l_next);
        self.load_var(&index);
        self.asm.mov_imm64(X2, 1);
        self.asm.add_reg_reg(X0, X0, X2);
        self.store_var(&index);
        self.asm.b_label(&l_loop);
        self.loops.pop();
        self.asm.label(&l_end);

        match previous {
            Some(var) => {
                self.vars.insert(name.to_string(), var);
            }
            None => {
                self.vars.remove(name);
            }
        }
        Ok(())
    }

    fn give_back(&mut self, e: &Expr) -> Result<(), String> {
        let Some(frame) = &self.frame else {
            return Err("'give back' only works inside a function".to_string());
        };
        let ret_ty = frame.ret_ty.clone();
        let ret_label = frame.ret_label.clone();
        let ty = self.expr(e)?;
        if ty != ret_ty {
            return Err(format!(
                "this function gives back {ret_ty}, but here it's {ty}"
            ));
        }
        self.asm.b_label(&ret_label);
        Ok(())
    }

    // ---- functions -------------------------------------------------------

    fn function(&mut self, name: &str, params: &[Param], body: &[Stmt]) -> Result<(), String> {
        let Some(sig) = self.funcs.get(name).cloned() else {
            return Err(format!("internal: no signature for '{name}'"));
        };
        let label = format!("fn_{name}");
        let ret_label = format!("ret_{name}");
        self.emit_body(&label, &ret_label, params, sig.ret, body, &[], Some(name))
    }

    fn lambda_body(&mut self, pending: PendingLambda) -> Result<(), String> {
        self.emit_body(
            &pending.label,
            &pending.ret_label,
            &pending.params,
            pending.ret,
            &pending.body,
            &pending.captures,
            None,
        )
    }

    /// Emit one function body. Named functions and closures share this frame:
    /// slot 0 holds the closure environment pointer, parameters follow, then
    /// any captured values, then ordinary locals.
    #[allow(clippy::too_many_arguments)]
    fn emit_body(
        &mut self,
        label: &str,
        ret_label: &str,
        params: &[Param],
        ret_ty: Ty,
        body: &[Stmt],
        captures: &[(String, Ty)],
        name: Option<&str>,
    ) -> Result<(), String> {
        let what = name.unwrap_or("a function value");
        if params.len() > crate::types::MAX_PARAMS {
            return Err(format!(
                "'{what}' has too many parameters ({} at most)",
                crate::types::MAX_PARAMS
            ));
        }

        let slots = 1 + params.len() as u64 + captures.len() as u64 + count_slots(body);
        let frame_size = FRAME_HEADER + slots * 8;
        let frame_size = frame_size.div_ceil(16) * 16;
        if frame_size > 4000 {
            return Err(format!("'{what}' has too many things to keep track of"));
        }

        // The function body starts with the top-level names in view, so it can
        // read and change globals. Parameters and locals are inserted on top
        // and shadow any global with the same name.
        let saved_vars = std::mem::take(&mut self.vars);
        self.vars = saved_vars.clone();
        let saved_frame = self.frame.take();
        self.frame = Some(Frame {
            ret_label: ret_label.to_string(),
            ret_ty,
            next_slot: 1 + params.len() as u64,
        });

        self.asm.label(label);
        self.asm.sub_imm(SP, SP, frame_size as u32);
        self.asm.str(X29, SP, 0);
        self.asm.str(X30, SP, 8);
        self.asm.add_imm(X29, SP, 0);

        // Refuse to let recursion run into the process stack. The counter is
        // global, so a deeply nested call reports a Rusk error instead of a
        // segmentation fault.
        let depth_ok = self.new_label("depth_ok");
        self.asm.mov_data_addr(X9, DEPTH_OFFSET);
        self.asm.ldr(X10, X9, 0);
        self.asm.mov_imm64(X11, MAX_DEPTH);
        self.asm.cmp_reg_reg(X10, X11);
        self.asm.bcond_label(CC_LT, &depth_ok);
        let id = self.intern(DEPTH_ERROR);
        self.asm.mov_abs(X0, id);
        self.asm.b_label("runtime_fail");
        self.asm.label(&depth_ok);
        self.asm.add_imm(X10, X10, 1);
        self.asm.str(X10, X9, 0);

        // Slot 0 is the environment pointer a closure body reads captures
        // through. Named functions are called with it zeroed.
        self.asm.str(X17, X29, frame_offset(0));

        for (i, p) in params.iter().enumerate() {
            self.vars.insert(
                p.name.clone(),
                Var {
                    slot: i as u64 + 1,
                    ty: p.ty.clone(),
                    local: true,
                    constant: false,
                    global: false,
                },
            );
            // Arguments arrive in a contiguous block pointed to by X18. Copy
            // each one into its frame slot so the body never depends on the
            // caller's stack.
            self.asm.ldr(X0, X18, (i * 8) as u32);
            self.asm.str(X0, X29, frame_offset(i as u64 + 1));
        }

        // Copy captures out of the environment record into ordinary frame
        // slots, so the rest of the body looks like any other function.
        for (i, (capture, ty)) in captures.iter().enumerate() {
            let slot = self.frame.as_mut().unwrap().next_slot;
            self.frame.as_mut().unwrap().next_slot += 1;
            self.asm.ldr(X0, X17, (i * 8) as u32);
            self.asm.str(X0, X29, frame_offset(slot));
            self.vars.insert(
                capture.clone(),
                Var {
                    slot,
                    ty: ty.clone(),
                    local: true,
                    constant: false,
                    global: false,
                },
            );
        }

        let mut result = Ok(());
        for stmt in body {
            if let Err(e) = self.stmt(stmt) {
                result = Err(e);
                break;
            }
        }

        // Falling off the end gives 0, so a procedure's result is harmless.
        self.asm.mov_imm64(X0, 0);
        self.asm.label(ret_label);
        // Give the depth counter back before tearing down the frame.
        self.asm.mov_data_addr(X1, DEPTH_OFFSET);
        self.asm.ldr(X2, X1, 0);
        self.asm.sub_imm(X2, X2, 1);
        self.asm.str(X2, X1, 0);
        self.asm.add_imm(SP, X29, 0);
        self.asm.ldr(X29, SP, 0);
        self.asm.ldr(X30, SP, 8);
        self.asm.add_imm(SP, SP, frame_size as u32);
        self.asm.ret();

        self.vars = saved_vars;
        self.frame = saved_frame;
        result
    }

    // ---- expressions -----------------------------------------------------

    /// The type checker's view of one expression in the current codegen scope.
    /// Closures need this to know what their body gives back before the body
    /// itself is emitted.
    fn expr_type(&self, e: &Expr) -> Result<Option<Ty>, String> {
        let vars: HashMap<String, Ty> = self
            .vars
            .iter()
            .map(|(name, var)| (name.clone(), var.ty.clone()))
            .collect();
        crate::types::type_of_with(e, &vars, &self.funcs)
    }

    /// Build `[code][capture count][capture...]` on the heap and leave its
    /// pointer in X0.
    fn emit_function_record(&mut self, code_label: &str, captures: &[String]) {
        let count = captures.len() as u64;
        let size = 16 + count * 8;
        self.asm.mov_imm64(X0, size);
        self.asm.bl_label("heap_alloc");
        self.asm.mov_reg_reg(X6, X0);
        self.asm.mov_label_addr(X7, code_label);
        self.asm.str(X7, X6, 0);
        self.asm.mov_imm64(X7, count);
        self.asm.str(X7, X6, 8);
        for (i, name) in captures.iter().enumerate() {
            let var = self.vars.get(name).cloned().expect("filtered capture");
            self.load_var(&var);
            self.asm.str(X0, X6, (16 + i * 8) as u32);
        }
        self.asm.mov_reg_reg(X0, X6);
    }

    /// Evaluate `args` into a contiguous block on the stack and point X18 at
    /// it. Returns the block's size in bytes, which the caller must add back
    /// to SP after the call.
    fn arg_block(&mut self, args: &[Expr]) -> Result<u32, String> {
        if args.is_empty() {
            return Ok(0);
        }
        if args.len() > crate::types::MAX_PARAMS {
            return Err(format!(
                "a call can pass at most {} things",
                crate::types::MAX_PARAMS
            ));
        }
        let bytes = args.len() * 8;
        let block = bytes.div_ceil(16) * 16;
        self.asm.sub_imm(SP, SP, block as u32);
        for (i, arg) in args.iter().enumerate() {
            self.expr(arg)?;
            self.asm.str(X0, SP, (i * 8) as u32);
        }
        // `mov_reg_reg` treats 31 as the zero register, so copy SP with `add`.
        self.asm.add_imm(X18, SP, 0);
        Ok(block as u32)
    }

    /// Call a function value: `[code][capture count][capture...]`. The code
    /// pointer goes in X14 and the environment pointer in X17.
    fn dynamic_call(&mut self, func: &Expr, args: &[Expr]) -> Result<Ty, String> {
        let ft = match self.expr(func)? {
            Ty::Func(ft) => ft,
            Ty::Nothing => {
                return Err("there's nothing here to call — that gives nothing back".to_string())
            }
            other => {
                return Err(format!(
                    "'call' needs a function value, but this is {other}"
                ));
            }
        };
        if args.len() != ft.arity() {
            return Err(format!(
                "this function value takes {} thing{}, but got {}",
                ft.arity(),
                if ft.arity() == 1 { "" } else { "s" },
                args.len()
            ));
        }
        // Keep the record above the argument block so evaluating the
        // arguments cannot overwrite it.
        self.asm.push_reg(X0);
        let block = self.arg_block(args)?;
        self.asm.ldr(X16, SP, block);
        self.asm.ldr(X14, X16, 0);
        self.asm.add_imm(X17, X16, 16);
        self.asm.call_reg(X14);
        if block > 0 {
            self.asm.add_imm(SP, SP, block);
        }
        self.asm.pop_reg(X16);
        Ok(ft.ret.clone())
    }

    fn expr(&mut self, e: &Expr) -> Result<Ty, String> {
        match e {
            Expr::Int(n) => {
                self.asm.mov_imm64(X0, *n as u64);
                Ok(Ty::Int)
            }
            Expr::Str(s) => {
                let id = self.intern(s);
                self.asm.mov_abs(X0, id);
                Ok(Ty::Str)
            }
            Expr::Bool(b) => {
                self.asm.mov_imm64(X0, u64::from(*b));
                Ok(Ty::Bool)
            }
            Expr::Var(name) => {
                if let Some(var) = self.vars.get(name).cloned() {
                    self.load_var(&var);
                    return Ok(var.ty);
                }
                if let Some(sig) = self.funcs.get(name).cloned() {
                    if sig.params.is_empty() {
                        self.asm.movz_reg(X17, 0);
                        self.asm.bl_label(&format!("fn_{name}"));
                        return Ok(sig.ret);
                    }
                    return Err(format!(
                        "'{name}' needs {} thing{} — call it with 'with ...'",
                        sig.params.len(),
                        if sig.params.len() == 1 { "" } else { "s" }
                    ));
                }
                Err(format!(
                    "I don't know a thing called '{name}' — declare it with 'let {name} be ...'"
                ))
            }
            Expr::Call(name, args) => {
                let Some(sig) = self.funcs.get(name).cloned() else {
                    // A variable holding a function value can be called the
                    // ordinary way too.
                    if let Some(var) = self.vars.get(name).cloned() {
                        if matches!(var.ty, Ty::Func(_)) {
                            let func = Expr::Var(name.clone());
                            return self.dynamic_call(&func, args);
                        }
                    }
                    return Err(format!("I don't know a function called '{name}'"));
                };
                if args.len() != sig.params.len() {
                    return Err(format!(
                        "'{name}' takes {} thing{}, but got {}",
                        sig.params.len(),
                        if sig.params.len() == 1 { "" } else { "s" },
                        args.len()
                    ));
                }
                let block = self.arg_block(args)?;
                self.asm.movz_reg(X17, 0);
                self.asm.bl_label(&format!("fn_{name}"));
                if block > 0 {
                    self.asm.add_imm(SP, SP, block);
                }
                Ok(sig.ret)
            }
            Expr::CallValue { func, args } => self.dynamic_call(func, args),
            Expr::FuncRef(name) => {
                let Some(sig) = self.funcs.get(name).cloned() else {
                    return Err(format!("I don't know a function called '{name}'"));
                };
                if sig.ret == Ty::Nothing {
                    return Err(format!(
                        "'{name}' gives nothing back, so it can't be used as a function value"
                    ));
                }
                self.emit_function_record(&format!("fn_{name}"), &[]);
                Ok(Ty::Func(std::rc::Rc::new(FuncTy {
                    params: sig.params.clone(),
                    ret: sig.ret.clone(),
                })))
            }
            Expr::Lambda { params, body } => {
                let capture_names: Vec<String> = lambda_free_names(params, body)
                    .into_iter()
                    .filter(|name| self.vars.contains_key(name))
                    .collect();
                let captures: Vec<(String, Ty)> = capture_names
                    .iter()
                    .map(|name| (name.clone(), self.vars[name].ty.clone()))
                    .collect();
                let Some(Ty::Func(ft)) = self.expr_type(e)? else {
                    return Err("internal: a function value has no function kind".to_string());
                };
                let label = self.new_label("lambda");
                let ret_label = self.new_label("lambda_ret");
                self.emit_function_record(&label, &capture_names);
                self.pending_lambdas.push(PendingLambda {
                    label,
                    ret_label,
                    params: params.clone(),
                    body: body.clone(),
                    captures,
                    ret: ft.ret.clone(),
                });
                Ok(Ty::Func(ft))
            }
            Expr::View(inner) => self.expr(inner),
            Expr::Length(inner) => {
                let ty = self.expr(inner)?;
                if ty != Ty::Str && ty.elem().is_none() {
                    return Err(format!(
                        "'length of' counts the letters in text or the items in a list, but this is {ty}"
                    ));
                }
                self.asm.ldr(X0, X0, 0);
                Ok(Ty::Int)
            }
            Expr::Letter(index, text) => {
                let it = self.expr(index)?;
                if it != Ty::Int {
                    return Err(format!(
                        "the letter number must be a number, but this is {it}"
                    ));
                }
                self.asm.push_reg(X0);
                let tt = self.expr(text)?;
                if tt != Ty::Str {
                    return Err(format!("'letter ... of' needs text, but this is {tt}"));
                }
                // text_letter wants the index in X0 and the text in X1.
                self.asm.mov_reg_reg(X1, X0);
                self.asm.pop_reg(X0);
                let msg = self.runtime_message("a letter outside the text");
                let id = self.intern(&msg);
                self.asm.mov_abs(X4, id);
                self.asm.bl_label("text_letter");
                Ok(Ty::Str)
            }
            Expr::Part { text, from, to } => {
                let tt = self.expr(text)?;
                if tt != Ty::Str {
                    return Err(format!("'part ... of' needs text, but this is {tt}"));
                }
                self.asm.push_reg(X0);
                let ft = self.expr(from)?;
                if ft != Ty::Int {
                    return Err(format!(
                        "the start of a 'part ... of' must be a number, but this is {ft}"
                    ));
                }
                self.asm.push_reg(X0);
                let et = self.expr(to)?;
                if et != Ty::Int {
                    return Err(format!(
                        "the end of a 'part ... of' must be a number, but this is {et}"
                    ));
                }
                // text_part wants start in X0, end in X1, text in X2.
                self.asm.mov_reg_reg(X1, X0);
                self.asm.pop_reg(X0);
                self.asm.pop_reg(X2);
                let msg = self.runtime_message("a part outside the text");
                let id = self.intern(&msg);
                self.asm.mov_abs(X4, id);
                self.asm.bl_label("text_part");
                Ok(Ty::Str)
            }
            Expr::Array(items) => {
                if items.is_empty() {
                    return Err(
                        "an empty list needs a kind — write 'an empty list of numbers', 'an empty list of text', or 'an empty list of truth values'"
                            .to_string(),
                    );
                }
                let mut elem: Option<Elem> = None;
                for item in items {
                    let t = self.expr(item)?;
                    let this = match Elem::from_ty(&t) {
                        Some(e) => e,
                        None => return Err("nothing can go in a list".to_string()),
                    };
                    if let Some(old) = elem {
                        if old != this {
                            return Err(format!(
                                "this list mixes {} and {} — a list holds one kind of thing",
                                old.ty(),
                                this.ty()
                            ));
                        }
                    }
                    elem = Some(this);
                    self.asm.push_reg(X0);
                }

                let n = items.len() as u64;
                if n * 8 + 16 >= 4096 * 8 {
                    return Err("this list literal is too long".to_string());
                }
                self.asm.mov_imm64(X0, n * 8 + 16);
                self.asm.bl_label("heap_alloc");
                self.asm.mov_reg_reg(X6, X0);
                self.asm.mov_imm64(X7, n);
                self.asm.str(X7, X6, 0);
                self.asm.str(X7, X6, 8); // capacity starts equal to length
                for i in (0..n).rev() {
                    self.asm.pop_reg(X0);
                    self.asm.str(X0, X6, (i * 8 + 16) as u32);
                }
                self.asm.mov_reg_reg(X0, X6);
                Ok(Ty::Array(elem.unwrap()))
            }
            Expr::EmptyArray(elem) => {
                self.asm.mov_imm64(X0, 16);
                self.asm.bl_label("heap_alloc");
                self.asm.str(XZR, X0, 0); // length
                self.asm.str(XZR, X0, 8); // capacity
                Ok(Ty::Array(elem.clone()))
            }
            Expr::Item(index, array) => {
                let it = self.expr(index)?;
                if it != Ty::Int {
                    return Err(format!(
                        "the item number must be a number, but this is {it}"
                    ));
                }
                self.asm.push_reg(X0);
                let at = self.expr(array)?;
                let Some(elem) = at.elem() else {
                    return Err(format!("'item ... of' needs a list, but this is {at}"));
                };
                self.asm.mov_reg_reg(X1, X0);
                self.asm.pop_reg(X0);
                let msg = self.runtime_message("an item outside the list");
                let id = self.intern(&msg);
                self.asm.mov_abs(X4, id);
                self.asm.bl_label("array_get");
                Ok(elem.ty())
            }
            Expr::Copy(inner) => {
                let ty = self.expr(inner)?;
                match ty {
                    Ty::Str => {
                        self.asm.bl_label("text_copy");
                        Ok(Ty::Str)
                    }
                    Ty::Array(elem) => {
                        let label = self.list_copy_label(&elem);
                        self.asm.bl_label(&label);
                        Ok(Ty::Array(elem))
                    }
                    Ty::Int | Ty::Bool => Ok(ty),
                    Ty::Func(_) => Ok(ty),
                    Ty::Nothing => {
                        Err("there's nothing to copy — that function gives nothing back"
                            .to_string())
                    }
                }
            }
            Expr::Argument(index) => {
                let ty = self.expr(index)?;
                if ty != Ty::Int {
                    return Err(format!(
                        "the argument number must be a number, but this is {ty}"
                    ));
                }
                self.asm.bl_label("arg_text");
                Ok(Ty::Str)
            }
            Expr::ArgCount => {
                self.asm.mov_data_addr(X1, ARGS_SP_OFFSET);
                self.asm.ldr(X0, X1, 0);
                self.asm.ldr(X0, X0, 0); // argc
                self.asm.sub_imm(X0, X0, 1); // skip the program's own name
                Ok(Ty::Int)
            }
            Expr::Environment(name) => {
                let ty = self.expr(name)?;
                if ty != Ty::Str {
                    return Err(format!(
                        "'environment' needs a variable name as text, but this is {ty}"
                    ));
                }
                self.asm.bl_label("env_text");
                Ok(Ty::Str)
            }
            Expr::ReadLine => {
                self.asm.movz_reg(X0, 0); // stdin
                self.asm.movz_reg(X1, 1); // stop at a newline
                self.asm.bl_label("read_stream");
                Ok(Ty::Str)
            }
            Expr::ReadAll => {
                self.asm.movz_reg(X0, 0); // stdin
                self.asm.movz_reg(X1, 0); // read to end of file
                self.asm.bl_label("read_stream");
                Ok(Ty::Str)
            }
            Expr::ReadFile(path) => {
                let ty = self.expr(path)?;
                if ty != Ty::Str {
                    return Err(format!(
                        "'read file' needs a path as text, but this is {ty}"
                    ));
                }
                self.asm.bl_label("read_file");
                Ok(Ty::Str)
            }
            Expr::ListDirectory(path) => {
                self.path_expr(path, "list directory")?;
                self.asm.bl_label("list_directory");
                Ok(Ty::Array(Elem::STR))
            }
            Expr::FileExists(path) => {
                self.path_expr(path, "file exists")?;
                self.asm.bl_label("file_exists");
                Ok(Ty::Bool)
            }
            Expr::AllArguments => {
                self.asm.bl_label("all_arguments");
                Ok(Ty::Array(Elem::STR))
            }
            Expr::RunCommand(command) => {
                let ty = self.expr(command)?;
                if ty != Ty::Str {
                    return Err(format!(
                        "'run command' needs the command as text, but this is {ty}"
                    ));
                }
                self.asm.bl_label("run_command");
                Ok(Ty::Int)
            }
            Expr::Unary(op, inner) => {
                let ty = self.expr(inner)?;
                match op {
                    UnOp::Neg => {
                        if ty != Ty::Int {
                            return Err(format!("'-' flips a number, but this is {ty}"));
                        }
                        self.asm.neg_reg(X0, X0);
                        Ok(Ty::Int)
                    }
                    UnOp::Not => {
                        if ty != Ty::Bool {
                            return Err(format!(
                                "'not' flips something true or false, but this is {ty}"
                            ));
                        }
                        self.asm.cmp_reg_reg(X0, XZR);
                        self.asm.cset(X0, CC_EQ);
                        Ok(Ty::Bool)
                    }
                }
            }
            Expr::Binary(op @ (BinOp::And | BinOp::Or), left, right) => {
                let lt = self.expr(left)?;
                if lt != Ty::Bool {
                    return Err(format!(
                        "'{}' joins truth values, but the left side is {lt}",
                        op_name(*op)
                    ));
                }
                let done = self.new_label("short_circuit");
                let cc = if *op == BinOp::And { CC_EQ } else { CC_NE };
                // Leaving X0 alone on the short path gives back the left value:
                // false for `and`, true for `or`.
                self.asm.cmp_reg_reg(X0, XZR);
                self.asm.bcond_label(cc, &done);
                let rt = self.expr(right)?;
                if rt != Ty::Bool {
                    return Err(format!(
                        "'{}' joins truth values, but the right side is {rt}",
                        op_name(*op)
                    ));
                }
                self.asm.label(&done);
                Ok(Ty::Bool)
            }
            Expr::Binary(op, left, right) => {
                let lt = self.expr(left)?;
                self.asm.push_reg(X0);
                let rt = self.expr(right)?;
                self.asm.pop_reg(X1);
                self.binary(*op, lt, rt)
            }
        }
    }

    fn binary(&mut self, op: BinOp, lt: Ty, rt: Ty) -> Result<Ty, String> {
        use BinOp::*;
        match op {
            Add => {
                if lt == Ty::Int && rt == Ty::Int {
                    self.asm.add_reg_reg(X0, X1, X0);
                    Ok(Ty::Int)
                } else if lt == Ty::Str && rt == Ty::Str {
                    // X1 = left, X0 = right. `text_concat` wants left in X0.
                    self.asm.mov_reg_reg(X2, X0);
                    self.asm.mov_reg_reg(X0, X1);
                    self.asm.mov_reg_reg(X1, X2);
                    self.asm.bl_label("text_concat");
                    Ok(Ty::Str)
                } else {
                    Err(format!(
                        "'+' adds numbers or joins text, but this has {lt} and {rt}"
                    ))
                }
            }
            Sub | Mul | Div => {
                if lt != Ty::Int || rt != Ty::Int {
                    return Err(format!(
                        "'{}' works on numbers, but this has {lt} and {rt}",
                        op_name(op)
                    ));
                }
                match op {
                    Sub => self.asm.sub_reg_reg(X0, X1, X0),
                    Mul => self.asm.mul_reg_reg(X0, X1, X0),
                    Div => {
                        let ok = self.new_label("div_ok");
                        self.asm.cmp_reg_reg(X0, XZR);
                        self.asm.bcond_label(CC_NE, &ok);
                        let msg = self.runtime_message("I can't divide by zero");
                        let id = self.intern(&msg);
                        self.asm.mov_abs(X0, id);
                        self.asm.b_label("runtime_fail");
                        self.asm.label(&ok);
                        self.asm.sdiv_reg_reg(X0, X1, X0);
                    }
                    _ => unreachable!(),
                }
                Ok(Ty::Int)
            }
            Eq | NotEq | Lt | LtEq | Gt | GtEq => {
                if lt != rt {
                    return Err(format!("'{}' can't compare {lt} with {rt}", op_name(op)));
                }
                let cc = match op {
                    Eq => CC_EQ,
                    NotEq => CC_NE,
                    Lt => CC_LT,
                    LtEq => CC_LE,
                    Gt => CC_GT,
                    GtEq => CC_GE,
                    _ => unreachable!(),
                };
                if lt == Ty::Str {
                    // X1 = left, X0 = right. `text_compare` wants left in X0
                    // and gives back -1, 0, or 1.
                    self.asm.mov_reg_reg(X2, X0);
                    self.asm.mov_reg_reg(X0, X1);
                    self.asm.mov_reg_reg(X1, X2);
                    self.asm.bl_label("text_compare");
                    self.asm.cmp_reg_reg(X0, XZR);
                } else {
                    self.asm.cmp_reg_reg(X1, X0);
                }
                self.asm.cset(X0, cc);
                Ok(Ty::Bool)
            }
            And | Or => unreachable!("and/or are short-circuited before binary"),
        }
    }

    // ---- printing --------------------------------------------------------

    fn emit_say(&mut self, ty: Ty) {
        self.emit_show(ty, 1);
    }

    /// Print a value to `fd` with a newline. `fd` 1 is stdout, 2 is stderr.
    fn emit_show(&mut self, ty: Ty, fd: u64) {
        match ty {
            Ty::Int => {
                self.asm.mov_imm64(X9, fd);
                self.asm.bl_label("print_int");
                self.emit_newline(fd);
            }
            Ty::Str => self.emit_print_text(fd),
            Ty::Bool => {
                let l_false = self.new_label("say_false");
                let l_done = self.new_label("say_done");
                self.asm.cmp_reg_reg(X0, XZR);
                self.asm.bcond_label(CC_EQ, &l_false);
                let t = self.intern("true");
                self.asm.mov_abs(X0, t);
                self.asm.b_label(&l_done);
                self.asm.label(&l_false);
                let f = self.intern("false");
                self.asm.mov_abs(X0, f);
                self.asm.label(&l_done);
                self.emit_print_text(fd);
            }
            Ty::Array(elem) => self.emit_say_array(elem, fd),
            Ty::Func(_) => {
                let id = self.intern("<function>");
                self.asm.mov_abs(X0, id);
                self.emit_print_text(fd);
            }
            Ty::Nothing => {}
        }
    }

    /// X0 points at a length-prefixed string: `[len: u64][bytes...]`.
    fn emit_write_text_value(&mut self, fd: u64) {
        self.asm.ldr(X2, X0, 0); // len
        self.asm.add_imm(X1, X0, 8); // bytes
        self.asm.mov_imm64(X0, fd);
        self.asm.mov_imm64(X8, SYS_WRITE);
        self.asm.svc();
    }

    fn emit_print_text(&mut self, fd: u64) {
        self.emit_write_text_value(fd);
        self.emit_newline(fd);
    }

    /// Write an interned literal without a newline.
    fn emit_write_literal(&mut self, s: &str, fd: u64) {
        let id = self.intern(s);
        self.asm.mov_abs(X1, id);
        self.asm.add_imm(X1, X1, 8);
        self.asm.mov_imm64(X2, s.len() as u64);
        self.asm.mov_imm64(X0, fd);
        self.asm.mov_imm64(X8, SYS_WRITE);
        self.asm.svc();
    }

    /// X0 points at a list: `[len: u64][capacity: u64][item: i64]...`.
    /// Print `[a, b, c]`.
    fn emit_say_array(&mut self, elem: Elem, fd: u64) {
        if elem.depth() > 0 {
            let label = self.show_array_label(&elem);
            self.asm.mov_imm64(X9, fd);
            self.asm.bl_label(&label);
            self.emit_newline(fd);
            return;
        }

        self.asm.sub_imm(SP, SP, 32);
        self.asm.str(X19, SP, 0);
        self.asm.str(X20, SP, 8);
        self.asm.str(X21, SP, 16);

        self.asm.mov_reg_reg(X19, X0); // list
        self.asm.ldr(X20, X19, 0); // length
        self.asm.add_imm(X19, X19, 16); // items
        self.emit_write_literal("[", fd);

        self.asm.movz_reg(X21, 0); // index
        let l_loop = self.new_label("say_list");
        let l_first = self.new_label("say_first");
        let l_done = self.new_label("say_done");

        self.asm.label(&l_loop);
        self.asm.cmp_reg_reg(X21, X20);
        self.asm.bcond_label(CC_GE, &l_done);
        self.asm.cmp_reg_reg(X21, XZR);
        self.asm.bcond_label(CC_EQ, &l_first);
        self.emit_write_literal(", ", fd);
        self.asm.label(&l_first);

        self.asm.ldr(X0, X19, 0);
        self.emit_scalar_item(&elem, fd);

        self.asm.add_imm(X19, X19, 8);
        self.asm.add_imm(X21, X21, 1);
        self.asm.b_label(&l_loop);

        self.asm.label(&l_done);
        self.emit_write_literal("]", fd);
        self.asm.ldr(X19, SP, 0);
        self.asm.ldr(X20, SP, 8);
        self.asm.ldr(X21, SP, 16);
        self.asm.add_imm(SP, SP, 32);
        self.emit_newline(fd);
    }

    /// Print one scalar list item that is already in X0.
    fn emit_scalar_item(&mut self, elem: &Elem, fd: u64) {
        if elem.func().is_some() {
            let id = self.intern("<function>");
            self.asm.mov_abs(X0, id);
            self.emit_write_text_value(fd);
            return;
        }
        match elem.base() {
            Base::Int => {
                self.asm.mov_imm64(X9, fd);
                self.asm.bl_label("print_int");
            }
            Base::Str => self.emit_write_text_value(fd),
            Base::Bool => {
                let l_false = self.new_label("say_item_false");
                let l_after = self.new_label("say_item_after");
                self.asm.cmp_reg_reg(X0, XZR);
                self.asm.bcond_label(CC_EQ, &l_false);
                let t = self.intern("true");
                self.asm.mov_abs(X0, t);
                self.asm.b_label(&l_after);
                self.asm.label(&l_false);
                let f = self.intern("false");
                self.asm.mov_abs(X0, f);
                self.asm.label(&l_after);
                self.emit_write_text_value(fd);
            }
        }
    }

    /// Write a literal using the file descriptor already in X9.
    fn emit_write_literal_x9(&mut self, s: &str) {
        let id = self.intern(s);
        self.asm.mov_abs(X1, id);
        self.asm.add_imm(X1, X1, 8);
        self.asm.mov_imm64(X2, s.len() as u64);
        self.asm.mov_reg_reg(X0, X9);
        self.asm.mov_imm64(X8, SYS_WRITE);
        self.asm.svc();
    }

    /// Write the text in X0 using the file descriptor already in X9.
    fn emit_write_text_value_x9(&mut self) {
        self.asm.ldr(X2, X0, 0);
        self.asm.add_imm(X1, X0, 8);
        self.asm.mov_reg_reg(X0, X9);
        self.asm.mov_imm64(X8, SYS_WRITE);
        self.asm.svc();
    }

    /// Print one scalar item in X0 when the file descriptor is already in X9.
    fn emit_scalar_item_x9(&mut self, elem: &Elem) {
        if elem.func().is_some() {
            let id = self.intern("<function>");
            self.asm.mov_abs(X0, id);
            self.emit_write_text_value_x9();
            return;
        }
        match elem.base() {
            Base::Int => self.asm.bl_label("print_int"),
            Base::Str => self.emit_write_text_value_x9(),
            Base::Bool => {
                let l_false = self.new_label("show_item_false");
                let l_after = self.new_label("show_item_after");
                self.asm.cmp_reg_reg(X0, XZR);
                self.asm.bcond_label(CC_EQ, &l_false);
                let t = self.intern("true");
                self.asm.mov_abs(X0, t);
                self.asm.b_label(&l_after);
                self.asm.label(&l_false);
                let f = self.intern("false");
                self.asm.mov_abs(X0, f);
                self.asm.label(&l_after);
                self.emit_write_text_value_x9();
            }
        }
    }

    /// Emit the recursive helpers discovered while compiling expressions.
    fn emit_nested_helpers(&mut self) {
        let mut copies = self.copy_shapes.clone();
        copies.sort_by_key(|elem| elem.depth());
        for elem in copies {
            self.emit_array_copy_nested(elem);
        }

        let mut shows = self.show_shapes.clone();
        shows.sort_by_key(|elem| elem.depth());
        for elem in shows {
            self.emit_show_array_helper(elem);
        }
    }

    /// A list of lists (or deeper) is copied item by item, recursively.
    fn emit_array_copy_nested(&mut self, elem: Elem) {
        let label = format!("array_copy_{}", Self::shape_tag(&elem));
        let l_loop = self.new_label("acopy_nested_loop");
        let l_done = self.new_label("acopy_nested_done");
        let child = elem.inner().expect("a nested list shape");
        let child_label = self.list_copy_label(&child);
        let a = &mut self.asm;

        a.label(&label);
        a.sub_imm(SP, SP, 48);
        a.str(X19, SP, 0);
        a.str(X20, SP, 8);
        a.str(X21, SP, 16);
        a.str(X22, SP, 24);
        a.str(X23, SP, 32);
        a.str(X30, SP, 40);

        a.mov_reg_reg(X19, X0); // source list
        a.ldr(X21, X19, 0); // length
        a.mov_reg_reg(X0, X21);
        a.add_reg_reg(X0, X0, X0);
        a.add_reg_reg(X0, X0, X0);
        a.add_reg_reg(X0, X0, X0); // length * 8
        a.add_imm(X0, X0, 16);
        a.bl_label("heap_alloc");
        a.mov_reg_reg(X6, X0); // destination start
        a.str(X21, X6, 0);
        a.str(X21, X6, 8);

        a.mov_reg_reg(X23, X6); // destination start
        a.add_imm(X19, X19, 16); // source items
        a.add_imm(X20, X6, 16); // destination items
        a.movz_reg(X22, 0);
        a.label(&l_loop);
        a.cmp_reg_reg(X22, X21);
        a.bcond_label(CC_GE, &l_done);
        a.ldr(X0, X19, 0);
        a.bl_label(&child_label);
        a.str(X0, X20, 0);
        a.add_imm(X19, X19, 8);
        a.add_imm(X20, X20, 8);
        a.add_imm(X22, X22, 1);
        a.b_label(&l_loop);

        a.label(&l_done);
        a.mov_reg_reg(X0, X23);
        a.ldr(X19, SP, 0);
        a.ldr(X20, SP, 8);
        a.ldr(X21, SP, 16);
        a.ldr(X22, SP, 24);
        a.ldr(X23, SP, 32);
        a.ldr(X30, SP, 40);
        a.add_imm(SP, SP, 48);
        a.ret();
    }

    /// Print a list whose items have shape `elem` without a newline, using
    /// the descriptor in X9.
    fn emit_show_array_helper(&mut self, elem: Elem) {
        let label = format!("show_array_{}", Self::shape_tag(&elem));
        let l_loop = self.new_label("show_list");
        let l_first = self.new_label("show_first");
        let l_done = self.new_label("show_done");
        let child_label = if let Some(child) = elem.inner() {
            let child_label = self.show_array_label(&child);
            Some((child, child_label))
        } else {
            None
        };

        self.asm.label(&label);
        self.asm.sub_imm(SP, SP, 48);
        self.asm.str(X19, SP, 0);
        self.asm.str(X20, SP, 8);
        self.asm.str(X21, SP, 16);
        self.asm.str(X22, SP, 24);
        self.asm.str(X23, SP, 32);
        self.asm.str(X30, SP, 40);

        self.asm.mov_reg_reg(X19, X0);
        self.asm.ldr(X20, X19, 0);
        self.asm.add_imm(X19, X19, 16);
        self.emit_write_literal_x9("[");

        self.asm.movz_reg(X21, 0);
        self.asm.label(&l_loop);
        self.asm.cmp_reg_reg(X21, X20);
        self.asm.bcond_label(CC_GE, &l_done);
        self.asm.cmp_reg_reg(X21, XZR);
        self.asm.bcond_label(CC_EQ, &l_first);
        self.emit_write_literal_x9(", ");
        self.asm.label(&l_first);

        self.asm.ldr(X0, X19, 0);
        if let Some((_, label)) = &child_label {
            self.asm.bl_label(label);
        } else {
            self.emit_scalar_item_x9(&elem);
        }

        self.asm.add_imm(X19, X19, 8);
        self.asm.add_imm(X21, X21, 1);
        self.asm.b_label(&l_loop);

        self.asm.label(&l_done);
        self.emit_write_literal_x9("]");
        self.asm.ldr(X19, SP, 0);
        self.asm.ldr(X20, SP, 8);
        self.asm.ldr(X21, SP, 16);
        self.asm.ldr(X22, SP, 24);
        self.asm.ldr(X23, SP, 32);
        self.asm.ldr(X30, SP, 40);
        self.asm.add_imm(SP, SP, 48);
        self.asm.ret();
    }

    fn emit_newline(&mut self, fd: u64) {
        let id = self.intern("\n");
        self.asm.mov_abs(X1, id);
        self.asm.add_imm(X1, X1, 8);
        self.asm.mov_imm64(X2, 1);
        self.asm.mov_imm64(X0, fd);
        self.asm.mov_imm64(X8, SYS_WRITE);
        self.asm.svc();
    }

    /// `print_int`: X0 in, decimal digits out, no newline, no libc.
    ///
    /// Writes backwards from the end of a 32-byte buffer, so the digits come out
    /// in the right order without a second pass.
    fn emit_print_int(&mut self) {
        let buf_end = ITOA_OFFSET + ITOA_CAPACITY;
        let a = &mut self.asm;

        a.label("print_int");
        a.sub_imm(SP, SP, 32);
        a.str(X19, SP, 0);

        a.mov_data_addr(X19, buf_end);
        a.mov_reg_reg(X1, X19); // ptr = end
        a.mov_reg_reg(X3, X0); // value
        a.movz_reg(X4, 0); // no sign yet

        a.cmp_reg_reg(X3, XZR);
        a.bcond_label(CC_GE, "pi_digits");
        a.neg_reg(X3, X3);
        a.movz_reg(X4, 1);

        a.label("pi_digits");
        a.label("pi_loop");
        a.movz_reg(X5, 10);
        a.udiv_reg_reg(X7, X3, X5); // X7 = magnitude / 10
        a.msub_reg_reg(X6, X7, X5, X3); // X6 = value - X7*10
        a.add_imm(X6, X6, 48); // -> ASCII
        a.sub_imm(X1, X1, 1);
        a.strb(X6, X1, 0);
        a.mov_reg_reg(X3, X7);
        a.cmp_reg_reg(X3, XZR);
        a.bcond_label(CC_NE, "pi_loop");

        a.cmp_reg_reg(X4, XZR);
        a.bcond_label(CC_EQ, "pi_write");
        a.sub_imm(X1, X1, 1);
        a.movz_reg(X6, 45); // '-'
        a.strb(X6, X1, 0);

        a.label("pi_write");
        a.mov_reg_reg(X2, X19);
        a.sub_reg_reg(X2, X2, X1); // length
        a.mov_reg_reg(X0, X9); // chosen file descriptor
        a.mov_imm64(X8, SYS_WRITE);
        a.svc();

        a.ldr(X19, SP, 0);
        a.add_imm(SP, SP, 32);
        a.ret();
    }

    /// `text_concat`: X0 = left, X1 = right, result in X0.
    ///
    /// Copies both byte blobs into the bump-allocated heap as one
    /// length-prefixed string. There is no bound check yet; the heap is
    /// 1 MiB, which is generous for a language this size.
    fn emit_text_concat(&mut self) {
        let a = &mut self.asm;

        a.label("text_concat");
        a.sub_imm(SP, SP, 32);
        a.str(X19, SP, 0);
        a.str(X20, SP, 8);
        a.str(X30, SP, 16);

        a.mov_reg_reg(X19, X0); // left
        a.mov_reg_reg(X20, X1); // right
        a.ldr(X2, X19, 0);
        a.ldr(X3, X20, 0);
        a.add_reg_reg(X4, X2, X3);
        a.add_imm(X0, X4, 8);
        a.bl_label("heap_alloc");
        a.mov_reg_reg(X6, X0);

        a.ldr(X2, X19, 0);
        a.ldr(X3, X20, 0);
        a.add_reg_reg(X4, X2, X3); // total length
        a.str(X4, X6, 0);
        a.add_imm(X9, X6, 8); // destination bytes

        a.add_imm(X8, X19, 8);
        a.mov_reg_reg(X10, X2);
        a.label("tcat_left");
        a.cmp_reg_reg(X10, XZR);
        a.bcond_label(CC_EQ, "tcat_right");
        a.ldrb(X11, X8, 0);
        a.strb(X11, X9, 0);
        a.add_imm(X8, X8, 1);
        a.add_imm(X9, X9, 1);
        a.sub_imm(X10, X10, 1);
        a.b_label("tcat_left");

        a.label("tcat_right");
        a.add_imm(X8, X20, 8);
        a.mov_reg_reg(X10, X3);
        a.label("tcat_right_loop");
        a.cmp_reg_reg(X10, XZR);
        a.bcond_label(CC_EQ, "tcat_done");
        a.ldrb(X11, X8, 0);
        a.strb(X11, X9, 0);
        a.add_imm(X8, X8, 1);
        a.add_imm(X9, X9, 1);
        a.sub_imm(X10, X10, 1);
        a.b_label("tcat_right_loop");

        a.label("tcat_done");
        a.mov_reg_reg(X0, X6);
        a.ldr(X19, SP, 0);
        a.ldr(X20, SP, 8);
        a.ldr(X30, SP, 16);
        a.add_imm(SP, SP, 32);
        a.ret();
    }

    /// `text_compare`: X0 = left, X1 = right. Gives back -1, 0, or 1 in X0,
    /// comparing byte by byte and then by length (dictionary order).
    fn emit_text_compare(&mut self) {
        let a = &mut self.asm;

        a.label("text_compare");
        a.ldr(X2, X0, 0); // left length
        a.ldr(X3, X1, 0); // right length
        a.cmp_reg_reg(X2, X3);
        a.bcond_label(CC_LE, "tc_min_left");
        a.mov_reg_reg(X4, X3);
        a.b_label("tc_min_done");
        a.label("tc_min_left");
        a.mov_reg_reg(X4, X2);
        a.label("tc_min_done");

        a.add_imm(X5, X0, 8);
        a.add_imm(X6, X1, 8);
        a.mov_reg_reg(X7, X4);
        a.label("tc_loop");
        a.cmp_reg_reg(X7, XZR);
        a.bcond_label(CC_EQ, "tc_lengths");
        a.ldrb(X8, X5, 0);
        a.ldrb(X9, X6, 0);
        a.cmp_reg_reg(X8, X9);
        a.bcond_label(CC_LT, "tc_less");
        a.bcond_label(CC_GT, "tc_greater");
        a.add_imm(X5, X5, 1);
        a.add_imm(X6, X6, 1);
        a.sub_imm(X7, X7, 1);
        a.b_label("tc_loop");

        a.label("tc_lengths");
        a.cmp_reg_reg(X2, X3);
        a.bcond_label(CC_LT, "tc_less");
        a.bcond_label(CC_GT, "tc_greater");
        a.movz_reg(X0, 0);
        a.ret();

        a.label("tc_less");
        a.mov_imm64(X0, (-1i64) as u64);
        a.ret();

        a.label("tc_greater");
        a.movz_reg(X0, 1);
        a.ret();
    }

    /// `text_letter`: X0 = index (1-based), X1 = text, result in X0.
    ///
    /// Allocates a fresh one-byte string on the heap. X4 holds the
    /// length-prefixed runtime error to print if the index falls outside.
    fn emit_text_letter(&mut self) {
        let a = &mut self.asm;

        a.label("text_letter");
        a.ldr(X2, X1, 0); // length
        a.movz_reg(X3, 1);
        a.cmp_reg_reg(X0, X3);
        a.bcond_label(CC_LT, "letter_fail");
        a.cmp_reg_reg(X0, X2);
        a.bcond_label(CC_GT, "letter_fail");

        a.sub_imm(SP, SP, 32);
        a.str(X19, SP, 0);
        a.str(X20, SP, 8);
        a.str(X30, SP, 16);
        a.mov_reg_reg(X19, X0);
        a.mov_reg_reg(X20, X1);
        a.movz_reg(X0, 9); // [len = 1][byte]
        a.bl_label("heap_alloc");
        a.mov_reg_reg(X6, X0);
        a.movz_reg(X3, 1);
        a.str(X3, X6, 0);
        a.add_imm(X1, X20, 8);
        a.sub_imm(X0, X19, 1);
        a.add_reg_reg(X1, X1, X0);
        a.ldrb(X3, X1, 0);
        a.strb(X3, X6, 8);
        a.mov_reg_reg(X0, X6);
        a.ldr(X19, SP, 0);
        a.ldr(X20, SP, 8);
        a.ldr(X30, SP, 16);
        a.add_imm(SP, SP, 32);
        a.ret();

        a.label("letter_fail");
        a.mov_reg_reg(X0, X4);
        a.b_label("runtime_fail");
    }

    /// `text_part`: X0 = start, X1 = end (both 1-based and inclusive),
    /// X2 = text, result in X0. X4 holds the runtime error.
    fn emit_text_part(&mut self) {
        let a = &mut self.asm;

        a.label("text_part");
        a.ldr(X5, X2, 0); // length
        a.movz_reg(X6, 1);
        a.cmp_reg_reg(X0, X6);
        a.bcond_label(CC_LT, "part_fail");
        a.add_reg_reg(X9, X5, X6); // len + 1: an empty part may start here
        a.cmp_reg_reg(X0, X9);
        a.bcond_label(CC_GT, "part_fail");
        a.cmp_reg_reg(X1, X5);
        a.bcond_label(CC_GT, "part_fail");
        a.cmp_reg_reg(X1, X0);
        a.bcond_label(CC_LT, "part_zero");

        // count = end - start + 1
        a.sub_reg_reg(X6, X1, X0);
        a.add_imm(X6, X6, 1);
        a.b_label("part_have_count");

        a.label("part_zero");
        a.movz_reg(X6, 0);

        a.label("part_have_count");
        a.sub_imm(SP, SP, 48);
        a.str(X19, SP, 0);
        a.str(X20, SP, 8);
        a.str(X21, SP, 16);
        a.str(X30, SP, 24);
        a.mov_reg_reg(X19, X0); // start
        a.mov_reg_reg(X20, X2); // text
        a.mov_reg_reg(X21, X6); // count
        a.add_imm(X0, X6, 8);
        a.bl_label("heap_alloc");
        a.mov_reg_reg(X8, X0);
        a.str(X21, X8, 0);

        // Source bytes start at text + 8 + (start - 1).
        a.sub_imm(X10, X19, 1);
        a.add_reg_reg(X10, X20, X10);
        a.add_imm(X10, X10, 8);
        a.add_imm(X11, X8, 8);
        a.mov_reg_reg(X6, X21);
        a.label("part_copy");
        a.cmp_reg_reg(X6, XZR);
        a.bcond_label(CC_EQ, "part_done");
        a.ldrb(X12, X10, 0);
        a.strb(X12, X11, 0);
        a.add_imm(X10, X10, 1);
        a.add_imm(X11, X11, 1);
        a.sub_imm(X6, X6, 1);
        a.b_label("part_copy");

        a.label("part_done");
        a.mov_reg_reg(X0, X8);
        a.ldr(X19, SP, 0);
        a.ldr(X20, SP, 8);
        a.ldr(X21, SP, 16);
        a.ldr(X30, SP, 24);
        a.add_imm(SP, SP, 48);
        a.ret();

        a.label("part_fail");
        a.mov_reg_reg(X0, X4);
        a.b_label("runtime_fail");
    }

    /// `array_get`: X0 = index (1-based), X1 = list, X4 = runtime error.
    /// Result in X0.
    fn emit_array_get(&mut self) {
        let a = &mut self.asm;

        a.label("array_get");
        a.ldr(X2, X1, 0); // length
        a.movz_reg(X3, 1);
        a.cmp_reg_reg(X0, X3);
        a.bcond_label(CC_LT, "array_get_fail");
        a.cmp_reg_reg(X0, X2);
        a.bcond_label(CC_GT, "array_get_fail");

        a.sub_imm(X0, X0, 1);
        a.add_reg_reg(X0, X0, X0); // *2
        a.add_reg_reg(X0, X0, X0); // *4
        a.add_reg_reg(X0, X0, X0); // *8
        a.add_imm(X0, X0, 16); // skip the length and capacity
        a.add_reg_reg(X0, X1, X0);
        a.ldr(X0, X0, 0);
        a.ret();

        a.label("array_get_fail");
        a.mov_reg_reg(X0, X4);
        a.b_label("runtime_fail");
    }

    /// `array_set`: X0 = index (1-based), X1 = list, X2 = value,
    /// X4 = runtime error. Writes in place.
    fn emit_array_set(&mut self) {
        let a = &mut self.asm;

        a.label("array_set");
        a.ldr(X3, X1, 0); // length
        a.movz_reg(X5, 1);
        a.cmp_reg_reg(X0, X5);
        a.bcond_label(CC_LT, "array_set_fail");
        a.cmp_reg_reg(X0, X3);
        a.bcond_label(CC_GT, "array_set_fail");

        a.sub_imm(X0, X0, 1);
        a.add_reg_reg(X0, X0, X0); // *2
        a.add_reg_reg(X0, X0, X0); // *4
        a.add_reg_reg(X0, X0, X0); // *8
        a.add_imm(X0, X0, 16);
        a.add_reg_reg(X0, X1, X0);
        a.str(X2, X0, 0);
        a.ret();

        a.label("array_set_fail");
        a.mov_reg_reg(X0, X4);
        a.b_label("runtime_fail");
    }

    /// `array_add`: X0 = list, X1 = new item. Appends in place when the list
    /// has spare capacity; otherwise it copies the items to a fresh block with
    /// twice the capacity and gives back the new list in X0.
    fn emit_array_add(&mut self) {
        let a = &mut self.asm;

        a.label("array_add");
        a.sub_imm(SP, SP, 64);
        a.str(X19, SP, 0);
        a.str(X20, SP, 8);
        a.str(X21, SP, 16);
        a.str(X22, SP, 24);
        a.str(X23, SP, 32);
        a.str(X30, SP, 40);
        a.mov_reg_reg(X19, X0); // list
        a.mov_reg_reg(X20, X1); // new item
        a.ldr(X2, X19, 0); // old length
        a.ldr(X3, X19, 8); // capacity
        a.cmp_reg_reg(X2, X3);
        a.bcond_label(CC_LT, "array_add_room");

        // Grow: new capacity is at least 4, otherwise double the old capacity.
        a.mov_reg_reg(X5, X3);
        a.add_reg_reg(X5, X5, X5);
        a.movz_reg(X6, 4);
        a.cmp_reg_reg(X5, X6);
        a.bcond_label(CC_GE, "array_add_capacity_ready");
        a.mov_reg_reg(X5, X6);
        a.label("array_add_capacity_ready");

        a.mov_reg_reg(X21, X5); // new capacity
        a.mov_reg_reg(X22, X2); // old length
        a.mov_reg_reg(X0, X5);
        a.add_reg_reg(X0, X0, X0);
        a.add_reg_reg(X0, X0, X0);
        a.add_reg_reg(X0, X0, X0);
        a.add_imm(X0, X0, 16);
        a.bl_label("heap_alloc");
        a.mov_reg_reg(X23, X0); // new list

        a.movz_reg(X6, 1);
        a.add_reg_reg(X6, X22, X6); // new length
        a.str(X6, X23, 0);
        a.str(X21, X23, 8);

        a.add_imm(X7, X23, 16); // destination items
        a.add_imm(X8, X19, 16); // source items
        a.mov_reg_reg(X9, X22);
        a.label("array_add_loop");
        a.cmp_reg_reg(X9, XZR);
        a.bcond_label(CC_EQ, "array_add_copied");
        a.ldr(X11, X8, 0);
        a.str(X11, X7, 0);
        a.add_imm(X8, X8, 8);
        a.add_imm(X7, X7, 8);
        a.sub_imm(X9, X9, 1);
        a.b_label("array_add_loop");

        a.label("array_add_copied");
        a.str(X20, X7, 0); // the new item
                           // The old buffer is unreachable now, so hand it back to the allocator.
        a.mov_reg_reg(X0, X19);
        a.bl_label("heap_free");
        a.mov_reg_reg(X0, X23);
        a.b_label("array_add_done");

        // There is room: append at items[length] without allocating.
        a.label("array_add_room");
        a.mov_reg_reg(X4, X19);
        a.mov_reg_reg(X5, X2);
        a.add_reg_reg(X5, X5, X5); // *2
        a.add_reg_reg(X5, X5, X5); // *4
        a.add_reg_reg(X5, X5, X5); // *8
        a.add_imm(X5, X5, 16);
        a.add_reg_reg(X5, X4, X5);
        a.str(X20, X5, 0);
        a.movz_reg(X6, 1);
        a.add_reg_reg(X6, X2, X6);
        a.str(X6, X4, 0); // new length
        a.mov_reg_reg(X0, X4);

        a.label("array_add_done");
        a.ldr(X19, SP, 0);
        a.ldr(X20, SP, 8);
        a.ldr(X21, SP, 16);
        a.ldr(X22, SP, 24);
        a.ldr(X23, SP, 32);
        a.ldr(X30, SP, 40);
        a.add_imm(SP, SP, 64);
        a.ret();
    }

    /// `array_remove_last`: X0 = list, X4 = runtime error. Shortens the list
    /// by one and returns it in X0. The capacity does not change.
    fn emit_array_remove_last(&mut self) {
        let a = &mut self.asm;

        a.label("array_remove_last");
        a.ldr(X1, X0, 0); // length
        a.cmp_reg_reg(X1, XZR);
        a.bcond_label(CC_EQ, "array_remove_last_fail");
        a.sub_imm(X1, X1, 1);
        a.str(X1, X0, 0);
        a.ret();

        a.label("array_remove_last_fail");
        a.mov_reg_reg(X0, X4);
        a.b_label("runtime_fail");
    }

    /// `array_remove_at`: X0 = index (1-based), X1 = list, X4 = runtime error.
    /// Removes the item, shifts later items left, and returns the list in X0.
    fn emit_array_remove_at(&mut self) {
        let a = &mut self.asm;

        a.label("array_remove_at");
        a.ldr(X2, X1, 0); // length
        a.movz_reg(X3, 1);
        a.cmp_reg_reg(X0, X3);
        a.bcond_label(CC_LT, "array_remove_at_fail");
        a.cmp_reg_reg(X0, X2);
        a.bcond_label(CC_GT, "array_remove_at_fail");

        a.sub_reg_reg(X6, X2, X0); // items after the removed one
        a.sub_imm(X5, X0, 1);
        a.add_reg_reg(X5, X5, X5); // *2
        a.add_reg_reg(X5, X5, X5); // *4
        a.add_reg_reg(X5, X5, X5); // *8
        a.add_imm(X5, X5, 16);
        a.add_reg_reg(X7, X1, X5); // destination slot
        a.add_imm(X8, X7, 8); // source slot

        a.label("array_remove_at_loop");
        a.cmp_reg_reg(X6, XZR);
        a.bcond_label(CC_EQ, "array_remove_at_done");
        a.ldr(X9, X8, 0);
        a.str(X9, X7, 0);
        a.add_imm(X8, X8, 8);
        a.add_imm(X7, X7, 8);
        a.sub_imm(X6, X6, 1);
        a.b_label("array_remove_at_loop");

        a.label("array_remove_at_done");
        a.sub_imm(X2, X2, 1);
        a.str(X2, X1, 0); // new length
        a.mov_reg_reg(X0, X1);
        a.ret();

        a.label("array_remove_at_fail");
        a.mov_reg_reg(X0, X4);
        a.b_label("runtime_fail");
    }

    /// `empty_text`: a fresh text value with length zero, in X0.
    fn emit_empty_text(&mut self) {
        let a = &mut self.asm;

        a.label("empty_text");
        a.sub_imm(SP, SP, 16);
        a.str(X30, SP, 0);
        a.movz_reg(X0, 8);
        a.bl_label("heap_alloc");
        a.str(XZR, X0, 0);
        a.ldr(X30, SP, 0);
        a.add_imm(SP, SP, 16);
        a.ret();
    }

    /// `cstr_copy`: X0 = a NUL-terminated C string. Copies it into a fresh Rusk
    /// text value and gives that back in X0.
    fn emit_cstr_copy(&mut self) {
        let a = &mut self.asm;

        a.label("cstr_copy");
        a.sub_imm(SP, SP, 32);
        a.str(X19, SP, 0);
        a.str(X20, SP, 8);
        a.str(X30, SP, 16);

        a.mov_reg_reg(X19, X0); // source
        a.mov_reg_reg(X1, X0); // scan pointer
        a.label("cstr_len_loop");
        a.ldrb(X2, X1, 0);
        a.cmp_reg_reg(X2, XZR);
        a.bcond_label(CC_EQ, "cstr_len_done");
        a.add_imm(X1, X1, 1);
        a.b_label("cstr_len_loop");

        a.label("cstr_len_done");
        a.sub_reg_reg(X20, X1, X19); // length
        a.add_imm(X0, X20, 8);
        a.bl_label("heap_alloc");
        a.mov_reg_reg(X4, X0);
        a.str(X20, X4, 0);

        a.add_imm(X5, X4, 8); // destination bytes
        a.mov_reg_reg(X6, X19); // source bytes
        a.mov_reg_reg(X7, X20);
        a.label("cstr_copy_loop");
        a.cmp_reg_reg(X7, XZR);
        a.bcond_label(CC_EQ, "cstr_copy_done");
        a.ldrb(X8, X6, 0);
        a.strb(X8, X5, 0);
        a.add_imm(X6, X6, 1);
        a.add_imm(X5, X5, 1);
        a.sub_imm(X7, X7, 1);
        a.b_label("cstr_copy_loop");

        a.label("cstr_copy_done");
        a.mov_reg_reg(X0, X4);
        a.ldr(X19, SP, 0);
        a.ldr(X20, SP, 8);
        a.ldr(X30, SP, 16);
        a.add_imm(SP, SP, 32);
        a.ret();
    }

    /// `arg_text`: X0 = 1-based command-line argument number, result text in X0.
    fn emit_arg_text(&mut self) {
        let a = &mut self.asm;

        a.label("arg_text");
        a.mov_data_addr(X1, ARGS_SP_OFFSET);
        a.ldr(X1, X1, 0); // initial stack pointer
        a.ldr(X2, X1, 0); // argc
        a.movz_reg(X3, 1);
        a.cmp_reg_reg(X0, X3);
        a.bcond_label(CC_LT, "arg_text_empty");
        a.cmp_reg_reg(X0, X2);
        a.bcond_label(CC_GE, "arg_text_empty");
        a.add_imm(X1, X1, 8); // argv
        a.mov_reg_reg(X3, X0);
        a.add_reg_reg(X3, X3, X3); // *2
        a.add_reg_reg(X3, X3, X3); // *4
        a.add_reg_reg(X3, X3, X3); // *8
        a.add_reg_reg(X1, X1, X3);
        a.ldr(X0, X1, 0);
        a.b_label("cstr_copy");

        a.label("arg_text_empty");
        a.b_label("empty_text");
    }

    /// `env_text`: X0 = variable name text, result text in X0. Empty when the
    /// variable is not set.
    fn emit_env_text(&mut self) {
        let a = &mut self.asm;

        a.label("env_text");
        a.sub_imm(SP, SP, 48);
        a.str(X19, SP, 0);
        a.str(X20, SP, 8);
        a.str(X21, SP, 16);
        a.str(X22, SP, 24);
        a.str(X23, SP, 32);
        a.str(X30, SP, 40);

        a.ldr(X20, X0, 0); // name length
        a.add_imm(X19, X0, 8); // name bytes
        a.mov_data_addr(X21, ARGS_SP_OFFSET);
        a.ldr(X21, X21, 0); // initial stack pointer
        a.ldr(X22, X21, 0); // argc
        a.add_imm(X21, X21, 8); // argv
        a.mov_reg_reg(X9, X22);
        a.add_reg_reg(X9, X9, X9); // *2
        a.add_reg_reg(X9, X9, X9); // *4
        a.add_reg_reg(X9, X9, X9); // *8
        a.add_reg_reg(X21, X21, X9); // after argv
        a.add_imm(X21, X21, 8); // envp

        a.label("env_loop");
        a.ldr(X24, X21, 0);
        a.cmp_reg_reg(X24, XZR);
        a.bcond_label(CC_EQ, "env_missing");
        a.mov_reg_reg(X9, X24); // cursor in this entry
        a.mov_reg_reg(X10, X19); // cursor in the name
        a.mov_reg_reg(X11, X20); // bytes left to match
        a.label("env_compare");
        a.cmp_reg_reg(X11, XZR);
        a.bcond_label(CC_EQ, "env_equal");
        a.ldrb(X12, X9, 0);
        a.ldrb(X13, X10, 0);
        a.cmp_reg_reg(X12, X13);
        a.bcond_label(CC_NE, "env_next");
        a.add_imm(X9, X9, 1);
        a.add_imm(X10, X10, 1);
        a.sub_imm(X11, X11, 1);
        a.b_label("env_compare");

        a.label("env_equal");
        a.ldrb(X12, X9, 0);
        a.movz_reg(X13, 61); // '='
        a.cmp_reg_reg(X12, X13);
        a.bcond_label(CC_NE, "env_next");
        a.add_imm(X0, X9, 1); // value after '='
        a.ldr(X30, SP, 40);
        a.ldr(X19, SP, 0);
        a.ldr(X20, SP, 8);
        a.ldr(X21, SP, 16);
        a.ldr(X22, SP, 24);
        a.ldr(X23, SP, 32);
        a.add_imm(SP, SP, 48);
        a.b_label("cstr_copy");

        a.label("env_next");
        a.add_imm(X21, X21, 8);
        a.b_label("env_loop");

        a.label("env_missing");
        a.ldr(X30, SP, 40);
        a.ldr(X19, SP, 0);
        a.ldr(X20, SP, 8);
        a.ldr(X21, SP, 16);
        a.ldr(X22, SP, 24);
        a.ldr(X23, SP, 32);
        a.add_imm(SP, SP, 48);
        a.b_label("empty_text");
    }

    /// `read_stream`: X0 = file descriptor, X1 = 1 to stop at a newline or 0 to
    /// read to end of file. Gives back text in X0.
    ///
    /// Bytes gather in a growable heap buffer with `[capacity][length]`, then
    /// an exact-size text is allocated and the buffer is handed back.
    fn emit_read_stream(&mut self) {
        let a = &mut self.asm;

        a.label("read_stream");
        a.sub_imm(SP, SP, 48);
        a.str(X19, SP, 0);
        a.str(X20, SP, 8);
        a.str(X21, SP, 16);
        a.str(X22, SP, 24);
        a.str(X23, SP, 32);
        a.str(X30, SP, 40);

        a.mov_reg_reg(X19, X0); // fd
        a.mov_reg_reg(X20, X1); // mode
        a.mov_imm64(X21, 4096); // initial capacity
        a.add_imm(X0, X21, 16);
        a.bl_label("heap_alloc");
        a.mov_reg_reg(X22, X0); // buffer
        a.str(X21, X22, 0); // capacity
        a.str(XZR, X22, 8); // length

        a.label("read_stream_loop");
        a.ldr(X23, X22, 8); // length
        a.cmp_reg_reg(X23, X21);
        a.bcond_label(CC_LT, "read_stream_room");

        // Full: double the capacity, copy the bytes, and free the old buffer.
        a.add_reg_reg(X21, X21, X21); // new capacity
        a.add_imm(X0, X21, 16);
        a.bl_label("heap_alloc");
        a.mov_reg_reg(X6, X0);
        a.str(X21, X6, 0);
        a.ldr(X23, X22, 8);
        a.str(X23, X6, 8);
        a.add_imm(X7, X22, 16);
        a.add_imm(X8, X6, 16);
        a.mov_reg_reg(X9, X23);
        a.label("read_stream_copy");
        a.cmp_reg_reg(X9, XZR);
        a.bcond_label(CC_EQ, "read_stream_copied");
        a.ldrb(X11, X7, 0);
        a.strb(X11, X8, 0);
        a.add_imm(X7, X7, 1);
        a.add_imm(X8, X8, 1);
        a.sub_imm(X9, X9, 1);
        a.b_label("read_stream_copy");
        a.label("read_stream_copied");
        a.mov_reg_reg(X0, X22);
        a.bl_label("heap_free");
        a.mov_reg_reg(X22, X6);

        a.label("read_stream_room");
        a.ldr(X23, X22, 8);
        a.add_imm(X10, X22, 16);
        a.add_reg_reg(X10, X10, X23); // write pointer
        a.mov_reg_reg(X0, X19);
        a.mov_reg_reg(X1, X10);
        a.movz_reg(X2, 1);
        a.mov_imm64(X8, SYS_READ);
        a.svc();
        a.cmp_reg_reg(X0, XZR);
        a.bcond_label(CC_LE, "read_stream_done");
        a.ldrb(X11, X10, 0);
        a.cmp_reg_reg(X20, XZR);
        a.bcond_label(CC_EQ, "read_stream_keep");
        a.movz_reg(X12, 10); // newline
        a.cmp_reg_reg(X11, X12);
        a.bcond_label(CC_EQ, "read_stream_done");
        a.label("read_stream_keep");
        a.add_imm(X23, X23, 1);
        a.str(X23, X22, 8);
        a.b_label("read_stream_loop");

        a.label("read_stream_done");
        a.ldr(X23, X22, 8); // length
        a.add_imm(X0, X23, 8);
        a.bl_label("heap_alloc");
        a.mov_reg_reg(X6, X0); // exact text
        a.str(X23, X6, 0);
        a.add_imm(X7, X22, 16);
        a.add_imm(X8, X6, 8);
        a.mov_reg_reg(X9, X23);
        a.label("read_stream_finish_copy");
        a.cmp_reg_reg(X9, XZR);
        a.bcond_label(CC_EQ, "read_stream_finish_done");
        a.ldrb(X11, X7, 0);
        a.strb(X11, X8, 0);
        a.add_imm(X7, X7, 1);
        a.add_imm(X8, X8, 1);
        a.sub_imm(X9, X9, 1);
        a.b_label("read_stream_finish_copy");
        a.label("read_stream_finish_done");
        a.mov_reg_reg(X23, X6);
        a.mov_reg_reg(X0, X22);
        a.bl_label("heap_free");
        a.mov_reg_reg(X0, X23);
        a.ldr(X19, SP, 0);
        a.ldr(X20, SP, 8);
        a.ldr(X21, SP, 16);
        a.ldr(X22, SP, 24);
        a.ldr(X23, SP, 32);
        a.ldr(X30, SP, 40);
        a.add_imm(SP, SP, 48);
        a.ret();
    }

    /// `path_copy`: X0 = path text, result is a NUL-terminated copy in the
    /// fixed scratch area, in X0.
    fn emit_path_copy(&mut self) {
        let too_long = self.intern("burnt: that file path is too long\n");
        let a = &mut self.asm;

        a.label("path_copy");
        a.mov_data_addr(X1, PATH_OFFSET);
        a.b_label("path_copy_to");

        a.label("path_copy_second");
        a.mov_data_addr(X1, PATH2_OFFSET);

        a.label("path_copy_to");
        a.ldr(X2, X0, 0); // length
        a.mov_imm64(X3, PATH_CAPACITY - 1);
        a.cmp_reg_reg(X2, X3);
        a.bcond_label(CC_GT, "path_copy_fail");
        a.add_imm(X3, X0, 8); // source bytes
        a.mov_reg_reg(X4, X1); // write cursor
        a.mov_reg_reg(X5, X1); // start of the destination
        a.mov_reg_reg(X6, X2); // bytes left
        a.label("path_copy_loop");
        a.cmp_reg_reg(X6, XZR);
        a.bcond_label(CC_EQ, "path_copy_done");
        a.ldrb(X7, X3, 0);
        a.strb(X7, X4, 0);
        a.add_imm(X3, X3, 1);
        a.add_imm(X4, X4, 1);
        a.sub_imm(X6, X6, 1);
        a.b_label("path_copy_loop");
        a.label("path_copy_done");
        a.strb(XZR, X4, 0);
        a.mov_reg_reg(X0, X5);
        a.ret();

        a.label("path_copy_fail");
        a.mov_abs(X0, too_long);
        a.b_label("runtime_fail");
    }

    /// `read_file`: X0 = path text, result text in X0.
    fn emit_read_file(&mut self) {
        let open_error = self.intern("burnt: I can't open that file\n");
        let a = &mut self.asm;

        a.label("read_file");
        a.sub_imm(SP, SP, 64);
        a.str(X19, SP, 0);
        a.str(X20, SP, 8);
        a.str(X21, SP, 16);
        a.str(X30, SP, 24);

        a.bl_label("path_copy");
        a.mov_imm64(X0, (-100i64) as u64); // AT_FDCWD
        a.mov_data_addr(X1, PATH_OFFSET);
        a.movz_reg(X2, 0); // O_RDONLY
        a.movz_reg(X3, 0);
        a.mov_imm64(X8, SYS_OPENAT);
        a.svc();
        a.cmp_reg_reg(X0, XZR);
        a.bcond_label(CC_LT, "read_file_fail");
        a.mov_reg_reg(X19, X0); // fd
        a.movz_reg(X1, 0); // read to end
        a.bl_label("read_stream");
        a.mov_reg_reg(X20, X0); // text
        a.mov_reg_reg(X0, X19);
        a.mov_imm64(X8, SYS_CLOSE);
        a.svc();
        a.mov_reg_reg(X0, X20);
        a.ldr(X30, SP, 24);
        a.ldr(X19, SP, 0);
        a.ldr(X20, SP, 8);
        a.ldr(X21, SP, 16);
        a.add_imm(SP, SP, 64);
        a.ret();

        a.label("read_file_fail");
        a.mov_abs(X0, open_error);
        a.b_label("runtime_fail");
    }

    /// `write_file`: X0 = text, X1 = path. Replaces the file's contents.
    fn emit_write_file(&mut self) {
        let open_error = self.intern("burnt: I can't write to that file\n");
        let a = &mut self.asm;

        a.label("write_file");
        a.sub_imm(SP, SP, 64);
        a.str(X19, SP, 0);
        a.str(X20, SP, 8);
        a.str(X21, SP, 16);
        a.str(X30, SP, 24);

        a.mov_reg_reg(X19, X0); // text
        a.mov_reg_reg(X20, X1); // path
        a.mov_reg_reg(X0, X20);
        a.bl_label("path_copy");
        a.mov_imm64(X0, (-100i64) as u64); // AT_FDCWD
        a.mov_data_addr(X1, PATH_OFFSET);
        a.mov_imm64(X2, 577); // O_WRONLY | O_CREAT | O_TRUNC
        a.mov_imm64(X3, 420); // 0o644
        a.mov_imm64(X8, SYS_OPENAT);
        a.svc();
        a.cmp_reg_reg(X0, XZR);
        a.bcond_label(CC_LT, "write_file_fail");
        a.mov_reg_reg(X21, X0); // fd
        a.ldr(X2, X19, 0); // length
        a.add_imm(X1, X19, 8); // bytes
        a.mov_reg_reg(X0, X21);
        a.mov_imm64(X8, SYS_WRITE);
        a.svc();
        a.mov_reg_reg(X0, X21);
        a.mov_imm64(X8, SYS_CLOSE);
        a.svc();
        a.ldr(X30, SP, 24);
        a.ldr(X19, SP, 0);
        a.ldr(X20, SP, 8);
        a.ldr(X21, SP, 16);
        a.add_imm(SP, SP, 64);
        a.ret();

        a.label("write_file_fail");
        a.mov_abs(X0, open_error);
        a.b_label("runtime_fail");
    }

    /// `make_directory`: X0 = path text. Creates the directory, treating an
    /// already-existing one as success.
    fn emit_make_directory(&mut self) {
        let error = self.intern("burnt: I can't make that directory\n");
        let a = &mut self.asm;

        a.label("make_directory");
        a.sub_imm(SP, SP, 32);
        a.str(X30, SP, 0);

        a.bl_label("path_copy");
        a.mov_imm64(X0, AT_FDCWD);
        a.mov_data_addr(X1, PATH_OFFSET);
        a.mov_imm64(X2, 511); // 0o777
        a.mov_imm64(X8, SYS_MKDIRAT);
        a.svc();
        a.cmp_reg_reg(X0, XZR);
        a.bcond_label(CC_GE, "make_directory_done");
        a.mov_imm64(X1, (-17i64) as u64); // EEXIST
        a.cmp_reg_reg(X0, X1);
        a.bcond_label(CC_EQ, "make_directory_done");
        a.mov_abs(X0, error);
        a.b_label("runtime_fail");

        a.label("make_directory_done");
        a.ldr(X30, SP, 0);
        a.add_imm(SP, SP, 32);
        a.ret();
    }

    /// `remove_path`: X0 = path text, X1 = unlinkat flags. Missing paths are
    /// ignored, matching the REPL.
    fn emit_remove_path(&mut self) {
        let error = self.intern("burnt: I can't remove that path\n");
        let a = &mut self.asm;

        a.label("remove_path");
        a.sub_imm(SP, SP, 32);
        a.str(X19, SP, 0);
        a.str(X30, SP, 8);

        a.mov_reg_reg(X19, X1); // flags
        a.bl_label("path_copy");
        a.mov_imm64(X0, AT_FDCWD);
        a.mov_data_addr(X1, PATH_OFFSET);
        a.mov_reg_reg(X2, X19);
        a.mov_imm64(X8, SYS_UNLINKAT);
        a.svc();
        a.cmp_reg_reg(X0, XZR);
        a.bcond_label(CC_GE, "remove_path_done");
        a.mov_imm64(X1, (-2i64) as u64); // ENOENT
        a.cmp_reg_reg(X0, X1);
        a.bcond_label(CC_EQ, "remove_path_done");
        a.mov_abs(X0, error);
        a.b_label("runtime_fail");

        a.label("remove_path_done");
        a.ldr(X19, SP, 0);
        a.ldr(X30, SP, 8);
        a.add_imm(SP, SP, 32);
        a.ret();
    }

    /// `rename_path`: X0 = old path text, X1 = new path text.
    fn emit_rename_path(&mut self) {
        let error = self.intern("burnt: I can't rename that path\n");
        let a = &mut self.asm;

        a.label("rename_path");
        a.sub_imm(SP, SP, 32);
        a.str(X19, SP, 0);
        a.str(X20, SP, 8);
        a.str(X30, SP, 16);

        a.mov_reg_reg(X19, X0);
        a.mov_reg_reg(X20, X1);
        a.mov_reg_reg(X0, X19);
        a.bl_label("path_copy");
        a.mov_reg_reg(X0, X20);
        a.bl_label("path_copy_second");

        a.mov_imm64(X0, AT_FDCWD);
        a.mov_data_addr(X1, PATH_OFFSET);
        a.mov_imm64(X2, AT_FDCWD);
        a.mov_data_addr(X3, PATH2_OFFSET);
        a.mov_imm64(X8, SYS_RENAMEAT);
        a.svc();
        a.cmp_reg_reg(X0, XZR);
        a.bcond_label(CC_GE, "rename_path_done");
        a.mov_abs(X0, error);
        a.b_label("runtime_fail");

        a.label("rename_path_done");
        a.ldr(X19, SP, 0);
        a.ldr(X20, SP, 8);
        a.ldr(X30, SP, 16);
        a.add_imm(SP, SP, 32);
        a.ret();
    }

    /// `file_exists`: X0 = path text, result is true or false in X0.
    fn emit_file_exists(&mut self) {
        let a = &mut self.asm;

        a.label("file_exists");
        a.sub_imm(SP, SP, 16);
        a.str(X30, SP, 0);
        a.bl_label("path_copy");
        a.mov_imm64(X0, AT_FDCWD);
        a.mov_data_addr(X1, PATH_OFFSET);
        a.movz_reg(X2, 0); // F_OK
        a.movz_reg(X3, 0);
        a.mov_imm64(X8, SYS_FACCESSAT);
        a.svc();
        a.cmp_reg_reg(X0, XZR);
        a.cset(X0, CC_EQ);
        a.ldr(X30, SP, 0);
        a.add_imm(SP, SP, 16);
        a.ret();
    }

    /// `all_arguments`: builds a list of every command-line argument after the
    /// program's own name.
    fn emit_all_arguments(&mut self) {
        let a = &mut self.asm;

        a.label("all_arguments");
        a.sub_imm(SP, SP, 48);
        a.str(X19, SP, 0);
        a.str(X20, SP, 8);
        a.str(X21, SP, 16);
        a.str(X22, SP, 24);
        a.str(X30, SP, 32);

        a.mov_data_addr(X1, ARGS_SP_OFFSET);
        a.ldr(X1, X1, 0); // initial stack pointer
        a.ldr(X19, X1, 0); // argc
        a.add_imm(X20, X1, 8); // argv

        a.movz_reg(X0, 48);
        a.bl_label("heap_alloc");
        a.str(XZR, X0, 0);
        a.movz_reg(X1, 4);
        a.str(X1, X0, 8);
        a.mov_reg_reg(X21, X0); // list
        a.movz_reg(X22, 1); // skip argv[0]

        a.label("all_arguments_loop");
        a.cmp_reg_reg(X22, X19);
        a.bcond_label(CC_GE, "all_arguments_done");
        a.mov_reg_reg(X2, X22);
        a.add_reg_reg(X2, X2, X2);
        a.add_reg_reg(X2, X2, X2);
        a.add_reg_reg(X2, X2, X2);
        a.add_reg_reg(X2, X20, X2);
        a.ldr(X0, X2, 0);
        a.bl_label("cstr_copy");
        a.mov_reg_reg(X1, X0);
        a.mov_reg_reg(X0, X21);
        a.bl_label("array_add");
        a.mov_reg_reg(X21, X0);
        a.add_imm(X22, X22, 1);
        a.b_label("all_arguments_loop");

        a.label("all_arguments_done");
        a.mov_reg_reg(X0, X21);
        a.ldr(X19, SP, 0);
        a.ldr(X20, SP, 8);
        a.ldr(X21, SP, 16);
        a.ldr(X22, SP, 24);
        a.ldr(X30, SP, 32);
        a.add_imm(SP, SP, 48);
        a.ret();
    }

    /// `list_directory`: X0 = path text, result is a list of entry names.
    fn emit_list_directory(&mut self) {
        let error = self.intern("burnt: I can't list that directory\n");
        let a = &mut self.asm;

        a.label("list_directory");
        a.sub_imm(SP, SP, 64);
        a.str(X19, SP, 0);
        a.str(X20, SP, 8);
        a.str(X21, SP, 16);
        a.str(X22, SP, 24);
        a.str(X23, SP, 32);
        a.str(X30, SP, 40);

        a.bl_label("path_copy");
        a.mov_imm64(X0, AT_FDCWD);
        a.mov_data_addr(X1, PATH_OFFSET);
        a.mov_imm64(X2, 0x1_0000); // O_RDONLY | O_DIRECTORY
        a.movz_reg(X3, 0);
        a.mov_imm64(X8, SYS_OPENAT);
        a.svc();
        a.cmp_reg_reg(X0, XZR);
        a.bcond_label(CC_LT, "list_directory_fail");
        a.mov_reg_reg(X19, X0); // fd

        a.movz_reg(X0, 48);
        a.bl_label("heap_alloc");
        a.str(XZR, X0, 0);
        a.movz_reg(X1, 4);
        a.str(X1, X0, 8);
        a.mov_reg_reg(X20, X0); // list

        a.label("list_directory_fill");
        a.mov_reg_reg(X0, X19);
        a.mov_data_addr(X1, DENTS_OFFSET);
        a.mov_imm64(X2, DENTS_CAPACITY);
        a.mov_imm64(X8, SYS_GETDENTS64);
        a.svc();
        a.cmp_reg_reg(X0, XZR);
        a.bcond_label(CC_EQ, "list_directory_done");
        a.bcond_label(CC_LT, "list_directory_fail");
        a.mov_reg_reg(X22, X0); // bytes in this batch
        a.mov_data_addr(X21, DENTS_OFFSET);

        a.label("list_directory_entry");
        a.ldrh(X23, X21, 16); // d_reclen
        a.add_imm(X9, X21, 19); // d_name

        // Skip "." and "..".
        a.ldrb(X10, X9, 0);
        a.movz_reg(X11, 46); // '.'
        a.cmp_reg_reg(X10, X11);
        a.bcond_label(CC_NE, "list_directory_keep");
        a.ldrb(X10, X9, 1);
        a.cmp_reg_reg(X10, XZR);
        a.bcond_label(CC_EQ, "list_directory_skip");
        a.movz_reg(X11, 46);
        a.cmp_reg_reg(X10, X11);
        a.bcond_label(CC_NE, "list_directory_keep");
        a.ldrb(X10, X9, 2);
        a.cmp_reg_reg(X10, XZR);
        a.bcond_label(CC_EQ, "list_directory_skip");

        a.label("list_directory_keep");
        a.mov_reg_reg(X0, X9);
        a.bl_label("cstr_copy");
        a.mov_reg_reg(X1, X0);
        a.mov_reg_reg(X0, X20);
        a.bl_label("array_add");
        a.mov_reg_reg(X20, X0);

        a.label("list_directory_skip");
        a.add_reg_reg(X21, X21, X23);
        a.sub_reg_reg(X22, X22, X23);
        a.cmp_reg_reg(X22, XZR);
        a.bcond_label(CC_GT, "list_directory_entry");
        a.b_label("list_directory_fill");

        a.label("list_directory_done");
        a.mov_reg_reg(X0, X19);
        a.mov_imm64(X8, SYS_CLOSE);
        a.svc();
        a.mov_reg_reg(X0, X20);
        a.ldr(X19, SP, 0);
        a.ldr(X20, SP, 8);
        a.ldr(X21, SP, 16);
        a.ldr(X22, SP, 24);
        a.ldr(X23, SP, 32);
        a.ldr(X30, SP, 40);
        a.add_imm(SP, SP, 64);
        a.ret();

        a.label("list_directory_fail");
        a.mov_abs(X0, error);
        a.b_label("runtime_fail");
    }

    /// `run_command`: X0 = command text, result is the shell's exit status.
    fn emit_run_command(&mut self) {
        let a = &mut self.asm;

        a.label("run_command");
        a.sub_imm(SP, SP, 32);
        a.str(X19, SP, 0);
        a.str(X30, SP, 8);

        a.bl_label("path_copy");
        // argv = ["/bin/sh", "-c", command, NULL]
        a.mov_data_addr(X1, RUN_ARGV_OFFSET);
        a.mov_data_addr(X2, RUN_SH_OFFSET);
        a.str(X2, X1, 0);
        a.mov_data_addr(X2, RUN_DASH_C_OFFSET);
        a.str(X2, X1, 8);
        a.mov_data_addr(X2, PATH_OFFSET);
        a.str(X2, X1, 16);
        a.str(XZR, X1, 24);

        a.movz_reg(X0, 17); // SIGCHLD
        a.movz_reg(X1, 0);
        a.movz_reg(X2, 0);
        a.movz_reg(X3, 0);
        a.movz_reg(X4, 0);
        a.mov_imm64(X8, SYS_CLONE);
        a.svc();
        a.cmp_reg_reg(X0, XZR);
        a.bcond_label(CC_EQ, "run_command_child");
        a.bcond_label(CC_LT, "run_command_fail");
        a.mov_reg_reg(X19, X0); // child pid

        a.mov_reg_reg(X0, X19);
        a.mov_data_addr(X1, RUN_STATUS_OFFSET);
        a.movz_reg(X2, 0);
        a.movz_reg(X3, 0);
        a.mov_imm64(X8, SYS_WAIT4);
        a.svc();
        a.mov_data_addr(X1, RUN_STATUS_OFFSET);
        a.ldr(X0, X1, 0);
        a.mov_imm64(X1, 256);
        a.udiv_reg_reg(X0, X0, X1); // WEXITSTATUS
        a.ldr(X19, SP, 0);
        a.ldr(X30, SP, 8);
        a.add_imm(SP, SP, 32);
        a.ret();

        a.label("run_command_child");
        a.mov_data_addr(X0, ARGS_SP_OFFSET);
        a.ldr(X0, X0, 0); // initial stack pointer
        a.ldr(X1, X0, 0); // argc
        a.add_imm(X2, X0, 8); // end of argv
        a.add_reg_reg(X3, X1, X1);
        a.add_reg_reg(X3, X3, X3);
        a.add_reg_reg(X3, X3, X3);
        a.add_reg_reg(X2, X2, X3);
        a.add_imm(X2, X2, 8); // envp
        a.mov_data_addr(X0, RUN_SH_OFFSET);
        a.mov_data_addr(X1, RUN_ARGV_OFFSET);
        a.mov_imm64(X8, SYS_EXECVE);
        a.svc();
        // The shell could not be started; report it like a shell would.
        a.mov_imm64(X8, SYS_EXIT);
        a.movz_reg(X0, 127);
        a.svc();

        a.label("run_command_fail");
        a.movz_reg(X0, 127);
        a.ldr(X19, SP, 0);
        a.ldr(X30, SP, 8);
        a.add_imm(SP, SP, 32);
        a.ret();
    }

    /// `text_copy`: X0 = text, result is a fresh copy of it in X0.
    fn emit_text_copy(&mut self) {
        let a = &mut self.asm;

        a.label("text_copy");
        a.sub_imm(SP, SP, 32);
        a.str(X19, SP, 0);
        a.str(X20, SP, 8);
        a.str(X30, SP, 16);

        a.mov_reg_reg(X19, X0); // source text
        a.ldr(X20, X19, 0); // length
        a.add_imm(X0, X20, 8);
        a.bl_label("heap_alloc");
        a.mov_reg_reg(X6, X0); // destination
        a.str(X20, X6, 0);

        a.add_imm(X3, X19, 8);
        a.add_imm(X4, X6, 8);
        a.mov_reg_reg(X9, X20);
        a.label("tcopy_loop");
        a.cmp_reg_reg(X9, XZR);
        a.bcond_label(CC_EQ, "tcopy_done");
        a.ldrb(X10, X3, 0);
        a.strb(X10, X4, 0);
        a.add_imm(X3, X3, 1);
        a.add_imm(X4, X4, 1);
        a.sub_imm(X9, X9, 1);
        a.b_label("tcopy_loop");

        a.label("tcopy_done");
        a.mov_reg_reg(X0, X6);
        a.ldr(X19, SP, 0);
        a.ldr(X20, SP, 8);
        a.ldr(X30, SP, 16);
        a.add_imm(SP, SP, 32);
        a.ret();
    }

    /// `array_copy_scalar`: X0 = list of numbers or truth values. A fresh list
    /// is allocated and the words are copied into it.
    fn emit_array_copy_scalar(&mut self) {
        let a = &mut self.asm;

        a.label("array_copy_scalar");
        a.sub_imm(SP, SP, 32);
        a.str(X19, SP, 0);
        a.str(X20, SP, 8);
        a.str(X30, SP, 16);

        a.mov_reg_reg(X19, X0); // source list
        a.ldr(X20, X19, 0); // length
        a.mov_reg_reg(X0, X20);
        a.add_reg_reg(X0, X0, X0);
        a.add_reg_reg(X0, X0, X0);
        a.add_reg_reg(X0, X0, X0); // length * 8
        a.add_imm(X0, X0, 16);
        a.bl_label("heap_alloc");
        a.mov_reg_reg(X6, X0); // destination
        a.str(X20, X6, 0); // length
        a.str(X20, X6, 8); // capacity

        a.add_imm(X1, X19, 16);
        a.add_imm(X3, X6, 16);
        a.mov_reg_reg(X4, X20);
        a.label("acopy_scalar_loop");
        a.cmp_reg_reg(X4, XZR);
        a.bcond_label(CC_EQ, "acopy_scalar_done");
        a.ldr(X7, X1, 0);
        a.str(X7, X3, 0);
        a.add_imm(X1, X1, 8);
        a.add_imm(X3, X3, 8);
        a.sub_imm(X4, X4, 1);
        a.b_label("acopy_scalar_loop");

        a.label("acopy_scalar_done");
        a.mov_reg_reg(X0, X6);
        a.ldr(X19, SP, 0);
        a.ldr(X20, SP, 8);
        a.ldr(X30, SP, 16);
        a.add_imm(SP, SP, 32);
        a.ret();
    }

    /// `array_copy_text`: X0 = list of text. Each text item is copied too, so
    /// the new list and the old list do not share any heap value.
    fn emit_array_copy_text(&mut self) {
        let a = &mut self.asm;

        a.label("array_copy_text");
        a.sub_imm(SP, SP, 48);
        a.str(X19, SP, 0);
        a.str(X20, SP, 8);
        a.str(X21, SP, 16);
        a.str(X22, SP, 24);
        a.str(X23, SP, 32);
        // `text_copy` is reached with `bl`, which overwrites X30. Keep the
        // caller's link register in the frame or `ret` returns into the loop.
        a.str(X30, SP, 40);

        a.mov_reg_reg(X19, X0); // source list
        a.ldr(X21, X19, 0); // length
        a.mov_reg_reg(X0, X21);
        a.add_reg_reg(X0, X0, X0);
        a.add_reg_reg(X0, X0, X0);
        a.add_reg_reg(X0, X0, X0); // length * 8
        a.add_imm(X0, X0, 16);
        a.bl_label("heap_alloc");
        a.mov_reg_reg(X6, X0); // destination start
        a.str(X21, X6, 0); // length
        a.str(X21, X6, 8); // capacity

        a.mov_reg_reg(X23, X6); // destination start
        a.add_imm(X19, X19, 16);
        a.add_imm(X20, X6, 16);
        a.movz_reg(X22, 0);
        a.label("acopy_text_loop");
        a.cmp_reg_reg(X22, X21);
        a.bcond_label(CC_GE, "acopy_text_done");
        a.ldr(X0, X19, 0);
        a.bl_label("text_copy");
        a.str(X0, X20, 0);
        a.add_imm(X19, X19, 8);
        a.add_imm(X20, X20, 8);
        a.add_imm(X22, X22, 1);
        a.b_label("acopy_text_loop");

        a.label("acopy_text_done");
        a.mov_reg_reg(X0, X23);
        a.ldr(X19, SP, 0);
        a.ldr(X20, SP, 8);
        a.ldr(X21, SP, 16);
        a.ldr(X22, SP, 24);
        a.ldr(X23, SP, 32);
        a.ldr(X30, SP, 40);
        a.add_imm(SP, SP, 48);
        a.ret();
    }

    /// `heap_grow`: X0 = the smallest useful chunk. Maps at least one chunk,
    /// records it in the chunk list, and makes it the current bump region.
    fn emit_heap_grow(&mut self) {
        let a = &mut self.asm;

        a.label("heap_grow");
        a.mov_imm64(X2, HEAP_CHUNK_SIZE);
        a.cmp_reg_reg(X0, X2);
        a.bcond_label(CC_GE, "heap_grow_size_ok");
        a.mov_reg_reg(X0, X2);
        a.label("heap_grow_size_ok");
        // Round the chunk up to a 16-byte boundary.
        a.add_imm(X1, X0, 15);
        a.movz_reg(X2, 16);
        a.udiv_reg_reg(X1, X1, X2);
        a.mul_reg_reg(X1, X1, X2);
        a.mov_reg_reg(X12, X1);

        a.movz_reg(X0, 0); // let the kernel choose the address
        a.mov_reg_reg(X1, X12);
        a.movz_reg(X2, 3); // PROT_READ | PROT_WRITE
        a.mov_imm64(X3, 0x22); // MAP_PRIVATE | MAP_ANONYMOUS
        a.mov_imm64(X4, (-1i64) as u64);
        a.movz_reg(X5, 0);
        a.mov_imm64(X8, SYS_MMAP);
        a.svc();
        a.cmp_reg_reg(X0, XZR);
        a.bcond_label(CC_LT, "heap_fail");

        // Chunk header: [end][next]. The first block starts just after it.
        a.add_reg_reg(X2, X0, X12);
        a.str(X2, X0, 0);
        a.mov_data_addr(X3, HEAP_CHUNKS_OFFSET);
        a.ldr(X4, X3, 0);
        a.str(X4, X0, 8);
        a.str(X0, X3, 0);
        a.add_imm(X2, X0, 16);
        a.mov_data_addr(X3, HEAP_PTR_OFFSET);
        a.str(X2, X3, 0);
        a.add_reg_reg(X2, X0, X12);
        a.mov_data_addr(X3, HEAP_END_OFFSET);
        a.str(X2, X3, 0);
        a.ret();
    }

    /// `heap_alloc`: X0 = payload bytes, result is a 16-byte-aligned pointer.
    /// Freed blocks are reused before the bump pointer moves.
    fn emit_heap_alloc(&mut self) {
        let a = &mut self.asm;

        a.label("heap_alloc");
        a.sub_imm(SP, SP, 32);
        a.str(X19, SP, 0);
        a.str(X20, SP, 8);
        a.str(X30, SP, 16);

        // need = max(32, round_up_16(size + 16))
        a.add_imm(X1, X0, 31);
        a.movz_reg(X2, 16);
        a.udiv_reg_reg(X1, X1, X2);
        a.mul_reg_reg(X1, X1, X2);
        a.movz_reg(X2, 32);
        a.cmp_reg_reg(X1, X2);
        a.bcond_label(CC_GE, "alloc_min_ok");
        a.mov_reg_reg(X1, X2);
        a.label("alloc_min_ok");
        a.mov_reg_reg(X19, X1);

        // First fit on the free list.
        a.mov_data_addr(X3, HEAP_FREE_OFFSET);
        a.ldr(X4, X3, 0);
        a.movz_reg(X5, 0);
        a.label("alloc_free_loop");
        a.cmp_reg_reg(X4, XZR);
        a.bcond_label(CC_EQ, "alloc_bump");
        a.ldr(X6, X4, 0);
        a.cmp_reg_reg(X6, X19);
        a.bcond_label(CC_GE, "alloc_free_hit");
        a.mov_reg_reg(X5, X4);
        a.ldr(X4, X4, 8);
        a.b_label("alloc_free_loop");

        a.label("alloc_free_hit");
        a.ldr(X6, X4, 8); // the next free block
        a.cmp_reg_reg(X5, XZR);
        a.bcond_label(CC_EQ, "alloc_free_head");
        a.str(X6, X5, 8);
        a.b_label("alloc_free_unlinked");
        a.label("alloc_free_head");
        a.str(X6, X3, 0);
        a.label("alloc_free_unlinked");
        a.add_imm(X0, X4, 16);
        a.b_label("alloc_done");

        a.label("alloc_bump");
        a.mov_data_addr(X7, HEAP_PTR_OFFSET);
        a.ldr(X8, X7, 0);
        a.add_reg_reg(X9, X8, X19);
        a.mov_data_addr(X10, HEAP_END_OFFSET);
        a.ldr(X11, X10, 0);
        a.cmp_reg_reg(X9, X11);
        a.bcond_label(CC_LE, "alloc_commit");
        a.mov_reg_reg(X0, X19);
        a.bl_label("heap_grow");
        a.b_label("alloc_bump");

        a.label("alloc_commit");
        a.str(X9, X7, 0);
        a.str(X19, X8, 0);
        a.str(XZR, X8, 8);
        a.add_imm(X0, X8, 16);

        a.label("alloc_done");
        a.ldr(X19, SP, 0);
        a.ldr(X20, SP, 8);
        a.ldr(X30, SP, 16);
        a.add_imm(SP, SP, 32);
        a.ret();
    }

    /// `heap_free`: X0 = a payload pointer. Pointers outside the mapped chunks
    /// (string literals, for example) are ignored, so freeing is always safe.
    fn emit_heap_free(&mut self) {
        let a = &mut self.asm;

        a.label("heap_free");
        a.cmp_reg_reg(X0, XZR);
        a.bcond_label(CC_EQ, "free_done");
        a.sub_imm(X1, X0, 16);
        a.mov_data_addr(X2, HEAP_CHUNKS_OFFSET);
        a.ldr(X2, X2, 0);
        a.label("free_scan");
        a.cmp_reg_reg(X2, XZR);
        a.bcond_label(CC_EQ, "free_done");
        a.ldr(X3, X2, 0);
        a.cmp_reg_reg(X1, X2);
        a.bcond_label(CC_LT, "free_next");
        a.cmp_reg_reg(X1, X3);
        a.bcond_label(CC_GE, "free_next");
        a.mov_data_addr(X4, HEAP_FREE_OFFSET);
        a.ldr(X5, X4, 0);
        a.str(X5, X1, 8);
        a.str(X1, X4, 0);
        a.b_label("free_done");
        a.label("free_next");
        a.ldr(X2, X2, 8);
        a.b_label("free_scan");
        a.label("free_done");
        a.ret();
    }

    /// `heap_fail`: report a heap that has run out of room and leave.
    fn emit_heap_fail(&mut self) {
        let id = self.heap_error;
        let a = &mut self.asm;
        a.label("heap_fail");
        a.mov_abs(X0, id);
        a.b_label("runtime_fail");
    }

    /// `runtime_fail`: X0 points at a length-prefixed sentence. Write it to
    /// stderr and leave with status 1.
    fn emit_runtime_fail(&mut self) {
        let a = &mut self.asm;

        a.label("runtime_fail");
        a.ldr(X2, X0, 0);
        a.add_imm(X1, X0, 8);
        a.mov_imm64(X0, 2); // fd = stderr
        a.mov_imm64(X8, SYS_WRITE);
        a.svc();
        a.mov_imm64(X8, SYS_EXIT);
        a.mov_imm64(X0, 1);
        a.svc();
        a.ret();
    }
}

fn frame_offset(slot: u64) -> u32 {
    (FRAME_HEADER + slot * 8) as u32
}

/// How many stack slots a function body needs: one per `let`, one per
/// `repeat` counter, including nested blocks.
fn count_slots(stmts: &[Stmt]) -> u64 {
    let mut n = 0;
    for stmt in stmts {
        match stmt {
            Stmt::Let { .. } | Stmt::Const { .. } => n += 1,
            Stmt::Repeat { body, .. } => n += 1 + count_slots(body),
            Stmt::While { body, .. } => n += count_slots(body),
            Stmt::ForEach { body, .. } => n += 2 + count_slots(body),
            Stmt::ForEachIn { body, .. } => n += 3 + count_slots(body),
            Stmt::If { then, other, .. } => n += count_slots(then) + count_slots(other),
            _ => {}
        }
    }
    n
}

fn op_name(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Eq => "is",
        BinOp::NotEq => "is not",
        BinOp::Lt => "is less than",
        BinOp::LtEq => "is at most",
        BinOp::Gt => "is more than",
        BinOp::GtEq => "is at least",
        BinOp::And => "and",
        BinOp::Or => "or",
    }
}

fn at(line: usize, msg: String) -> String {
    format!("line {line}: {msg}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::lex;
    use crate::parser::parse;

    fn bake(src: &str) -> Result<Baked, String> {
        let prog = parse(lex(src).unwrap())?;
        compile(&prog)
    }

    #[test]
    fn hello_world_bakes() {
        let baked = bake("say \"hello\"\n").unwrap();
        assert!(baked.text.len() > 10);
        assert!(baked.rodata.len() > 8);
    }

    #[test]
    fn unknown_name_is_a_friendly_error() {
        let err = bake("say missing\n").unwrap_err();
        assert!(err.contains("don't know a thing called 'missing'"), "{err}");
    }

    #[test]
    fn type_errors_name_the_kind() {
        let err = bake("say \"a\" + 1\n").unwrap_err();
        assert!(err.contains("adds numbers or joins text"), "{err}");
    }

    #[test]
    fn redeclaring_needs_set() {
        let err = bake("let x be 1\nlet x be 2\n").unwrap_err();
        assert!(err.contains("already exists"), "{err}");
    }

    #[test]
    fn functions_bake() {
        let src = "\
to double with n as a number:
    give back n * 2
say double with 21
";
        let baked = bake(src).unwrap();
        assert!(baked.text.len() > 40);
    }

    #[test]
    fn too_many_parameters() {
        let params: Vec<String> = (1..=crate::types::MAX_PARAMS + 1)
            .map(|n| format!("p{n} as a number"))
            .collect();
        let src = format!("to wide with {}:\n    give back p1\n", params.join(" and "));
        assert!(bake(&src).is_err());
    }

    #[test]
    fn many_parameters_bake() {
        let params: Vec<String> = (1..=crate::types::MAX_PARAMS)
            .map(|n| format!("p{n} as a number"))
            .collect();
        let src = format!(
            "to wide with {}:\n    give back p1\nsay wide with {}\n",
            params.join(" and "),
            vec!["1"; crate::types::MAX_PARAMS].join(" and ")
        );
        assert!(bake(&src).is_ok());
    }

    #[test]
    fn function_values_and_closures_bake() {
        let src = "\
to double with n as a number:
    give back n * 2

let f be the function double
say call f with 21
say f with 4

let base be 10
let add_base be a function with n as a number:
    give back n + base

say call add_base with 5
";
        let baked = bake(src).unwrap();
        assert!(baked.text.len() > 200);
    }

    #[test]
    fn closures_can_capture_lists_and_nest() {
        let src = "\
to make with xs as a view of a list of numbers:
    let outer be a function with a as a number:
        let inner be a function with b as a number:
            give back a + b + item 1 of xs
        give back call inner with 2
    give back call outer with 1

let xs be [3]
say make with xs
";
        let baked = bake(src).unwrap();
        assert!(baked.text.len() > 200);
    }

    #[test]
    fn closures_capture_the_kind_they_had_in_scope() {
        let err = bake(
            "\
to make with n as a number:
    let bad be a function with m as a number:
        give back m + n
    give back call bad with 1
say make with \"text\"
",
        )
        .unwrap_err();
        assert!(err.contains("should be a number"), "{err}");
    }
}
