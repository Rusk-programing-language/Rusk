//! The shape of a Rusk program, after parsing and before baking.

use std::fmt;

/// The kinds of value Rusk understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ty {
    Int,
    Str,
    Bool,
    /// What a function "gives back" when it uses no `give back` at all.
    Nothing,
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Ty::Int => write!(f, "a number"),
            Ty::Str => write!(f, "text"),
            Ty::Bool => write!(f, "a truth value"),
            Ty::Nothing => write!(f, "nothing"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: String,
    pub ty: Ty,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Int(i64),
    Str(String),
    Bool(bool),
    Var(String),
    /// A call written `name with arg and arg`.
    Call(String, Vec<Expr>),
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UnOp {
    Neg,
    Not,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Eq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    Say(Expr, usize),
    Let {
        name: String,
        value: Expr,
        line: usize,
    },
    Set {
        name: String,
        value: Expr,
        line: usize,
    },
    If {
        cond: Expr,
        then: Vec<Stmt>,
        other: Vec<Stmt>,
        line: usize,
    },
    Repeat {
        count: Expr,
        body: Vec<Stmt>,
        line: usize,
    },
    /// `while CONDITION:` — runs the body for as long as the condition holds.
    While {
        cond: Expr,
        body: Vec<Stmt>,
        line: usize,
    },
    /// `for each NAME from START to END:` — counts from START to END inclusive.
    ForEach {
        name: String,
        from: Expr,
        to: Expr,
        body: Vec<Stmt>,
        line: usize,
    },
    FuncDef {
        name: String,
        params: Vec<Param>,
        body: Vec<Stmt>,
        line: usize,
    },
    GiveBack(Expr, usize),
    Expr(Expr, usize),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub stmts: Vec<Stmt>,
}
