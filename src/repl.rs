//! An interactive session for Rusk.
//!
//! This is a tree-walking companion to the compiler, not a second backend:
//! every submission is lexed, parsed, and type checked against the whole
//! session before it runs, and the evaluator follows the same AST the code
//! generator walks. Values and functions stay alive between lines, so the
//! prompt behaves like Python's or IRB's.
//!
//! Text is kept as bytes here too. The compiled language treats a string as
//! `[len][bytes]`, so `length of`, `letter`, and `part` must count and slice
//! bytes, not Unicode scalars, or the REPL would disagree with a baked program.

use crate::ast::*;
use crate::types::{self, Sig};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::Path;
use std::rc::Rc;

const MAX_DEPTH: usize = 2000;

#[derive(Debug, Clone, PartialEq)]
enum Value {
    Int(i64),
    Str(Vec<u8>),
    Bool(bool),
    /// A list is owned by one name at a time, just as a pointer is moved in a
    /// baked program. `copy of` makes a new list instead of aliasing.
    Array(Rc<RefCell<Vec<Value>>>),
    Nothing,
}

impl Value {
    /// A quoted, escape-safe form for `:vars`.
    fn display(&self) -> String {
        match self {
            Value::Int(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Str(bytes) => quote(bytes),
            Value::Array(items) => {
                let items = items.borrow();
                let parts: Vec<String> = items.iter().map(Value::display).collect();
                format!("[{}]", parts.join(", "))
            }
            Value::Nothing => "nothing".to_string(),
        }
    }
}

fn quote(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::from("\"");
    for ch in text.chars() {
        for escaped in ch.escape_default() {
            out.push(escaped);
        }
    }
    out.push('"');
    out
}

enum Flow {
    Normal,
    Return(Value),
    Break,
    Continue,
}

struct Func {
    params: Vec<Param>,
    body: Vec<Stmt>,
}

struct Session {
    stmts: Vec<Stmt>,
    globals: HashMap<String, Value>,
    constants: HashSet<String>,
    funcs: HashMap<String, Rc<Func>>,
    sigs: HashMap<String, Sig>,
    line: usize,
    depth: usize,
}

impl Session {
    fn new() -> Self {
        Session {
            stmts: Vec::new(),
            globals: HashMap::new(),
            constants: HashSet::new(),
            funcs: HashMap::new(),
            sigs: HashMap::new(),
            line: 0,
            depth: 0,
        }
    }

    fn reset(&mut self) {
        self.stmts.clear();
        self.globals.clear();
        self.constants.clear();
        self.funcs.clear();
        self.sigs.clear();
    }

    /// Type check a submission against the session, then run it statement by
    /// statement. Statements that run are remembered; a failing statement and
    /// everything after it are dropped.
    fn submit(&mut self, src: &str) -> Result<(), String> {
        self.submit_at(src, None)
    }

    /// Like `submit`, but resolves `import` paths relative to `base` first.
    fn submit_at(&mut self, src: &str, base: Option<&Path>) -> Result<(), String> {
        let program = crate::imports::expand(src, base)?;
        if program.stmts.is_empty() {
            return Ok(());
        }

        let mut all = self.stmts.clone();
        all.extend(program.stmts.iter().cloned());
        let whole = Program { stmts: all };
        self.sigs = types::check(&whole)?;
        crate::ownership::check(&whole, &self.sigs)?;

        let mut env = std::mem::take(&mut self.globals);
        let mut result = Ok(());
        for stmt in &program.stmts {
            match self.exec_stmt(stmt, &mut env) {
                Ok(Flow::Normal) => self.stmts.push(stmt.clone()),
                Ok(Flow::Return(_)) => {
                    result = Err(format!(
                        "line {}: 'give back' only works inside a function",
                        stmt.line()
                    ));
                    break;
                }
                Ok(Flow::Break) => {
                    result = Err(format!(
                        "line {}: 'break' only works inside a loop",
                        stmt.line()
                    ));
                    break;
                }
                Ok(Flow::Continue) => {
                    result = Err(format!(
                        "line {}: 'continue' only works inside a loop",
                        stmt.line()
                    ));
                    break;
                }
                Err(e) => {
                    result = Err(e);
                    break;
                }
            }
        }
        self.globals = env;
        result
    }

    // ---- statements ------------------------------------------------------

