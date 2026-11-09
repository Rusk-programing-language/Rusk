//! The shape of a Rusk program, after parsing and before baking.

use std::collections::HashSet;
use std::fmt;
use std::rc::Rc;

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
/// numbers. Function items carry their whole signature, so a list of function
/// values is as specific as a list of numbers.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Elem {
    base: Base,
    depth: u8,
    /// Present when the innermost item is a function value. `base` is then an
    /// unused placeholder so the shape helper can still report a depth.
    func: Option<Rc<FuncTy>>,
}

impl Elem {
    pub const INT: Elem = Elem {
        base: Base::Int,
        depth: 0,
        func: None,
    };
    pub const STR: Elem = Elem {
        base: Base::Str,
        depth: 0,
        func: None,
    };
    pub const BOOL: Elem = Elem {
        base: Base::Bool,
        depth: 0,
        func: None,
    };

    pub fn new(base: Base, depth: u8) -> Self {
        Elem {
            base,
            depth,
            func: None,
        }
    }

    pub fn function(ft: Rc<FuncTy>) -> Self {
        Elem {
            base: Base::Int,
            depth: 0,
            func: Some(ft),
        }
    }

    pub fn base(&self) -> Base {
        self.base
    }

    pub fn depth(&self) -> u8 {
        self.depth
    }

    pub fn is_func(&self) -> bool {
        self.func.is_some() && self.depth == 0
    }

    pub fn func(&self) -> Option<&Rc<FuncTy>> {
        if self.depth == 0 {
            self.func.as_ref()
        } else {
            None
        }
    }

    /// One more list around this item.
    pub fn nested(&self) -> Self {
        Elem {
            base: self.base,
            depth: self.depth + 1,
            func: self.func.clone(),
        }
    }

    /// The item shape inside one list layer, if this is a list shape.
    pub fn inner(&self) -> Option<Self> {
        if self.depth == 0 {
            None
        } else {
            Some(Elem {
                base: self.base,
                depth: self.depth - 1,
                func: self.func.clone(),
            })
        }
    }

    /// The kind of a value with this element shape.
    pub fn ty(&self) -> Ty {
        if self.depth == 0 {
            match &self.func {
                Some(ft) => Ty::Func(ft.clone()),
                None => match self.base {
                    Base::Int => Ty::Int,
                    Base::Str => Ty::Str,
                    Base::Bool => Ty::Bool,
                },
            }
        } else {
            Ty::Array(Elem {
                base: self.base,
                depth: self.depth - 1,
                func: self.func.clone(),
            })
        }
    }

    /// The element shape of a value kind, if it can sit in a list.
    pub fn from_ty(ty: &Ty) -> Option<Self> {
        match ty {
            Ty::Int => Some(Elem::INT),
            Ty::Str => Some(Elem::STR),
            Ty::Bool => Some(Elem::BOOL),
            Ty::Array(elem) => Some(elem.nested()),
            Ty::Func(ft) => Some(Elem::function(ft.clone())),
            Ty::Nothing => None,
        }
    }

    /// A stable, label-safe name for this shape.
    pub fn tag(&self) -> String {
        match (&self.func, self.depth) {
            (Some(ft), 0) => {
                use std::collections::hash_map::DefaultHasher;
                use std::hash::{Hash, Hasher};
                let mut hasher = DefaultHasher::new();
                ft.hash(&mut hasher);
                format!("func_{:x}", hasher.finish())
            }
            _ => format!("{}_{}", self.base.word(), self.depth),
        }
    }

    /// The phrase used after "a list of", e.g. `lists of numbers`.
    pub fn plural(&self) -> String {
        let mut out = match (&self.func, self.depth) {
            (Some(ft), 0) => format!("function values ({ft})"),
            _ => self.base.plural().to_string(),
        };
        for _ in 0..self.depth {
            out = format!("lists of {out}");
        }
        out
    }
}

/// The full type of a callable value: the kinds of its parameters and what it
/// gives back. Keeping the parameters here is what lets a function value be
/// passed to a parameter, stored in a list, or given back from another
/// function with its signature checked.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FuncTy {
    pub params: Vec<Ty>,
    pub ret: Ty,
}

