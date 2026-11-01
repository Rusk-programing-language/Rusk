//! The shape of a Rusk program, after parsing and before baking.

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Int(i64),
    Str(String),
    Bool(bool),
    Var(String),
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
    Expr(Expr, usize),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub stmts: Vec<Stmt>,
}