    fn exec_stmt(
        &mut self,
        stmt: &Stmt,
        env: &mut HashMap<String, Value>,
    ) -> Result<Flow, String> {
        self.line = stmt.line();
        match stmt {
            Stmt::Say(e, _) => {
                let value = self.eval(e, env)?;
                if value == Value::Nothing {
                    return Err(format!(
                        "line {}: that function doesn't give anything back, so there's nothing to say",
                        self.line
                    ));
                }
                write_value(&value, true)
                    .map_err(|e| format!("line {}: can't write to the screen: {e}", self.line))?;
                Ok(Flow::Normal)
            }
            Stmt::Let { name, value, .. } => {
                let v = self.eval(value, env)?;
                env.insert(name.clone(), v);
                self.constants.remove(name);
                Ok(Flow::Normal)
            }
            Stmt::Const { name, value, .. } => {
                let v = self.eval(value, env)?;
                env.insert(name.clone(), v);
                self.constants.insert(name.clone());
                Ok(Flow::Normal)
            }
            Stmt::Set { name, value, .. } => {
                if self.constants.contains(name) {
                    return Err(format!(
                        "line {}: '{name}' is a constant — it can't be changed with 'set'",
                        self.line
                    ));
                }
                let v = self.eval(value, env)?;
                match env.get_mut(name) {
                    Some(slot) => {
                        *slot = v;
                        Ok(Flow::Normal)
                    }
                    None => Err(format!(
                        "line {}: I don't know a thing called '{name}' — declare it with 'let {name} be ...' first",
                        self.line
                    )),
                }
            }
            Stmt::SetItem {
                index,
                array,
                value,
                ..
            } => {
                let index = self.eval_int(index, env)?;
                let items = match self.eval(array, env)? {
                    Value::Array(items) => items,
                    other => {
                        return Err(format!(
                            "line {}: 'set item' needs a list, but this is {}",
                            self.line,
                            kind_name(&other)
                        ))
                    }
                };
                let v = self.eval(value, env)?;
                let mut items = items.borrow_mut();
                if index < 1 || index as usize > items.len() {
                    return Err(format!("line {}: an item outside the list", self.line));
                }
                items[index as usize - 1] = v;
                Ok(Flow::Normal)
            }
            Stmt::AddTo { value, name, .. } => {
                if self.constants.contains(name) {
                    return Err(format!(
                        "line {}: '{name}' is a constant — it can't be grown",
                        self.line
                    ));
                }
                let v = self.eval(value, env)?;
                match env.get(name) {
                    Some(Value::Array(items)) => {
                        items.borrow_mut().push(v);
                        Ok(Flow::Normal)
                    }
                    Some(other) => Err(format!(
                        "line {}: 'add' needs a list, but '{name}' holds {}",
                        self.line,
                        kind_name(other)
                    )),
                    None => Err(format!(
                        "line {}: I don't know a thing called '{name}'",
                        self.line
                    )),
                }
            }
            Stmt::If {
                cond, then, other, ..
            } => {
                let value = self.eval(cond, env)?;
                match value {
                    Value::Bool(true) => self.exec_block(then, env),
                    Value::Bool(false) => self.exec_block(other, env),
                    other => Err(format!(
                        "line {}: 'if' needs a condition that is true or false, but this is {}",
                        self.line,
                        kind_name(&other)
                    )),
                }
            }
            Stmt::Repeat { count, body, .. } => {
                let count = self.eval_int(count, env)?;
                let mut done = 0i64;
                while done < count {
                    match self.exec_block(body, env)? {
                        Flow::Normal | Flow::Continue => {}
                        Flow::Break => break,
                        Flow::Return(v) => return Ok(Flow::Return(v)),
                    }
                    if done == i64::MAX {
                        break;
                    }
                    done += 1;
                }
                Ok(Flow::Normal)
            }
            Stmt::While { cond, body, .. } => loop {
                let value = self.eval(cond, env)?;
                match value {
                    Value::Bool(false) => return Ok(Flow::Normal),
                    Value::Bool(true) => match self.exec_block(body, env)? {
                        Flow::Normal | Flow::Continue => {}
                        Flow::Break => return Ok(Flow::Normal),
                        Flow::Return(v) => return Ok(Flow::Return(v)),
                    },
                    other => {
                        return Err(format!(
                            "line {}: 'while' needs a condition that is true or false, but this is {}",
                            self.line,
                            kind_name(&other)
                        ))
                    }
                }
            },
            Stmt::ForEach {
                name,
                from,
                to,
                body,
                ..
            } => self.exec_for_each(name, from, to, body, env),
            Stmt::ForEachIn {
                name,
                array,
                body,
                ..
            } => self.exec_for_each_in(name, array, body, env),
            Stmt::FuncDef {
                name, params, body, ..
            } => {
                self.funcs.insert(
                    name.clone(),
                    Rc::new(Func {
                        params: params.clone(),
                        body: body.clone(),
                    }),
                );
                Ok(Flow::Normal)
            }
            Stmt::Import { .. } => Err(format!(
                "line {}: this import wasn't resolved — imports only work at the top level",
                self.line
            )),
            Stmt::GiveBack(e, _) => Ok(Flow::Return(self.eval(e, env)?)),
            Stmt::Break(_) => Ok(Flow::Break),
            Stmt::Continue(_) => Ok(Flow::Continue),
            Stmt::Expr(e, _) => {
                let value = self.eval(e, env)?;
                if value != Value::Nothing {
                    write_value(&value, true).map_err(|e| {
                        format!("line {}: can't write to the screen: {e}", self.line)
                    })?;
                }
                Ok(Flow::Normal)
            }
        }
    }

