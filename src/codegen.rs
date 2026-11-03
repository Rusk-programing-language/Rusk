//! Walks the AST and emits AArch64 machine code.
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

use crate::aarch64::*;
use crate::ast::{BinOp, Elem, Expr, Param, Program, Stmt, Ty, UnOp};
use crate::elf::DATA_BASE;
use crate::types::Sig;
use std::collections::HashMap;

/// Offset of the itoa scratch buffer inside the data area.
const ITOA_OFFSET: u64 = 0x2000;
const ITOA_CAPACITY: u64 = 32;
/// Fixed slots in the zeroed data area for the text heap.
const HEAP_PTR_OFFSET: u64 = 0x1000;
const HEAP_START_OFFSET: u64 = 0x1_0000;
const HEAP_SIZE: u64 = 0x10_0000;
const HEAP_END: u64 = DATA_BASE + HEAP_START_OFFSET + HEAP_SIZE;
/// Where the runtime keeps the current call depth.
const DEPTH_OFFSET: u64 = 0x1800;
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
}

pub fn compile(program: &Program) -> Result<Baked, String> {
    let funcs = crate::types::check(program)?;
    crate::ownership::check(program, &funcs)?;
    let mut cg = Codegen::new(funcs);
    cg.heap_error = cg.intern(HEAP_ERROR);

    cg.asm.label("_start");
    // The text heap starts at a fixed spot; a bump pointer in the data area
    // remembers where the next joined string goes.
    cg.asm.mov_imm64(X1, DATA_BASE + HEAP_PTR_OFFSET);
    cg.asm.mov_imm64(X0, DATA_BASE + HEAP_START_OFFSET);
    cg.asm.str(X0, X1, 0);

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

    cg.emit_print_int();
    cg.emit_text_concat();
    cg.emit_text_compare();
    cg.emit_text_letter();
    cg.emit_text_part();
    cg.emit_array_get();
    cg.emit_array_set();
    cg.emit_text_copy();
    cg.emit_array_copy_scalar();
    cg.emit_array_copy_text();
    cg.emit_heap_fail();
    cg.emit_runtime_fail();

    if cg.next_global * 8 > HEAP_PTR_OFFSET {
        return Err("this program has too many top-level things to keep track of".to_string());
    }

    let bss = cg.bss_size();
    let (text, rodata) = cg.asm.finish();
    Ok(Baked { text, rodata, bss })
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
}

#[derive(Clone)]
struct LoopLabels {
    /// Where `break` jumps: the instruction after the loop.
    after: String,
    /// Where `continue` jumps: this loop's next-turn label.
    next: String,
}

impl Codegen {
    fn new(funcs: HashMap<String, Sig>) -> Self {
        Codegen {
            asm: Asm::new(),
            vars: HashMap::new(),
            funcs,
            frame: None,
            next_global: 0,
            label_counter: 0,
            strings: HashMap::new(),
            line: 0,
            heap_error: 0,
            loops: Vec::new(),
        }
    }

    fn new_label(&mut self, hint: &str) -> String {
        self.label_counter += 1;
        format!("{hint}_{}", self.label_counter)
    }

    fn bss_size(&self) -> u64 {
        (self.next_global * 8)
            .max(ITOA_OFFSET + ITOA_CAPACITY)
            .max(HEAP_START_OFFSET + HEAP_SIZE)
    }