impl FuncTy {
    pub fn arity(&self) -> usize {
        self.params.len()
    }
}

impl fmt::Display for FuncTy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.params.is_empty() {
            return write!(f, "a function giving back {}", self.ret);
        }
        let params: Vec<String> = self.params.iter().map(|t| t.to_string()).collect();
        write!(
            f,
            "a function taking {} giving back {}",
            params.join(", "),
            self.ret
        )
    }
}

/// The kinds of value Rusk understands.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Ty {
    Int,
    Str,
    Bool,
    /// A homogeneous list: a pointer to `[len][capacity][item...]` at runtime.
    Array(Elem),
    /// A callable value: a pointer to `[code][capture count][capture...]`.
    Func(Rc<FuncTy>),
    /// What a function "gives back" when it uses no `give back` at all.
    Nothing,
}

impl Ty {
    pub fn elem(&self) -> Option<Elem> {
        match self {
            Ty::Array(elem) => Some(elem.clone()),
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
            Ty::Func(ft) => write!(f, "{ft}"),
            Ty::Nothing => write!(f, "nothing"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: String,
    pub ty: Ty,
    /// `as a view of ...` — the function may read the value but not move or
    /// change it.
    pub borrowed: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Int(i64),
    Str(String),
    Bool(bool),
    Var(String),
    /// A call written `name with arg and arg`.
    Call(String, Vec<Expr>),
    /// `call VALUE with arg and arg` — call something held in a variable.
    CallValue {
        func: Box<Expr>,
        args: Vec<Expr>,
    },
    /// `the function NAME` — a named function as a value.
    FuncRef(String),
    /// `a function with ...:` — a closure whose free names are captured.
    Lambda {
        params: Vec<Param>,
        body: Vec<Stmt>,
    },
    /// `a view of VALUE` — a shared, read-only alias.
    View(Box<Expr>),
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
    /// `export "file.rk"`, `export "file.rk" as name`,
    /// `export name from "file.rk"`, or `export name, other`.
    Export {
        path: Option<String>,
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

/// The names a closure body reads from the world outside it. Parameters,
/// locals, and loop variables are not included; neither are names bound by a
/// nested function definition. The order is first-use order, so codegen can
/// build a stable capture list.
pub fn lambda_free_names(params: &[Param], body: &[Stmt]) -> Vec<String> {
    let mut bound: HashSet<String> = params.iter().map(|p| p.name.clone()).collect();
    let mut free = Vec::new();
    let mut seen = HashSet::new();
    collect_free_stmts(body, &mut bound, &mut free, &mut seen);
    free
}

fn note_free(
    name: &str,
    bound: &HashSet<String>,
    free: &mut Vec<String>,
    seen: &mut HashSet<String>,
) {
    if bound.contains(name) || !seen.insert(name.to_string()) {
        return;
    }
    free.push(name.to_string());
}

fn collect_free_expr(
    expr: &Expr,
    bound: &mut HashSet<String>,
    free: &mut Vec<String>,
    seen: &mut HashSet<String>,
) {
    match expr {
        Expr::Int(_) | Expr::Str(_) | Expr::Bool(_) | Expr::FuncRef(_) | Expr::ArgCount => {}
        Expr::Var(name) => note_free(name, bound, free, seen),
        Expr::Call(_, args) | Expr::CallValue { args, .. } | Expr::Array(args) => {
            if let Expr::CallValue { func, .. } = expr {
                collect_free_expr(func, bound, free, seen);
            }
            for arg in args {
                collect_free_expr(arg, bound, free, seen);
            }
        }
        Expr::Lambda { params, body } => {
            collect_free_lambda(params, body, bound, free, seen);
        }
        Expr::View(inner)
        | Expr::Length(inner)
        | Expr::Copy(inner)
        | Expr::Argument(inner)
        | Expr::Environment(inner)
        | Expr::ReadFile(inner)
        | Expr::Unary(_, inner) => collect_free_expr(inner, bound, free, seen),
        Expr::Letter(a, b) | Expr::Item(a, b) | Expr::Binary(_, a, b) => {
            collect_free_expr(a, bound, free, seen);
            collect_free_expr(b, bound, free, seen);
        }
        Expr::Part { text, from, to } => {
            collect_free_expr(text, bound, free, seen);
            collect_free_expr(from, bound, free, seen);
            collect_free_expr(to, bound, free, seen);
        }
        Expr::EmptyArray(_) | Expr::ReadLine | Expr::ReadAll => {}
    }
}

fn collect_free_lambda(
    params: &[Param],
    body: &[Stmt],
    outer: &HashSet<String>,
    free: &mut Vec<String>,
    seen: &mut HashSet<String>,
) {
    let mut inner = outer.clone();
    for param in params {
        inner.insert(param.name.clone());
    }
    collect_free_stmts(body, &mut inner, free, seen);
}

fn collect_free_block(
    body: &[Stmt],
    bound: &mut HashSet<String>,
    free: &mut Vec<String>,
    seen: &mut HashSet<String>,
) {
    let mut inner = bound.clone();
    collect_free_stmts(body, &mut inner, free, seen);
}

fn collect_free_stmts(
    stmts: &[Stmt],
    bound: &mut HashSet<String>,
    free: &mut Vec<String>,
    seen: &mut HashSet<String>,
) {
    for stmt in stmts {
        match stmt {
            Stmt::Say(expr, _)
            | Stmt::Warn(expr, _)
            | Stmt::Stop(expr, _)
            | Stmt::GiveBack(expr, _)
            | Stmt::Expr(expr, _) => collect_free_expr(expr, bound, free, seen),
            Stmt::Let { name, value, .. } | Stmt::Const { name, value, .. } => {
                collect_free_expr(value, bound, free, seen);
                bound.insert(name.clone());
            }
            Stmt::Set { name, value, .. } => {
                collect_free_expr(value, bound, free, seen);
                note_free(name, bound, free, seen);
            }
            Stmt::SetItem {
                index,
                array,
                value,
                ..
            } => {
                collect_free_expr(index, bound, free, seen);
                collect_free_expr(array, bound, free, seen);
                collect_free_expr(value, bound, free, seen);
            }
            Stmt::AddTo { value, name, .. } => {
                collect_free_expr(value, bound, free, seen);
                note_free(name, bound, free, seen);
            }
            Stmt::Remove { index, name, .. } => {
                if let Some(index) = index {
                    collect_free_expr(index, bound, free, seen);
                }
                note_free(name, bound, free, seen);
            }
            Stmt::WriteFile { text, path, .. } => {
                collect_free_expr(text, bound, free, seen);
                collect_free_expr(path, bound, free, seen);
            }
            Stmt::If {
                cond, then, other, ..
            } => {
                collect_free_expr(cond, bound, free, seen);
                collect_free_block(then, bound, free, seen);
                collect_free_block(other, bound, free, seen);
            }
            Stmt::Repeat { count, body, .. } => {
                collect_free_expr(count, bound, free, seen);
                collect_free_block(body, bound, free, seen);
            }
            Stmt::While { cond, body, .. } => {
                collect_free_expr(cond, bound, free, seen);
                collect_free_block(body, bound, free, seen);
            }
            Stmt::ForEach {
                name,
                from,
                to,
                body,
                ..
            } => {
                collect_free_expr(from, bound, free, seen);
                collect_free_expr(to, bound, free, seen);
                let mut inner = bound.clone();
                inner.insert(name.clone());
                collect_free_stmts(body, &mut inner, free, seen);
            }
            Stmt::ForEachIn {
                name, array, body, ..
            } => {
                collect_free_expr(array, bound, free, seen);
                let mut inner = bound.clone();
                inner.insert(name.clone());
                collect_free_stmts(body, &mut inner, free, seen);
            }
            // A named function is not a closure: it cannot see these locals.
            Stmt::FuncDef { .. } | Stmt::Import { .. } | Stmt::Export { .. } => {}
            Stmt::Break(_) | Stmt::Continue(_) => {}
        }
    }
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
            | Stmt::Export { line, .. }
            | Stmt::GiveBack(_, line)
            | Stmt::Warn(_, line)
            | Stmt::Stop(_, line)
            | Stmt::Break(line)
            | Stmt::Continue(line)
            | Stmt::Expr(_, line) => *line,
        }
    }
}