    fn exec_block(
        &mut self,
        stmts: &[Stmt],
        env: &mut HashMap<String, Value>,
    ) -> Result<Flow, String> {
        for stmt in stmts {
            match self.exec_stmt(stmt, env)? {
                Flow::Normal => {}
                flow => return Ok(flow),
            }
        }
        Ok(Flow::Normal)
    }

    fn exec_for_each(
        &mut self,
        name: &str,
        from: &Expr,
        to: &Expr,
        body: &[Stmt],
        env: &mut HashMap<String, Value>,
    ) -> Result<Flow, String> {
        let start = self.eval_int(from, env)?;
        let end = self.eval_int(to, env)?;

        // The loop name lives only inside the body; put back what it meant.
        let previous = env.insert(name.to_string(), Value::Int(start));
        let mut i = start;
        loop {
            if i > end {
                break;
            }
            env.insert(name.to_string(), Value::Int(i));
            match self.exec_block(body, env) {
                Ok(Flow::Normal) | Ok(Flow::Continue) => {}
                Ok(Flow::Break) => break,
                Ok(flow) => {
                    restore(env, name, previous);
                    return Ok(flow);
                }
                Err(e) => {
                    restore(env, name, previous);
                    return Err(e);
                }
            }
            if i == i64::MAX {
                break;
            }
            i += 1;
        }
        restore(env, name, previous);
        Ok(Flow::Normal)
    }

    fn exec_for_each_in(
        &mut self,
        name: &str,
        array: &Expr,
        body: &[Stmt],
        env: &mut HashMap<String, Value>,
    ) -> Result<Flow, String> {
        let items = match self.eval(array, env)? {
            Value::Array(items) => items,
            other => {
                return Err(format!(
                    "line {}: 'for each ... in' needs a list, but this is {}",
                    self.line,
                    kind_name(&other)
                ))
            }
        };

        // The loop name lives only inside the body; put back what it meant.
        let previous = env.insert(name.to_string(), Value::Nothing);
        let mut i = 0usize;
        loop {
            let len = items.borrow().len();
            if i >= len {
                break;
            }
            let value = items.borrow()[i].clone();
            env.insert(name.to_string(), value);
            match self.exec_block(body, env) {
                Ok(Flow::Normal) | Ok(Flow::Continue) => {}
                Ok(Flow::Break) => break,
                Ok(flow) => {
                    restore(env, name, previous);
                    return Ok(flow);
                }
                Err(e) => {
                    restore(env, name, previous);
                    return Err(e);
                }
            }
            i += 1;
        }
        restore(env, name, previous);
        Ok(Flow::Normal)
    }

    // ---- expressions -----------------------------------------------------

