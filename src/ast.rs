//! The shape of a Rusk program, after parsing and before baking.

use std::fmt;

/// The plain kinds a list can ultimately hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Base {
    Int,
    Str,
    Bool,
}

impl Base {
    fn plural(self) -> &'static str {
        match self {
            Base::Int => "numbers",
            Base::Str => "text",
            Base::Bool => "truth values",
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            Base::Int => "int",
            Base::Str => "str",
            Base::Bool => "bool",
        }
    }
}

/// The kind of one list item. A depth of 0 is a plain value; each extra level
/// wraps it in another list, so `{Int, depth: 2}` is a list of lists of
/// numbers. Keeping this compact and `Copy` lets `Ty` stay cheap even though
/// lists nest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Elem {
    base: Base,
    depth: u8,
}

impl Elem {
    pub const INT: Elem = Elem {
        base: Base::Int,
        depth: 0,
    };
    pub const STR: Elem = Elem {
        base: Base::Str,
        depth: 0,
    };
    pub const BOOL: Elem = Elem {
        base: Base::Bool,
        depth: 0,
    };

    pub fn new(base: Base, depth: u8) -> Self {
        Elem { base, depth }
    }

    pub fn base(self) -> Base {
        self.base
    }

    pub fn depth(self) -> u8 {
        self.depth
    }

    /// One more list around this item.
    pub fn nested(self) -> Self {
        Elem {
            base: self.base,
            depth: self.depth + 1,
        }
    }

    /// The kind of a value with this element shape.
    pub fn ty(self) -> Ty {
        if self.depth == 0 {
            match self.base {
                Base::Int => Ty::Int,
                Base::Str => Ty::Str,
                Base::Bool => Ty::Bool,
            }
        } else {
            Ty::Array(Elem {
                base: self.base,
                depth: self.depth - 1,
            })
        }
    }

    /// The element shape of a value kind, if it can sit in a list.
    pub fn from_ty(ty: Ty) -> Option<Self> {
        match ty {
            Ty::Int => Some(Elem::INT),
            Ty::Str => Some(Elem::STR),
            Ty::Bool => Some(Elem::BOOL),
            Ty::Array(elem) => Some(elem.nested()),
            Ty::Nothing => None,
        }
    }

    /// The phrase used after "a list of", e.g. `lists of numbers`.
    pub fn plural(self) -> String {
        let mut out = self.base.plural().to_string();
        for _ in 0..self.depth {
            out = format!("lists of {out}");
        }
        out
    }
}

/// The kinds of value Rusk understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ty {
    Int,
    Str,
    Bool,
    /// A homogeneous list: a pointer to `[len][capacity][item...]` at runtime.
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
    /// `argument N` — command-line argument N, counting from 1.
    Argument(Box<Expr>),
    /// `number of arguments` — how many command-line arguments were given.
    ArgCount,
    /// `environment NAME` — an environment variable's text, empty if unset.
    Environment(Box<Expr>),
    /// `read line` — one line from standard input, without its newline.
    ReadLine,
    /// `read all input` — everything left on standard input.
    ReadAll,
    /// `read file PATH` — a file's contents as text.
    ReadFile(Box<Expr>),
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
    /// `and` / `or`: logical conjunction and disjunction.
    And,
    Or,
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
    /// `add VALUE to LIST` — append one item, growing the list.
    AddTo {
        value: Expr,
        name: String,
        line: usize,
    },
    /// `remove last from LIST` or `remove item N from LIST`.
    Remove {
        index: Option<Expr>,
        name: String,
        line: usize,
    },
    /// `warn VALUE` — print to standard error with a newline.
    Warn(Expr, usize),
    /// `stop with CODE` — end the program with an exit status.
    Stop(Expr, usize),
    /// `write TEXT to file PATH` — replace a file's contents.
    WriteFile {
        text: Expr,
        path: Expr,
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
    /// `import "file.rk"`, `import "file.rk" as name`, or
    /// `from "file.rk" import name, other as alias`.
    Import {
        path: String,
        alias: Option<String>,
        names: Vec<(String, Option<String>)>,
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
            | Stmt::AddTo { line, .. }
            | Stmt::Remove { line, .. }
            | Stmt::WriteFile { line, .. }
            | Stmt::If { line, .. }
            | Stmt::Repeat { line, .. }
            | Stmt::While { line, .. }
            | Stmt::ForEach { line, .. }
            | Stmt::ForEachIn { line, .. }
            | Stmt::FuncDef { line, .. }
            | Stmt::Import { line, .. }
            | Stmt::GiveBack(_, line)
            | Stmt::Warn(_, line)
            | Stmt::Stop(_, line)
            | Stmt::Break(line)
            | Stmt::Continue(line)
            | Stmt::Expr(_, line) => *line,
        }
    }
}
