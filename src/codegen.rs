//! Walks the AST and emits AArch64 machine code.
//!
//! Strategy: a straightforward stack-of-registers expression evaluator.
//! Every expression leaves its value in `X0`; binary operators push the left
//! side, evaluate the right side, then pop. Variables live in fixed 8-byte
//! slots above the file image, which the ELF loader zero-fills for us.
//!
//! No libc, no runtime: `say` is a `write(2)` syscall, `print_int` is a
//! hand-built itoa loop at the end of the text section, and the program ends
//! with `exit(2)`.

use crate::aarch64::*;
use crate::ast::{BinOp, Expr, Program, Stmt, UnOp};
use crate::elf::DATA_BASE;
use std::collections::HashMap;
use std::fmt;

/// Offset of the itoa scratch buffer inside the data area.
const ITOA_OFFSET: u64 = 0x2000;
const ITOA_CAPACITY: u64 = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ty {
    Int,
    Str,
    Bool,
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Ty::Int => write!(f, "a number"),
            Ty::Str => write!(f, "text"),
            Ty::Bool => write!(f, "a truth value"),
        }
    }
}

struct Var {
    slot: u64,
    ty: Ty,
}

#[derive(Debug)]
pub struct Baked {
    pub text: Vec<u8>,
    pub rodata: Vec<u8>,
    pub bss: u64,
}

pub fn compile(program: &Program) -> Result<Baked, String> {
    let mut cg = Codegen::new();
    cg.asm.label("_start");
    for stmt in &program.stmts {
        cg.stmt(stmt)?;
    }
    // exit(0)
    cg.asm.mov_imm64(X8, SYS_EXIT);
    cg.asm.mov_imm64(X0, 0);
    cg.asm.svc();

    cg.emit_print_int();

    let bss = cg.bss_size();
    let (text, rodata) = cg.asm.finish();
    Ok(Baked { text, rodata, bss })
}

struct Codegen {
    asm: Asm,
    vars: HashMap<String, Var>,
    strings: HashMap<String, usize>,
    next_slot: u64,
    label_counter: u64,
}

impl Codegen {
    fn new() -> Self {
        Codegen {
            asm: Asm::new(),
            vars: HashMap::new(),
            strings: HashMap::new(),
            next_slot: 0,
            label_counter: 0,
        }
    }

    fn new_label(&mut self, hint: &str) -> String {
        self.label_counter += 1;
        format!("{hint}_{}", self.label_counter)
    }

    fn alloc_slot(&mut self) -> u64 {
        let slot = self.next_slot;
        self.next_slot += 1;
        slot
    }

    fn slot_addr(slot: u64) -> u64 {
        DATA_BASE + slot * 8
    }

    fn bss_size(&self) -> u64 {
        (self.next_slot * 8).max(ITOA_OFFSET + ITOA_CAPACITY)
    }

    fn intern(&mut self, s: &str) -> usize {
        if let Some(id) = self.strings.get(s) {
            return *id;
        }
        let id = self.asm.add_string(s);
        self.strings.insert(s.to_string(), id);
        id
    }

    // ---- statements ------------------------------------------------------

    fn stmt(&mut self, stmt: &Stmt) -> Result<(), String> {
        match stmt {
            Stmt::Say(e, line) => self.say(e).map_err(|m| format!("line {line}: {m}")),
            Stmt::Let { name, value, line } => {
                self.let_(name, value).map_err(|m| format!("line {line}: {m}"))
            }
            Stmt::Set { name, value, line } => {
                self.set(name, value).map_err(|m| format!("line {line}: {m}"))
            }
            Stmt::If {
                cond,
                then,
                other,
                line,
            } => self
                .if_(cond, then, other)
                .map_err(|m| format!("line {line}: {m}")),
            Stmt::Repeat { count, body, line } => self
                .repeat(count, body)
                .map_err(|m| format!("line {line}: {m}")),
            Stmt::Expr(e, line) => self
                .expr(e)
                .map(|_| ())
                .map_err(|m| format!("line {line}: {m}")),
        }
    }