    fn eval(
        &mut self,
        expr: &Expr,
        env: &mut HashMap<String, Value>,
    ) -> Result<Value, String> {
        match expr {
            Expr::Int(n) => Ok(Value::Int(*n)),
            Expr::Str(s) => Ok(Value::Str(s.as_bytes().to_vec())),
            Expr::Bool(b) => Ok(Value::Bool(*b)),
            Expr::Var(name) => {
                if let Some(value) = env.get(name) {
                    return Ok(value.clone());
                }
                if let Some(func) = self.funcs.get(name).cloned() {
                    if func.params.is_empty() {
                        return self.call(name, Vec::new());
                    }
                    return Err(format!(
                        "line {}: '{name}' needs {} thing{} — call it with 'with ...'",
                        self.line,
                        func.params.len(),
                        if func.params.len() == 1 { "" } else { "s" }
                    ));
                }
                Err(format!(
                    "line {}: I don't know a thing called '{name}' — declare it with 'let {name} be ...'",
                    self.line
                ))
            }
            Expr::Call(name, args) => {
                let mut values = Vec::with_capacity(args.len());
                for arg in args {
                    values.push(self.eval(arg, env)?);
                }
                self.call(name, values)
            }
            Expr::Length(inner) => match self.eval(inner, env)? {
                Value::Str(bytes) => Ok(Value::Int(bytes.len() as i64)),
                Value::Array(items) => Ok(Value::Int(items.borrow().len() as i64)),
                other => Err(format!(
                    "line {}: 'length of' counts the letters in text or the items in a list, but this is {}",
                    self.line,
                    kind_name(&other)
                )),
            },
            Expr::Letter(index, text) => {
                let index = self.eval_int(index, env)?;
                match self.eval(text, env)? {
                    Value::Str(bytes) => {
                        if index < 1 || index as usize > bytes.len() {
                            return Err(format!(
                                "line {}: a letter outside the text",
                                self.line
                            ));
                        }
                        Ok(Value::Str(vec![bytes[index as usize - 1]]))
                    }
                    other => Err(format!(
                        "line {}: 'letter ... of' needs text, but this is {}",
                        self.line,
                        kind_name(&other)
                    )),
                }
            }
            Expr::Part { text, from, to } => {
                let bytes = match self.eval(text, env)? {
                    Value::Str(bytes) => bytes,
                    other => {
                        return Err(format!(
                            "line {}: 'part ... of' needs text, but this is {}",
                            self.line,
                            kind_name(&other)
                        ))
                    }
                };
                let start = self.eval_int(from, env)?;
                let end = self.eval_int(to, env)?;
                let len = bytes.len() as i64;
                if start < 1 || start > len + 1 || end > len {
                    return Err(format!("line {}: a part outside the text", self.line));
                }
                if end < start {
                    return Ok(Value::Str(Vec::new()));
                }
                Ok(Value::Str(bytes[(start - 1) as usize..end as usize].to_vec()))
            }
            Expr::Array(items) => {
                if items.is_empty() {
                    return Err(
                        "an empty list needs a kind — write 'an empty list of numbers', 'an empty list of text', or 'an empty list of truth values'"
                            .to_string(),
                    );
                }
                let mut values = Vec::with_capacity(items.len());
                for item in items {
                    values.push(self.eval(item, env)?);
                }
                Ok(Value::Array(Rc::new(RefCell::new(values))))
            }
            Expr::EmptyArray(_) => Ok(Value::Array(Rc::new(RefCell::new(Vec::new())))),
            Expr::Copy(inner) => match self.eval(inner, env)? {
                Value::Str(bytes) => Ok(Value::Str(bytes)),
                Value::Array(items) => {
                    let copied = items.borrow().iter().map(copy_value).collect();
                    Ok(Value::Array(Rc::new(RefCell::new(copied))))
                }
                Value::Int(n) => Ok(Value::Int(n)),
                Value::Bool(b) => Ok(Value::Bool(b)),
                Value::Nothing => Err(format!(
                    "line {}: there's nothing to copy — that function gives nothing back",
                    self.line
                )),
            },
            Expr::Item(index, array) => {
                let index = self.eval_int(index, env)?;
                let items = match self.eval(array, env)? {
                    Value::Array(items) => items,
                    other => {
                        return Err(format!(
                            "line {}: 'item ... of' needs a list, but this is {}",
                            self.line,
                            kind_name(&other)
                        ))
                    }
                };
                let items = items.borrow();
                if index < 1 || index as usize > items.len() {
                    return Err(format!("line {}: an item outside the list", self.line));
                }
                Ok(items[index as usize - 1].clone())
            }
            Expr::Unary(op, inner) => {
                let value = self.eval(inner, env)?;
                match (op, value) {
                    (UnOp::Neg, Value::Int(n)) => Ok(Value::Int(n.wrapping_neg())),
                    (UnOp::Not, Value::Bool(b)) => Ok(Value::Bool(!b)),
                    (UnOp::Neg, other) => Err(format!(
                        "line {}: '-' flips a number, but this is {}",
                        self.line,
                        kind_name(&other)
                    )),
                    (UnOp::Not, other) => Err(format!(
                        "line {}: 'not' flips something true or false, but this is {}",
                        self.line,
                        kind_name(&other)
                    )),
                }
            }
            Expr::Binary(op @ (BinOp::And | BinOp::Or), left, right) => {
                let left = self.eval(left, env)?;
                let short = matches!(
                    (op, &left),
                    (BinOp::And, Value::Bool(false)) | (BinOp::Or, Value::Bool(true))
                );
                if short {
                    return Ok(left);
                }
                if !matches!(left, Value::Bool(_)) {
                    return Err(format!(
                        "line {}: '{}' joins truth values, but the left side is {}",
                        self.line,
                        op_word(*op),
                        kind_name(&left)
                    ));
                }
                let right = self.eval(right, env)?;
                match right {
                    Value::Bool(_) => Ok(right),
                    other => Err(format!(
                        "line {}: '{}' joins truth values, but the right side is {}",
                        self.line,
                        op_word(*op),
                        kind_name(&other)
                    )),
                }
            }
            Expr::Binary(op, left, right) => {
                let left = self.eval(left, env)?;
                let right = self.eval(right, env)?;
                self.binary(*op, left, right)
            }
        }
    }

