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
use crate::ast::{BinOp, Expr, Param, Program, Stmt, Ty, UnOp};
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
/// Frame slots start after the saved frame pointer and return address.
const FRAME_HEADER: u64 = 16;

#[derive(Clone)]
struct Var {
    slot: u64,
    ty: Ty,
    local: bool,
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
    let mut cg = Codegen::new(funcs);

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

    /// Reserve an 8-byte number slot, in the current frame or at top level.
    fn alloc_int_slot(&mut self) -> Var {
        if self.frame.is_some() {
            let frame = self.frame.as_mut().unwrap();
            let slot = frame.next_slot;
            frame.next_slot += 1;
            Var {
                slot,
                ty: Ty::Int,
                local: true,
            }
        } else {
            let slot = self.next_global;
            self.next_global += 1;
            Var {
                slot,
                ty: Ty::Int,
                local: false,
            }
        }
    }

    fn intern(&mut self, s: &str) -> usize {
        if let Some(id) = self.strings.get(s) {
            return *id;
        }
        let id = self.asm.add_string(s);
        self.strings.insert(s.to_string(), id);
        id
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
                self.let_(name, value).map_err(|m| at(*line, m))
            }
            Stmt::Set { name, value, line } => {
                self.set(name, value).map_err(|m| at(*line, m))
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
            Stmt::GiveBack(e, line) => self.give_back(e).map_err(|m| at(*line, m)),
            Stmt::Expr(e, line) => self.expr(e).map(|_| ()).map_err(|m| at(*line, m)),
            Stmt::FuncDef { line, .. } => Err(at(
                *line,
                "functions have to live at the top level".to_string(),
            )),
        }
    }

    fn let_(&mut self, name: &str, value: &Expr) -> Result<(), String> {
        if self.vars.contains_key(name) {
            return Err(format!(
                "'{name}' already exists — use 'set {name} to ...' to change it"
            ));
        }
        let ty = self.expr(value)?;
        let var = if self.frame.is_some() {
            let slot = {
                let frame = self.frame.as_mut().unwrap();
                let slot = frame.next_slot;
                frame.next_slot += 1;
                slot
            };
            Var {
                slot,
                ty,
                local: true,
            }
        } else {
            let slot = self.next_global;
            self.next_global += 1;
            Var {
                slot,
                ty,
                local: false,
            }
        };
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
        let l_end = self.new_label("after");
        self.asm.label(&l_loop);
        self.load_var(&counter);
        self.asm.cmp_reg_reg(X0, XZR);
        self.asm.bcond_label(CC_LE, &l_end);
        for s in body {
            self.stmt(s)?;
        }
        self.load_var(&counter);
        self.asm.mov_imm64(X2, 1);
        self.asm.sub_reg_reg(X0, X0, X2);
        self.store_var(&counter);
        self.asm.b_label(&l_loop);
        self.asm.label(&l_end);
        Ok(())
    }

    fn while_(&mut self, cond: &Expr, body: &[Stmt]) -> Result<(), String> {
        let l_loop = self.new_label("while");
        let l_end = self.new_label("after");
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
        let l_end = self.new_label("after");
        self.asm.label(&l_loop);
        self.load_var(&item);
        self.asm.mov_reg_reg(X2, X0);
        self.load_var(&limit);
        self.asm.cmp_reg_reg(X2, X0);
        self.asm.bcond_label(CC_GT, &l_end);
        for s in body {
            self.stmt(s)?;
        }
        self.load_var(&item);
        self.asm.mov_imm64(X2, 1);
        self.asm.add_reg_reg(X0, X0, X2);
        self.store_var(&item);
        self.asm.b_label(&l_loop);
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

        for (i, p) in params.iter().enumerate() {
            self.vars.insert(
                p.name.clone(),
                Var {
                    slot: i as u64,
                    ty: p.ty,
                    local: true,
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
                    Div => self.asm.sdiv_reg_reg(X0, X1, X0),
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
            Ty::Nothing => {}
        }
    }

    /// X0 points at a length-prefixed string: `[len: u64][bytes...]`.
    fn emit_print_text(&mut self) {
        self.asm.ldr(X2, X0, 0); // len
        self.asm.add_imm(X1, X0, 8); // bytes
        self.asm.mov_imm64(X0, 1); // fd = stdout
        self.asm.mov_imm64(X8, SYS_WRITE);
        self.asm.svc();
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
            Stmt::Let { .. } => n += 1,
            Stmt::Repeat { body, .. } => n += 1 + count_slots(body),
            Stmt::While { body, .. } => n += count_slots(body),
            Stmt::ForEach { body, .. } => n += 2 + count_slots(body),
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