    fn say(&mut self, e: &Expr) -> Result<(), String> {
        let ty = self.expr(e)?;
        self.emit_say(ty);
        Ok(())
    }

    fn let_(&mut self, name: &str, value: &Expr) -> Result<(), String> {
        if self.vars.contains_key(name) {
            return Err(format!(
                "'{name}' already exists — use 'set {name} to ...' to change it"
            ));
        }
        let ty = self.expr(value)?;
        let slot = self.alloc_slot();
        self.asm.mov_imm64(X1, Self::slot_addr(slot));
        self.asm.str(X0, X1, 0);
        self.vars.insert(name.to_string(), Var { slot, ty });
        Ok(())
    }

    fn set(&mut self, name: &str, value: &Expr) -> Result<(), String> {
        let (slot, old_ty) = match self.vars.get(name) {
            Some(v) => (v.slot, v.ty),
            None => {
                return Err(format!(
                    "I don't know a thing called '{name}' — declare it with 'let {name} be ...' first"
                ))
            }
        };
        let ty = self.expr(value)?;
        if ty != old_ty {
            return Err(format!(
                "'{name}' holds {old_ty}, but that's {ty} — Rusk won't change a thing's kind"
            ));
        }
        self.asm.mov_imm64(X1, Self::slot_addr(slot));
        self.asm.str(X0, X1, 0);
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
        let slot = self.alloc_slot();
        let addr = Self::slot_addr(slot);
        self.asm.mov_imm64(X1, addr);
        self.asm.str(X0, X1, 0);

        let l_loop = self.new_label("loop");
        let l_end = self.new_label("after");
        self.asm.label(&l_loop);
        self.asm.mov_imm64(X1, addr);
        self.asm.ldr(X0, X1, 0);
        self.asm.cmp_reg_reg(X0, XZR);
        self.asm.bcond_label(CC_LE, &l_end);
        for s in body {
            self.stmt(s)?;
        }
        self.asm.mov_imm64(X1, addr);
        self.asm.ldr(X0, X1, 0);
        self.asm.mov_imm64(X2, 1);
        self.asm.sub_reg_reg(X0, X0, X2);
        self.asm.str(X0, X1, 0);
        self.asm.b_label(&l_loop);
        self.asm.label(&l_end);
        Ok(())
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
                let var = self.vars.get(name).ok_or_else(|| {
                    format!(
                        "I don't know a thing called '{name}' — declare it with 'let {name} be ...'"
                    )
                })?;
                let (slot, ty) = (var.slot, var.ty);
                self.asm.mov_imm64(X1, Self::slot_addr(slot));
                self.asm.ldr(X0, X1, 0);
                Ok(ty)
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
            Add | Sub | Mul | Div => {
                if lt != Ty::Int || rt != Ty::Int {
                    return Err(format!(
                        "'{}' works on numbers, but this has {lt} and {rt}",
                        op_name(op)
                    ));
                }
                // X1 = left, X0 = right.
                match op {
                    Add => self.asm.add_reg_reg(X0, X1, X0),
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
                if !matches!(lt, Ty::Int | Ty::Bool) {
                    return Err(format!(
                        "'{}' can't compare {lt} yet — text comparison is on the roadmap",
                        op_name(op)
                    ));
                }
                self.asm.cmp_reg_reg(X1, X0);
                let cc = match op {
                    Eq => CC_EQ,
                    NotEq => CC_NE,
                    Lt => CC_LT,
                    LtEq => CC_LE,
                    Gt => CC_GT,
                    GtEq => CC_GE,
                    _ => unreachable!(),
                };
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
        assert!(err.contains("works on numbers"), "{err}");
    }

    #[test]
    fn redeclaring_needs_set() {
        let err = bake("let x be 1\nlet x be 2\n").unwrap_err();
        assert!(err.contains("already exists"), "{err}");
    }
}