    fn eval_int(
        &mut self,
        expr: &Expr,
        env: &mut HashMap<String, Value>,
    ) -> Result<i64, String> {
        match self.eval(expr, env)? {
            Value::Int(n) => Ok(n),
            other => Err(format!(
                "line {}: I expected a number here, but found {}",
                self.line,
                kind_name(&other)
            )),
        }
    }

    fn binary(&mut self, op: BinOp, left: Value, right: Value) -> Result<Value, String> {
        use BinOp::*;
        match op {
            Add => match (left, right) {
                (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a.wrapping_add(b))),
                (Value::Str(mut a), Value::Str(b)) => {
                    a.extend_from_slice(&b);
                    Ok(Value::Str(a))
                }
                (a, b) => Err(format!(
                    "line {}: '+' adds numbers or joins text, but this has {} and {}",
                    self.line,
                    kind_name(&a),
                    kind_name(&b)
                )),
            },
            Sub | Mul | Div => {
                let (Value::Int(a), Value::Int(b)) = (left, right) else {
                    return Err(format!(
                        "line {}: '{}' works on numbers",
                        self.line,
                        op_word(op)
                    ));
                };
                match op {
                    Sub => Ok(Value::Int(a.wrapping_sub(b))),
                    Mul => Ok(Value::Int(a.wrapping_mul(b))),
                    Div if b == 0 => Err(format!("line {}: I can't divide by zero", self.line)),
                    Div => Ok(Value::Int(a.wrapping_div(b))),
                    _ => unreachable!(),
                }
            }
            Eq | NotEq | Lt | LtEq | Gt | GtEq => {
                let ordering = match (&left, &right) {
                    (Value::Int(a), Value::Int(b)) => a.cmp(b),
                    (Value::Bool(a), Value::Bool(b)) => a.cmp(b),
                    (Value::Str(a), Value::Str(b)) => a.as_slice().cmp(b.as_slice()),
                    (a, b) => {
                        return Err(format!(
                            "line {}: '{}' can't compare {} with {}",
                            self.line,
                            op_word(op),
                            kind_name(a),
                            kind_name(b)
                        ))
                    }
                };
                let yes = match op {
                    Eq => ordering.is_eq(),
                    NotEq => ordering.is_ne(),
                    Lt => ordering.is_lt(),
                    LtEq => ordering.is_le(),
                    Gt => ordering.is_gt(),
                    GtEq => ordering.is_ge(),
                    _ => unreachable!(),
                };
                Ok(Value::Bool(yes))
            }
            And | Or => unreachable!("and/or are short-circuited before binary"),
        }
    }

    fn call(&mut self, name: &str, args: Vec<Value>) -> Result<Value, String> {
        let Some(func) = self.funcs.get(name).cloned() else {
            return Err(format!(
                "line {}: I don't know a function called '{name}'",
                self.line
            ));
        };
        if args.len() != func.params.len() {
            return Err(format!(
                "line {}: '{name}' takes {} thing{}, but got {}",
                self.line,
                func.params.len(),
                if func.params.len() == 1 { "" } else { "s" },
                args.len()
            ));
        }
        if self.depth >= MAX_DEPTH {
            return Err(format!(
                "line {}: '{name}' is nested too deeply — try a smaller problem",
                self.line
            ));
        }

        let mut env = HashMap::new();
        for (param, value) in func.params.iter().zip(args) {
            env.insert(param.name.clone(), value);
        }

        let saved_line = self.line;
        self.depth += 1;
        let mut result = Ok(Value::Nothing);
        for stmt in &func.body {
            match self.exec_stmt(stmt, &mut env) {
                Ok(Flow::Normal) => {}
                Ok(Flow::Return(value)) => {
                    result = Ok(value);
                    break;
                }
                Ok(Flow::Break) | Ok(Flow::Continue) => {
                    result = Err(format!(
                        "line {}: that control word only works inside a loop",
                        self.line
                    ));
                    break;
                }
                Err(e) => {
                    result = Err(e);
                    break;
                }
            }
        }
        self.depth -= 1;
        self.line = saved_line;
        result
    }