    /// Reserve an 8-byte slot, in the current frame or at top level.
    fn alloc_var(&mut self, ty: Ty, constant: bool) -> Var {
        if self.frame.is_some() {
            let frame = self.frame.as_mut().unwrap();
            let slot = frame.next_slot;
            frame.next_slot += 1;
            Var {
                slot,
                ty,
                local: true,
                constant,
            }
        } else {
            let slot = self.next_global;
            self.next_global += 1;
            Var {
                slot,
                ty,
                local: false,
                constant,
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

    /// Fail with the heap message if `reg` has grown past the end of the heap.
    fn emit_heap_check(&mut self, reg: u8) {
        self.asm.mov_imm64(X8, HEAP_END);
        self.asm.cmp_reg_reg(reg, X8);
        self.asm.bcond_label(CC_GT, "heap_fail");
    }

    fn store_var(&mut self, var: &Var) {
        if var.local {
            self.asm.str(X0, X29, frame_offset(var.slot));
        } else {
            self.asm.mov_imm64(X1, DATA_BASE + var.slot * 8);
            self.asm.str(X0, X1, 0);
        }
    }

    fn load_var(&mut self, var: &Var) {
        if var.local {
            self.asm.ldr(X0, X29, frame_offset(var.slot));
        } else {
            self.asm.mov_imm64(X1, DATA_BASE + var.slot * 8);
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
            Stmt::Set { name, value, line } => {
                self.set(name, value).map_err(|m| at(*line, m))
            }
            Stmt::SetItem {
                index,
                array,
                value,
                line,
            } => self
                .set_item(index, array, value)
                .map_err(|m| at(*line, m)),
            Stmt::If {
                cond,
                then,
                other,
                line,
            } => self.if_(cond, then, other).map_err(|m| at(*line, m)),
            Stmt::Repeat { count, body, line } => {
                self.repeat(count, body).map_err(|m| at(*line, m))
            }
            Stmt::While { cond, body, line } => {
                self.while_(cond, body).map_err(|m| at(*line, m))
            }
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
                None => Err(at(
                    *line,
                    "'break' only works inside a loop".to_string(),
                )),
            },
            Stmt::Continue(line) => match self.loops.last() {
                Some(labels) => {
                    let next = labels.next.clone();
                    self.asm.b_label(&next);
                    Ok(())
                }
                None => Err(at(
                    *line,
                    "'continue' only works inside a loop".to_string(),
                )),
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
        }
    }

    fn declare(&mut self, name: &str, value: &Expr, constant: bool) -> Result<(), String> {
        if self.vars.contains_key(name) {
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
        let ret_ty = frame.ret_ty;
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
        if params.len() > 8 {
            return Err(format!("'{name}' has too many parameters (8 at most)"));
        }

        let slots = params.len() as u64 + count_slots(body);
        let frame_size = FRAME_HEADER + slots * 8;
        let frame_size = (frame_size + 15) / 16 * 16;
        if frame_size > 4000 {
            return Err(format!("'{name}' has too many things to keep track of"));
        }

        let ret_label = format!("ret_{name}");
        let saved_vars = std::mem::take(&mut self.vars);
        let saved_frame = self.frame.take();
        self.frame = Some(Frame {
            ret_label: ret_label.clone(),
            ret_ty: sig.ret,
            next_slot: params.len() as u64,
        });

        self.asm.label(&format!("fn_{name}"));
        self.asm.sub_imm(SP, SP, frame_size as u32);
        self.asm.str(X29, SP, 0);
        self.asm.str(X30, SP, 8);
        self.asm.add_imm(X29, SP, 0);

        // Refuse to let recursion run into the process stack. The counter is
        // global, so a deeply nested call reports a Rusk error instead of a
        // segmentation fault.
        let depth_ok = self.new_label("depth_ok");
        self.asm.mov_imm64(X9, DATA_BASE + DEPTH_OFFSET);
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

        for (i, p) in params.iter().enumerate() {
            self.vars.insert(
                p.name.clone(),
                Var {
                    slot: i as u64,
                    ty: p.ty,
                    local: true,
                    constant: false,
                },
            );
            self.asm.str(i as u8, X29, frame_offset(i as u64));
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
        self.asm.label(&ret_label);
        // Give the depth counter back before tearing down the frame.
        self.asm.mov_imm64(X1, DATA_BASE + DEPTH_OFFSET);
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
                if args.len() > 8 {
                    return Err("a call can pass at most 8 things".to_string());
                }
                for arg in args {
                    self.expr(arg)?;
                    self.asm.push_reg(X0);
                }
                for i in (0..args.len()).rev() {
                    self.asm.pop_reg(i as u8);
                }
                self.asm.bl_label(&format!("fn_{name}"));
                Ok(sig.ret)
            }
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
                    let this = match t {
                        Ty::Int => Elem::Int,
                        Ty::Str => Elem::Str,
                        Ty::Bool => Elem::Bool,
                        Ty::Array(_) => {
                            return Err(
                                "a list can hold numbers, text, or truth values, not other lists"
                                    .to_string(),
                            )
                        }
                        Ty::Nothing => {
                            return Err("nothing can go in a list".to_string());
                        }
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
                if n * 8 + 8 >= 4096 * 8 {
                    return Err("this list literal is too long".to_string());
                }
                let heap_ptr = DATA_BASE + HEAP_PTR_OFFSET;
                self.asm.mov_imm64(X5, heap_ptr);
                self.asm.ldr(X6, X5, 0);
                self.asm.mov_imm64(X7, n * 8 + 8);
                self.asm.add_reg_reg(X7, X6, X7);
                self.emit_heap_check(X7);
                self.asm.str(X7, X5, 0);
                self.asm.mov_imm64(X7, n);
                self.asm.str(X7, X6, 0);
                for i in (0..n).rev() {
                    self.asm.pop_reg(X0);
                    self.asm.str(X0, X6, (i * 8 + 8) as u32);
                }
                self.asm.mov_reg_reg(X0, X6);
                Ok(Ty::Array(elem.unwrap()))
            }
            Expr::EmptyArray(elem) => {
                let heap_ptr = DATA_BASE + HEAP_PTR_OFFSET;
                self.asm.mov_imm64(X5, heap_ptr);
                self.asm.ldr(X6, X5, 0);
                self.asm.add_imm(X7, X6, 8);
                self.emit_heap_check(X7);
                self.asm.str(X7, X5, 0);
                self.asm.movz_reg(X7, 0);
                self.asm.str(X7, X6, 0);
                self.asm.mov_reg_reg(X0, X6);
                Ok(Ty::Array(*elem))
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
                        match elem {
                            Elem::Str => self.asm.bl_label("array_copy_text"),
                            Elem::Int | Elem::Bool => self.asm.bl_label("array_copy_scalar"),
                        }
                        Ok(ty)
                    }
                    Ty::Int | Ty::Bool => Ok(ty),
                    Ty::Nothing => Err(
                        "there's nothing to copy — that function gives nothing back".to_string(),
                    ),
                }
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
        }
    }

    // ---- printing --------------------------------------------------------

    fn emit_say(&mut self, ty: Ty) {
        match ty {
            Ty::Int => {
                self.asm.bl_label("print_int");
                self.emit_newline();
            }
            Ty::Str => self.emit_print_text(),
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
                self.emit_print_text();
            }
            Ty::Array(elem) => self.emit_say_array(elem),
            Ty::Nothing => {}
        }
    }

    /// X0 points at a length-prefixed string: `[len: u64][bytes...]`.
    fn emit_write_text_value(&mut self) {
        self.asm.ldr(X2, X0, 0); // len
        self.asm.add_imm(X1, X0, 8); // bytes
        self.asm.mov_imm64(X0, 1); // fd = stdout
        self.asm.mov_imm64(X8, SYS_WRITE);
        self.asm.svc();
    }

    fn emit_print_text(&mut self) {
        self.emit_write_text_value();
        self.emit_newline();
    }

    /// Write an interned literal without a newline.
    fn emit_write_literal(&mut self, s: &str) {
        let id = self.intern(s);
        self.asm.mov_abs(X1, id);
        self.asm.add_imm(X1, X1, 8);
        self.asm.mov_imm64(X2, s.len() as u64);
        self.asm.mov_imm64(X0, 1);
        self.asm.mov_imm64(X8, SYS_WRITE);
        self.asm.svc();
    }

    /// X0 points at a list: `[len: u64][item: i64]...`. Print `[a, b, c]`.
    fn emit_say_array(&mut self, elem: Elem) {
        self.asm.sub_imm(SP, SP, 32);
        self.asm.str(X19, SP, 0);
        self.asm.str(X20, SP, 8);
        self.asm.str(X21, SP, 16);

        self.asm.mov_reg_reg(X19, X0); // list
        self.asm.ldr(X20, X19, 0); // length
        self.asm.add_imm(X19, X19, 8); // items
        self.emit_write_literal("[");

        self.asm.movz_reg(X21, 0); // index
        let l_loop = self.new_label("say_list");
        let l_first = self.new_label("say_first");
        let l_done = self.new_label("say_done");

        self.asm.label(&l_loop);
        self.asm.cmp_reg_reg(X21, X20);
        self.asm.bcond_label(CC_GE, &l_done);
        self.asm.cmp_reg_reg(X21, XZR);
        self.asm.bcond_label(CC_EQ, &l_first);
        self.emit_write_literal(", ");
        self.asm.label(&l_first);

        self.asm.ldr(X0, X19, 0);
        match elem {
            Elem::Int => self.asm.bl_label("print_int"),
            Elem::Str => self.emit_write_text_value(),
            Elem::Bool => {
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
                self.emit_write_text_value();
            }
        }

        self.asm.add_imm(X19, X19, 8);
        self.asm.add_imm(X21, X21, 1);
        self.asm.b_label(&l_loop);

        self.asm.label(&l_done);
        self.emit_write_literal("]");
        self.asm.ldr(X19, SP, 0);
        self.asm.ldr(X20, SP, 8);
        self.asm.ldr(X21, SP, 16);
        self.asm.add_imm(SP, SP, 32);
        self.emit_newline();
    }

    fn emit_newline(&mut self) {
        let id = self.intern("\n");
        self.asm.mov_abs(X1, id);
        self.asm.add_imm(X1, X1, 8);
        self.asm.mov_imm64(X2, 1);
        self.asm.mov_imm64(X0, 1);
        self.asm.mov_imm64(X8, SYS_WRITE);
        self.asm.svc();
    }

    /// `print_int`: X0 in, decimal digits out, no newline, no libc.
    ///
    /// Writes backwards from the end of a 32-byte buffer, so the digits come out
    /// in the right order without a second pass.
    fn emit_print_int(&mut self) {
        let buf_end = DATA_BASE + ITOA_OFFSET + ITOA_CAPACITY;
        let a = &mut self.asm;

        a.label("print_int");
        a.sub_imm(SP, SP, 32);
        a.str(X19, SP, 0);

        a.mov_imm64(X19, buf_end);
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
        a.sdiv_reg_reg(X7, X3, X5); // X7 = value / 10
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
        a.mov_imm64(X0, 1);
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
        let heap_ptr = DATA_BASE + HEAP_PTR_OFFSET;
        let a = &mut self.asm;

        a.label("text_concat");
        a.ldr(X2, X0, 0); // left length
        a.ldr(X3, X1, 0); // right length
        a.add_reg_reg(X4, X2, X3); // total length

        a.mov_imm64(X5, heap_ptr);
        a.ldr(X6, X5, 0); // new string starts here
        a.add_reg_reg(X7, X6, X4);
        a.add_imm(X7, X7, 8);
        a.mov_imm64(X8, HEAP_END);
        a.cmp_reg_reg(X7, X8);
        a.bcond_label(CC_GT, "heap_fail");
        a.str(X7, X5, 0); // bump the heap pointer
        a.str(X4, X6, 0); // write the length
        a.add_imm(X9, X6, 8); // destination bytes

        a.add_imm(X8, X0, 8); // left bytes
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
        a.add_imm(X8, X1, 8); // right bytes
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
        let heap_ptr = DATA_BASE + HEAP_PTR_OFFSET;
        let a = &mut self.asm;

        a.label("text_letter");
        a.ldr(X2, X1, 0); // length
        a.movz_reg(X3, 1);
        a.cmp_reg_reg(X0, X3);
        a.bcond_label(CC_LT, "letter_fail");
        a.cmp_reg_reg(X0, X2);
        a.bcond_label(CC_GT, "letter_fail");

        // A one-byte string is [len = 1][byte], nine bytes in all.
        a.mov_imm64(X5, heap_ptr);
        a.ldr(X6, X5, 0);
        a.add_imm(X7, X6, 9);
        a.mov_imm64(X8, HEAP_END);
        a.cmp_reg_reg(X7, X8);
        a.bcond_label(CC_GT, "heap_fail");
        a.str(X7, X5, 0);
        a.movz_reg(X3, 1);
        a.str(X3, X6, 0);

        a.add_imm(X1, X1, 8);
        a.sub_imm(X0, X0, 1);
        a.add_reg_reg(X1, X1, X0);
        a.ldrb(X3, X1, 0);
        a.strb(X3, X6, 8);
        a.mov_reg_reg(X0, X6);
        a.ret();

        a.label("letter_fail");
        a.mov_reg_reg(X0, X4);
        a.b_label("runtime_fail");
    }

    /// `text_part`: X0 = start, X1 = end (both 1-based and inclusive),
    /// X2 = text, result in X0. X4 holds the runtime error.
    fn emit_text_part(&mut self) {
        let heap_ptr = DATA_BASE + HEAP_PTR_OFFSET;
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
        a.bcond_label(CC_LT, "part_empty");

        // count = end - start + 1
        a.sub_reg_reg(X6, X1, X0);
        a.add_imm(X6, X6, 1);

        a.mov_imm64(X7, heap_ptr);
        a.ldr(X8, X7, 0);
        a.add_reg_reg(X9, X8, X6);
        a.add_imm(X9, X9, 8);
        a.mov_imm64(X10, HEAP_END);
        a.cmp_reg_reg(X9, X10);
        a.bcond_label(CC_GT, "heap_fail");
        a.str(X9, X7, 0);
        a.str(X6, X8, 0);

        // Source bytes start at text + 8 + (start - 1).
        a.sub_imm(X10, X0, 1);
        a.add_reg_reg(X10, X2, X10);
        a.add_imm(X10, X10, 8);
        a.add_imm(X11, X8, 8);
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
        a.ret();

        a.label("part_empty");
        a.mov_imm64(X7, heap_ptr);
        a.ldr(X8, X7, 0);
        a.add_imm(X9, X8, 8);
        a.mov_imm64(X10, HEAP_END);
        a.cmp_reg_reg(X9, X10);
        a.bcond_label(CC_GT, "heap_fail");
        a.str(X9, X7, 0);
        a.movz_reg(X9, 0);
        a.str(X9, X8, 0);
        a.mov_reg_reg(X0, X8);
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
        a.add_imm(X0, X0, 8); // skip the length
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
        a.add_imm(X0, X0, 8);
        a.add_reg_reg(X0, X1, X0);
        a.str(X2, X0, 0);
        a.ret();

        a.label("array_set_fail");
        a.mov_reg_reg(X0, X4);
        a.b_label("runtime_fail");
    }

    /// `text_copy`: X0 = text, result is a fresh copy of it in X0.
    fn emit_text_copy(&mut self) {
        let heap_ptr = DATA_BASE + HEAP_PTR_OFFSET;
        let a = &mut self.asm;

        a.label("text_copy");
        a.ldr(X2, X0, 0); // length
        a.mov_reg_reg(X3, X0); // source text
        a.mov_imm64(X5, heap_ptr);
        a.ldr(X6, X5, 0); // destination
        a.add_reg_reg(X7, X6, X2);
        a.add_imm(X7, X7, 8);
        a.mov_imm64(X8, HEAP_END);
        a.cmp_reg_reg(X7, X8);
        a.bcond_label(CC_GT, "heap_fail");
        a.str(X7, X5, 0);
        a.str(X2, X6, 0);

        a.add_imm(X3, X3, 8);
        a.add_imm(X4, X6, 8);
        a.mov_reg_reg(X9, X2);
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
        a.ret();
    }

    /// `array_copy_scalar`: X0 = list of numbers or truth values. A fresh list
    /// is allocated and the words are copied into it.
    fn emit_array_copy_scalar(&mut self) {
        let heap_ptr = DATA_BASE + HEAP_PTR_OFFSET;
        let a = &mut self.asm;

        a.label("array_copy_scalar");
        a.mov_reg_reg(X1, X0); // source list
        a.ldr(X2, X1, 0); // length
        a.mov_imm64(X5, heap_ptr);
        a.ldr(X6, X5, 0); // destination

        a.mov_reg_reg(X7, X2);
        a.add_reg_reg(X7, X7, X7);
        a.add_reg_reg(X7, X7, X7);
        a.add_reg_reg(X7, X7, X7);
        a.add_imm(X7, X7, 8);
        a.mov_reg_reg(X9, X6);
        a.add_reg_reg(X9, X9, X7);
        a.mov_imm64(X8, HEAP_END);
        a.cmp_reg_reg(X9, X8);
        a.bcond_label(CC_GT, "heap_fail");
        a.str(X9, X5, 0);
        a.str(X2, X6, 0);

        a.add_imm(X1, X1, 8);
        a.add_imm(X3, X6, 8);
        a.mov_reg_reg(X4, X2);
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
        a.ret();
    }

    /// `array_copy_text`: X0 = list of text. Each text item is copied too, so
    /// the new list and the old list do not share any heap value.
    fn emit_array_copy_text(&mut self) {
        let heap_ptr = DATA_BASE + HEAP_PTR_OFFSET;
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

        a.mov_reg_reg(X19, X0); // source items cursor
        a.ldr(X21, X19, 0); // length
        a.mov_imm64(X5, heap_ptr);
        a.ldr(X6, X5, 0); // destination start
        a.mov_reg_reg(X7, X21);
        a.add_reg_reg(X7, X7, X7);
        a.add_reg_reg(X7, X7, X7);
        a.add_reg_reg(X7, X7, X7);
        a.add_imm(X7, X7, 8);
        a.mov_reg_reg(X9, X6);
        a.add_reg_reg(X9, X9, X7);
        a.mov_imm64(X8, HEAP_END);
        a.cmp_reg_reg(X9, X8);
        a.bcond_label(CC_GT, "heap_fail");
        a.str(X9, X5, 0);
        a.str(X21, X6, 0);

        a.mov_reg_reg(X23, X6); // destination start
        a.add_imm(X19, X19, 8);
        a.add_imm(X20, X6, 8);
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
        let src = "\
to wide with a as a number and b as a number and c as a number and d as a number and e as a number and f as a number and g as a number and h as a number and i as a number:
    give back a
";
        assert!(bake(src).is_err());
    }
}
