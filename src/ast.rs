//! The shape of a Rusk program, after parsing and before baking.

use std::fmt;

/// The kinds a list can hold. Lists are homogeneous and hold one of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Elem {
    Int,
    Str,
    Bool,
}

impl Elem {
    pub fn ty(self) -> Ty {
        match self {
            Elem::Int => Ty::Int,
            Elem::Str => Ty::Str,
            Elem::Bool => Ty::Bool,
        }
    }

    /// The plural used after "a list of".
    pub fn plural(self) -> &'static str {
        match self {
            Elem::Int => "numbers",
            Elem::Str => "text",
            Elem::Bool => "truth values",
        }
    }
}

/// The kinds of value Rusk understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ty {
    Int,
    Str,
    Bool,
    /// A homogeneous list: a pointer to `[len][item...]` at runtime.
    Array(Elem),
    /// What a function "gives back" when it uses no `give back` at all.
    Nothing,
}

impl Ty {
    pub fn elem(self) -> Option<Elem> {
        match self {
            Ty::Array(elem) => Some(elem),
            _ => None,
        }
    }
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Ty::Int => write!(f, "a number"),
            Ty::Str => write!(f, "text"),
            Ty::Bool => write!(f, "a truth value"),
            Ty::Array(elem) => write!(f, "a list of {}", elem.plural()),
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
    /// `length of TEXT` — how many bytes the text holds.
    Length(Box<Expr>),
    /// `letter N of TEXT` — one piece of text, counting from 1.
    Letter(Box<Expr>, Box<Expr>),
    /// `part START to END of TEXT` — inclusive, counting from 1.
    Part {
        text: Box<Expr>,
        from: Box<Expr>,
        to: Box<Expr>,
    },
    /// `[a, b, c]` — a new list whose kind comes from its items.
    Array(Vec<Expr>),
    /// `an empty list of numbers` — a list whose kind must be written down.
    EmptyArray(Elem),
    /// `item N of LIST` — one item, counting from 1.
    Item(Box<Expr>, Box<Expr>),
    /// `copy of VALUE` — a fresh copy of text or a list.
    Copy(Box<Expr>),
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
    /// `constant NAME be VALUE` — a name that cannot be changed with `set`.
    Const {
        name: String,
        value: Expr,
        line: usize,
    },
    Set {
        name: String,
        value: Expr,
        line: usize,
    },
    /// `set item N of LIST to VALUE`.
    SetItem {
        index: Expr,
        array: Expr,
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
    /// `for each NAME in LIST:` — visits each item in order.
    ForEachIn {
        name: String,
        array: Expr,
        body: Vec<Stmt>,
        line: usize,
    },
    /// `import "file.rk"` — expanded before type checking.
    Import {
        path: String,
        line: usize,
    },
    FuncDef {
        name: String,
        params: Vec<Param>,
        body: Vec<Stmt>,
        line: usize,
    },
    GiveBack(Expr, usize),
    /// `break` — leave the nearest enclosing loop.
    Break(usize),
    /// `continue` — skip to the next turn of the nearest enclosing loop.
    Continue(usize),
    Expr(Expr, usize),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub stmts: Vec<Stmt>,
}

impl Stmt {
    /// The source line this statement starts on.
    pub fn line(&self) -> usize {
        match self {
            Stmt::Say(_, line)
            | Stmt::Let { line, .. }
            | Stmt::Const { line, .. }
            | Stmt::Set { line, .. }
            | Stmt::SetItem { line, .. }
            | Stmt::If { line, .. }
            | Stmt::Repeat { line, .. }
            | Stmt::While { line, .. }
            | Stmt::ForEach { line, .. }
            | Stmt::ForEachIn { line, .. }
            | Stmt::FuncDef { line, .. }
            | Stmt::Import { line, .. }
            | Stmt::GiveBack(_, line)
            | Stmt::Break(line)
            | Stmt::Continue(line)
            | Stmt::Expr(_, line) => *line,
        }
    }
}