    // ---- commands --------------------------------------------------------

    /// Returns `false` when the session should end.
    fn command(&mut self, line: &str) -> Result<bool, String> {
        let line = line.trim();
        let (name, rest) = match line.split_once(char::is_whitespace) {
            Some((name, rest)) => (name, rest.trim()),
            None => (line, ""),
        };
        match name {
            "help" | "h" | "?" => {
                print_help();
                Ok(true)
            }
            "quit" | "exit" | "q" => Ok(false),
            "reset" | "clear" => {
                self.reset();
                println!("forgotten everything.");
                Ok(true)
            }
            "vars" => {
                self.show_vars();
                Ok(true)
            }
            "functions" | "funcs" => {
                self.show_funcs();
                Ok(true)
            }
            "load" => {
                if rest.is_empty() {
                    return Err("':load' needs a file name".to_string());
                }
                let path = Path::new(rest);
                let src = std::fs::read_to_string(path)
                    .map_err(|e| format!("can't read {rest}: {e}"))?;
                self.submit_at(&src, path.parent())
                    .map_err(|e| format!("in {rest}: {e}"))?;
                Ok(true)
            }
            other => Err(format!(
                "I don't know the command ':{other}' — try :help"
            )),
        }
    }

    fn show_vars(&self) {
        if self.globals.is_empty() {
            println!("nothing yet.");
            return;
        }
        let mut names: Vec<&String> = self.globals.keys().collect();
        names.sort();
        for name in names {
            println!("{name} = {}", self.globals[name].display());
        }
    }

    fn show_funcs(&self) {
        if self.funcs.is_empty() {
            println!("no functions yet.");
            return;
        }
        let mut names: Vec<&String> = self.funcs.keys().collect();
        names.sort();
        for name in names {
            let func = &self.funcs[name];
            let params = func
                .params
                .iter()
                .map(|p| format!("{} as {}", p.name, p.ty))
                .collect::<Vec<_>>()
                .join(" and ");
            let gives = self
                .sigs
                .get(name)
                .map(|sig| sig.ret)
                .unwrap_or(Ty::Nothing);
            if params.is_empty() {
                println!("to {name}: gives back {gives}");
            } else {
                println!("to {name} with {params}: gives back {gives}");
            }
        }
    }
}

/// A fresh copy for `copy of`. Text is already an owned byte vector; a list
/// gets a new vector, and its text items are cloned as well.
fn copy_value(value: &Value) -> Value {
    match value {
        Value::Int(n) => Value::Int(*n),
        Value::Bool(b) => Value::Bool(*b),
        Value::Str(bytes) => Value::Str(bytes.clone()),
        Value::Array(items) => {
            let copied = items.borrow().iter().map(copy_value).collect();
            Value::Array(Rc::new(RefCell::new(copied)))
        }
        Value::Nothing => Value::Nothing,
    }
}

fn restore(env: &mut HashMap<String, Value>, name: &str, previous: Option<Value>) {
    match previous {
        Some(value) => {
            env.insert(name.to_string(), value);
        }
        None => {
            env.remove(name);
        }
    }
}

fn kind_name(value: &Value) -> &'static str {
    match value {
        Value::Int(_) => "a number",
        Value::Str(_) => "text",
        Value::Bool(_) => "a truth value",
        Value::Array(_) => "a list",
        Value::Nothing => "nothing",
    }
}

fn op_word(op: BinOp) -> &'static str {
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

fn write_value(value: &Value, newline: bool) -> io::Result<()> {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    match value {
        Value::Int(n) => write!(out, "{n}")?,
        Value::Str(bytes) => out.write_all(bytes)?,
        Value::Bool(b) => write!(out, "{}", if *b { "true" } else { "false" })?,
        Value::Array(items) => {
            out.write_all(b"[")?;
            for (i, item) in items.borrow().iter().enumerate() {
                if i > 0 {
                    out.write_all(b", ")?;
                }
                write_value_to(&mut out, item)?;
            }
            out.write_all(b"]")?;
        }
        Value::Nothing => {}
    }
    if newline {
        out.write_all(b"\n")?;
    }
    out.flush()
}

fn write_value_to<W: Write>(out: &mut W, value: &Value) -> io::Result<()> {
    match value {
        Value::Int(n) => write!(out, "{n}")?,
        Value::Str(bytes) => out.write_all(bytes)?,
        Value::Bool(b) => write!(out, "{}", if *b { "true" } else { "false" })?,
        Value::Array(items) => {
            out.write_all(b"[")?;
            for (i, item) in items.borrow().iter().enumerate() {
                if i > 0 {
                    out.write_all(b", ")?;
                }
                write_value_to(out, item)?;
            }
            out.write_all(b"]")?;
        }
        Value::Nothing => {}
    }
    Ok(())
}

/// True when the line opens an indented block, ignoring colons inside text and
/// comments. This is what makes the prompt show `...` for a `to`/`if`/loop.
fn opens_block(line: &str) -> bool {
    let mut in_string = false;
    let mut escaped = false;
    let mut in_comment = false;
    let mut last_code = None;
    for ch in line.chars() {
        if in_comment {
            break;
        }
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '#' => in_comment = true,
            c if c.is_whitespace() => {}
            c => last_code = Some(c),
        }
    }
    last_code == Some(':')
}

fn starts_indented(line: &str) -> bool {
    line.starts_with(' ') || line.starts_with('\t')
}

fn read_line<R: BufRead>(
    reader: &mut R,
    interactive: bool,
    prompt: &str,
) -> io::Result<Option<String>> {
    if interactive {
        print!("{prompt}");
        io::stdout().flush()?;
    }
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    Ok(Some(line))
}

fn trim_newline(line: &str) -> &str {
    line.trim_end_matches(|c| c == '\n' || c == '\r')
}

/// The wordmark shown when the session starts on a terminal.
const BANNER: &str = "\
░█▀▄░█░█░█▀▀░█░█
░█▀▄░█░█░▀▀█░█▀▄
░▀░▀░▀▀▀░▀▀▀░▀░▀";

/// Run the interactive session on stdin/stdout.
pub fn run() -> Result<(), String> {
    let interactive = io::stdin().is_terminal();
    if interactive {
        println!("{BANNER}");
        println!();
        println!("Rusk {} — an English-like language", env!("CARGO_PKG_VERSION"));
        println!("Type :help for commands, :quit to leave.\n");
    }

    let stdin = io::stdin();
    let mut reader = stdin.lock();
    let mut session = Session::new();

    loop {
        let Some(first) = read_line(&mut reader, interactive, "rusk> ")
            .map_err(|e| format!("can't read the prompt: {e}"))?
        else {
            break;
        };
        let first = trim_newline(&first);
        if first.trim().is_empty() {
            continue;
        }
        if let Some(command) = first.trim().strip_prefix(':') {
            if !session.command(command)? {
                break;
            }
            continue;
        }

        let mut lines = vec![first.to_string()];
        if opens_block(first) || starts_indented(first) {
            loop {
                let Some(more) = read_line(&mut reader, interactive, "...   ")
                    .map_err(|e| format!("can't read the prompt: {e}"))?
                else {
                    break;
                };
                let more = trim_newline(&more);
                if more.trim().is_empty() {
                    break;
                }
                lines.push(more.to_string());
            }
        }

        let src = format!("{}\n", lines.join("\n"));
        if let Err(e) = session.submit(&src) {
            eprintln!("burnt: {e}");
        }
    }

    if interactive {
        println!();
    }
    Ok(())
}

fn print_help() {
    println!(
        "Commands
  :help            show this list
  :quit            leave the session
  :reset           forget every name and function
  :vars            show the names the session remembers
  :functions       show the functions the session remembers
  :load <file.rk>  run a file in this session

Type Rusk at the prompt. A line ending in ':' opens a block; finish it with a
blank line. An expression on its own line shows its value, as in Python."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_and_functions_persist_between_submissions() {
        let mut session = Session::new();
        session.submit("let x be 40\n").unwrap();
        session
            .submit("to double with n as a number:\n    give back n * 2\n\n")
            .unwrap();
        session.submit("let y be double with x + 1\n").unwrap();
        assert_eq!(session.globals["x"], Value::Int(40));
        assert_eq!(session.globals["y"], Value::Int(82));
    }

    #[test]
    fn text_operations_follow_byte_semantics() {
        let mut session = Session::new();
        session.submit("let s be \"hello\"\n").unwrap();
        session.submit("let n be length of s\n").unwrap();
        session.submit("let c be letter 2 of s\n").unwrap();
        session.submit("let m be part 2 to 4 of s\n").unwrap();
        assert_eq!(session.globals["n"], Value::Int(5));
        assert_eq!(session.globals["c"], Value::Str(b"e".to_vec()));
        assert_eq!(session.globals["m"], Value::Str(b"ell".to_vec()));
    }

    #[test]
    fn runtime_errors_keep_the_session_alive() {
        let mut session = Session::new();
        session.submit("let s be \"abc\"\n").unwrap();
        let err = session.submit("say letter 9 of s\n").unwrap_err();
        assert!(err.contains("outside the text"), "{err}");
        session.submit("let t be s + \"!\"\n").unwrap();
        assert_eq!(session.globals["t"], Value::Str(b"abc!".to_vec()));
    }

    #[test]
    fn type_errors_do_not_change_the_session() {
        let mut session = Session::new();
        session.submit("let x be 1\n").unwrap();
        let err = session.submit("let y be \"a\" + 1\n").unwrap_err();
        assert!(err.contains("joins text"), "{err}");
        assert!(!session.globals.contains_key("y"));
    }

    #[test]
    fn blocks_finish_at_a_blank_line() {
        assert!(opens_block("if x is less than 3:"));
        assert!(!opens_block("say \"ready:\""));
        assert!(!opens_block("say 1  # a note:"));
        assert!(starts_indented("    say 1"));
    }

    #[test]
    fn lists_persist_and_copy_of_makes_an_independent_list() {
        let mut session = Session::new();
        session.submit("let xs be [1, 2, 3]\n").unwrap();
        session.submit("set item 2 of xs to 9\n").unwrap();
        session.submit("let ys be copy of xs\n").unwrap();
        session.submit("set item 1 of ys to 7\n").unwrap();
        assert_eq!(session.globals["xs"].display(), "[1, 9, 3]");
        assert_eq!(session.globals["ys"].display(), "[7, 9, 3]");

        session
            .submit("let empty be an empty list of text\n")
            .unwrap();
        assert_eq!(session.globals["empty"].display(), "[]");
    }

    #[test]
    fn moving_a_name_removes_the_old_owner() {
        let mut session = Session::new();
        session.submit("let xs be [1, 2, 3]\n").unwrap();
        session.submit("let ys be xs\n").unwrap();
        let err = session.submit("say xs\n").unwrap_err();
        assert!(err.contains("was moved already"), "{err}");
        session.submit("say ys\n").unwrap();
    }

    #[test]
    fn for_each_in_visits_every_item() {
        let mut session = Session::new();
        session.submit("let xs be [1, 2, 3]\n").unwrap();
        session.submit("let total be 0\n").unwrap();
        session
            .submit("for each x in xs:\n    set total to total + x\n")
            .unwrap();
        assert_eq!(session.globals["total"], Value::Int(6));
        assert!(!session.globals.contains_key("x"));
    }

    #[test]
    fn a_constant_survives_a_refused_change() {
        let mut session = Session::new();
        session.submit("constant pi be 3\n").unwrap();
        let err = session.submit("set pi to 4\n").unwrap_err();
        assert!(err.contains("constant"), "{err}");
        assert_eq!(session.globals["pi"], Value::Int(3));
    }

    #[test]
    fn list_bounds_errors_keep_the_session_alive() {
        let mut session = Session::new();
        session.submit("let xs be [1, 2]\n").unwrap();
        let err = session.submit("say item 3 of xs\n").unwrap_err();
        assert!(err.contains("outside the list"), "{err}");
        session.submit("set item 2 of xs to 5\n").unwrap();
        assert_eq!(session.globals["xs"].display(), "[1, 5]");
    }

    #[test]
    fn the_banner_is_the_rusk_wordmark() {
        assert_eq!(
            BANNER,
            "░█▀▄░█░█░█▀▀░█░█\n░█▀▄░█░█░▀▀█░█▀▄\n░▀░▀░▀▀▀░▀▀▀░▀░▀"
        );
        assert_eq!(BANNER.lines().count(), 3);
    }

    #[test]
    fn load_resolves_imports_next_to_the_file() {
        let dir = std::env::temp_dir().join(format!("rusk-repl-load-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("helper.rk"),
            "to helper with n as a number:\n    give back n * 3\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("program.rk"),
            "import \"helper\"\nlet loaded be helper with 2\n",
        )
        .unwrap();

        let mut session = Session::new();
        session
            .command(&format!("load {}", dir.join("program.rk").display()))
            .unwrap();
        assert_eq!(session.globals["loaded"], Value::Int(6));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
