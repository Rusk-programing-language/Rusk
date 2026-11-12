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
use crate::tui::{Ansi, Color, Key, RawMode, Terminal, BANNER};
use crate::types::{self, Sig};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::io::{self, BufRead, IsTerminal, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::rc::Rc;

const MAX_DEPTH: usize = 2000;

/// Internal marker for `stop with`. It travels as an ordinary error so a stop
/// inside a function or loop unwinds the evaluator; the REPL notices the
/// `stop` field and never shows this text.
const STOP: &str = "\0rusk-stop";

#[derive(Debug, Clone, PartialEq)]
enum Value {
    Int(i64),
    Str(Vec<u8>),
    Bool(bool),
    /// A list is owned by one name at a time, just as a pointer is moved in a
    /// baked program. `copy of` makes a new list instead of aliasing.
    Array(Rc<RefCell<Vec<Value>>>),
    /// A named function or closure. Captures are shared: scalars were copied
    /// away, and lists keep the same `Rc` the enclosing scope holds.
    Func(Rc<FuncValue>),
    Nothing,
}

/// A function value in the evaluator. The REPL has no heap records, so the
/// captures live beside the body instead of in an environment block.
#[derive(Debug, Clone, PartialEq)]
struct FuncValue {
    /// How to name this function in messages.
    label: String,
    params: Vec<Param>,
    body: Vec<Stmt>,
    captures: HashMap<String, Value>,
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
            Value::Func(_) => "<function>".to_string(),
            Value::Nothing => "nothing".to_string(),
        }
    }
}

/// Turn Rusk text into a filesystem path. Unix keeps the exact bytes; Windows
/// has to decode them as UTF-8 because its paths are UTF-16.
fn path_from_bytes(bytes: &[u8]) -> std::path::PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        std::path::PathBuf::from(OsStr::from_bytes(bytes))
    }
    #[cfg(not(unix))]
    {
        std::path::PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
    }
}

fn os_string_from_bytes(bytes: &[u8]) -> std::ffi::OsString {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        OsStr::from_bytes(bytes).to_os_string()
    }
    #[cfg(not(unix))]
    {
        std::ffi::OsString::from(String::from_utf8_lossy(bytes).into_owned())
    }
}

fn os_string_bytes(value: &OsStr) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        value.as_bytes().to_vec()
    }
    #[cfg(not(unix))]
    {
        value.to_string_lossy().into_owned().into_bytes()
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

/// A live network object at the prompt. The number the language sees is the
/// key in `Session::connections`, mirroring a file descriptor in a baked run.
enum Connection {
    Stream(TcpStream),
    Listener(TcpListener),
}

struct Session {
    stmts: Vec<Stmt>,
    globals: HashMap<String, Value>,
    constants: HashSet<String>,
    funcs: HashMap<String, Rc<Func>>,
    sigs: HashMap<String, Sig>,
    /// Command-line arguments for `argument N` and `number of arguments`.
    args: Vec<Vec<u8>>,
    /// When set, output is collected here instead of going to stdout.
    capture: Option<Vec<u8>>,
    /// The status from `stop with`, kept until the caller can restore the
    /// terminal before exiting.
    stop: Option<i32>,
    line: usize,
    depth: usize,
    /// Sockets and listeners opened by the session, numbered like descriptors.
    connections: HashMap<i64, Connection>,
    next_connection: i64,
    /// Raw input stays in place until `turn off raw terminal` or the session
    /// ends; dropping it restores the original terminal settings.
    raw_mode: Option<RawMode>,
}

impl Session {
    fn new() -> Self {
        Session {
            stmts: Vec::new(),
            globals: HashMap::new(),
            constants: HashSet::new(),
            funcs: HashMap::new(),
            sigs: HashMap::new(),
            args: Vec::new(),
            capture: None,
            stop: None,
            line: 0,
            depth: 0,
            connections: HashMap::new(),
            next_connection: 3,
            raw_mode: None,
        }
    }

    fn reset(&mut self) {
        self.stmts.clear();
        self.globals.clear();
        self.constants.clear();
        self.funcs.clear();
        self.sigs.clear();
        self.stop = None;
        self.connections.clear();
        self.raw_mode = None;
    }

    /// Give a new network object the next descriptor-like number.
    fn add_connection(&mut self, connection: Connection) -> i64 {
        let number = self.next_connection;
        self.next_connection += 1;
        self.connections.insert(number, connection);
        number
    }

    /// Collect everything the session prints instead of writing it to stdout.
    fn begin_capture(&mut self) {
        self.capture = Some(Vec::new());
    }

    /// Stop collecting and return what was printed as text.
    fn end_capture(&mut self) -> String {
        let bytes = self.capture.take().unwrap_or_default();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    fn write_out(&mut self, value: &Value, newline: bool) -> io::Result<()> {
        match self.capture.as_mut() {
            Some(buffer) => write_value_impl(buffer, value, newline),
            None => write_value(value, newline),
        }
    }

    fn write_warn(&mut self, value: &Value, newline: bool) -> io::Result<()> {
        match self.capture.as_mut() {
            Some(buffer) => write_value_impl(buffer, value, newline),
            None => write_value_err(value, newline),
        }
    }

    fn write_line(&mut self, text: &str) {
        match self.capture.as_mut() {
            Some(buffer) => {
                let _ = buffer.write_all(text.as_bytes());
                let _ = buffer.write_all(b"\n");
            }
            None => println!("{text}"),
        }
    }

    /// Read a name from the local environment, then from the globals.
    fn lookup(&self, env: &HashMap<String, Value>, name: &str) -> Option<Value> {
        env.get(name)
            .cloned()
            .or_else(|| self.globals.get(name).cloned())
    }

    /// Change the name where it lives: a local shadows a global.
    fn assign(&mut self, env: &mut HashMap<String, Value>, name: &str, value: Value) -> bool {
        if let Some(slot) = env.get_mut(name) {
            *slot = value;
            true
        } else if let Some(slot) = self.globals.get_mut(name) {
            *slot = value;
            true
        } else {
            false
        }
    }

    /// A top-level `let` or `constant` lives in the globals; one inside a
    /// function lives in that call's environment.
    fn declare(
        &mut self,
        env: &mut HashMap<String, Value>,
        name: &str,
        value: Value,
        constant: bool,
    ) {
        if self.depth == 0 {
            self.globals.insert(name.to_string(), value);
            if constant {
                self.constants.insert(name.to_string());
            } else {
                self.constants.remove(name);
            }
        } else {
            env.insert(name.to_string(), value);
        }
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

        let mut env = HashMap::new();
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
                Err(e) if self.stop.is_some() && e == STOP => {
                    result = Ok(());
                    break;
                }
                Err(e) => {
                    result = Err(e);
                    break;
                }
            }
        }
        result
    }

    // ---- statements ------------------------------------------------------

    fn exec_stmt(&mut self, stmt: &Stmt, env: &mut HashMap<String, Value>) -> Result<Flow, String> {
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
                self.write_out(&value, true)
                    .map_err(|e| format!("line {}: can't write to the screen: {e}", self.line))?;
                Ok(Flow::Normal)
            }
            Stmt::Let { name, value, .. } => {
                let v = self.eval(value, env)?;
                self.declare(env, name, v, false);
                Ok(Flow::Normal)
            }
            Stmt::Const { name, value, .. } => {
                let v = self.eval(value, env)?;
                self.declare(env, name, v, true);
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
                if self.assign(env, name, v) {
                    Ok(Flow::Normal)
                } else {
                    Err(format!(
                        "line {}: I don't know a thing called '{name}' — declare it with 'let {name} be ...' first",
                        self.line
                    ))
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
                let found = env
                    .get(name)
                    .cloned()
                    .or_else(|| self.globals.get(name).cloned());
                match found {
                    Some(Value::Array(items)) => {
                        items.borrow_mut().push(v);
                        Ok(Flow::Normal)
                    }
                    Some(other) => Err(format!(
                        "line {}: 'add' needs a list, but '{name}' holds {}",
                        self.line,
                        kind_name(&other)
                    )),
                    None => Err(format!(
                        "line {}: I don't know a thing called '{name}'",
                        self.line
                    )),
                }
            }
            Stmt::Remove { index, name, .. } => {
                if self.constants.contains(name) {
                    return Err(format!(
                        "line {}: '{name}' is a constant — it can't be changed",
                        self.line
                    ));
                }
                let index = match index {
                    Some(index) => Some(self.eval_int(index, env)?),
                    None => None,
                };
                let found = env
                    .get(name)
                    .cloned()
                    .or_else(|| self.globals.get(name).cloned());
                match found {
                    Some(Value::Array(items)) => {
                        let mut items = items.borrow_mut();
                        match index {
                            Some(index) => {
                                if index < 1 || index as usize > items.len() {
                                    return Err(format!(
                                        "line {}: an item outside the list",
                                        self.line
                                    ));
                                }
                                items.remove(index as usize - 1);
                            }
                            None => {
                                if items.pop().is_none() {
                                    return Err(format!(
                                        "line {}: there's nothing to remove",
                                        self.line
                                    ));
                                }
                            }
                        }
                        Ok(Flow::Normal)
                    }
                    Some(other) => Err(format!(
                        "line {}: 'remove' needs a list, but '{name}' holds {}",
                        self.line,
                        kind_name(&other)
                    )),
                    None => Err(format!(
                        "line {}: I don't know a thing called '{name}'",
                        self.line
                    )),
                }
            }
            Stmt::Warn(e, _) => {
                let value = self.eval(e, env)?;
                if value == Value::Nothing {
                    return Err(format!(
                        "line {}: that function doesn't give anything back, so there's nothing to warn about",
                        self.line
                    ));
                }
                self.write_warn(&value, true)
                    .map_err(|e| format!("line {}: can't write to the screen: {e}", self.line))?;
                Ok(Flow::Normal)
            }
            Stmt::Stop(e, _) => {
                let code = self.eval_int(e, env)?;
                self.stop = Some(code as i32);
                Err(STOP.to_string())
            }
            Stmt::WriteFile { text, path, .. } => {
                let text = match self.eval(text, env)? {
                    Value::Str(bytes) => bytes,
                    other => {
                        return Err(format!(
                            "line {}: 'write ... to file' needs the text as text, but this is {}",
                            self.line,
                            kind_name(&other)
                        ))
                    }
                };
                let path = match self.eval(path, env)? {
                    Value::Str(bytes) => bytes,
                    other => {
                        return Err(format!(
                            "line {}: 'write ... to file' needs the path as text, but this is {}",
                            self.line,
                            kind_name(&other)
                        ))
                    }
                };
                std::fs::write(path_from_bytes(&path), text)
                    .map_err(|_| format!("line {}: I can't write to that file", self.line))?;
                Ok(Flow::Normal)
            }
            Stmt::MakeDirectory { path, .. } => {
                let path = self.eval_text(path, env, "make directory")?;
                match std::fs::create_dir(path_from_bytes(&path)) {
                    Ok(()) => Ok(Flow::Normal),
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(Flow::Normal),
                    Err(_) => Err(format!("line {}: I can't make that directory", self.line)),
                }
            }
            Stmt::RemoveFile { path, .. } => {
                let path = self.eval_text(path, env, "remove file")?;
                match std::fs::remove_file(path_from_bytes(&path)) {
                    Ok(()) => Ok(Flow::Normal),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Flow::Normal),
                    Err(_) => Err(format!("line {}: I can't remove that file", self.line)),
                }
            }
            Stmt::RemoveDirectory { path, .. } => {
                let path = self.eval_text(path, env, "remove directory")?;
                match std::fs::remove_dir(path_from_bytes(&path)) {
                    Ok(()) => Ok(Flow::Normal),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Flow::Normal),
                    Err(_) => Err(format!("line {}: I can't remove that directory", self.line)),
                }
            }
            Stmt::RenamePath { from, to, .. } => {
                let from = self.eval_text(from, env, "rename")?;
                let to = self.eval_text(to, env, "rename")?;
                std::fs::rename(path_from_bytes(&from), path_from_bytes(&to))
                    .map_err(|_| format!("line {}: I can't rename that path", self.line))?;
                Ok(Flow::Normal)
            }
            Stmt::Send { text, socket, .. } => {
                let text = self.eval_text(text, env, "send")?;
                let socket = self.eval_int(socket, env)?;
                if let Some(Connection::Stream(stream)) = self.connections.get(&socket) {
                    let mut writer = stream;
                    let _ = writer.write_all(&text);
                }
                Ok(Flow::Normal)
            }
            Stmt::Close { socket, .. } => {
                let socket = self.eval_int(socket, env)?;
                self.connections.remove(&socket);
                Ok(Flow::Normal)
            }
            Stmt::RawTerminal { on, .. } => {
                if *on {
                    if self.raw_mode.is_none() {
                        self.raw_mode = crate::tui::enter_raw_input().ok().flatten();
                    }
                } else {
                    // Dropping the guard puts the saved settings back.
                    self.raw_mode = None;
                }
                Ok(Flow::Normal)
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
                name, array, body, ..
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
            Stmt::Export { .. } => Err(format!(
                "line {}: this export wasn't resolved — exports only work at the top level",
                self.line
            )),
            Stmt::GiveBack(e, _) => Ok(Flow::Return(self.eval(e, env)?)),
            Stmt::Break(_) => Ok(Flow::Break),
            Stmt::Continue(_) => Ok(Flow::Continue),
            Stmt::Expr(e, _) => {
                let value = self.eval(e, env)?;
                if value != Value::Nothing {
                    self.write_out(&value, true).map_err(|e| {
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

    fn eval(&mut self, expr: &Expr, env: &mut HashMap<String, Value>) -> Result<Value, String> {
        match expr {
            Expr::Int(n) => Ok(Value::Int(*n)),
            Expr::Str(s) => Ok(Value::Str(s.as_bytes().to_vec())),
            Expr::Bool(b) => Ok(Value::Bool(*b)),
            Expr::Var(name) => {
                if let Some(value) = self.lookup(env, name) {
                    return Ok(value);
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
                // `f with 1` reads like a named call, but `f` may hold a
                // function value. Prefer the named function when there is one.
                if !self.funcs.contains_key(name) {
                    if let Some(Value::Func(func)) = self.lookup(env, name) {
                        let mut values = Vec::with_capacity(args.len());
                        for arg in args {
                            values.push(self.eval(arg, env)?);
                        }
                        return self.call_func(&func, values);
                    }
                }
                let mut values = Vec::with_capacity(args.len());
                for arg in args {
                    values.push(self.eval(arg, env)?);
                }
                self.call(name, values)
            }
            Expr::CallValue { func, args } => {
                let func = match self.eval(func, env)? {
                    Value::Func(func) => func,
                    other => {
                        return Err(format!(
                            "line {}: 'call' needs a function value, but this is {}",
                            self.line,
                            kind_name(&other)
                        ))
                    }
                };
                let mut values = Vec::with_capacity(args.len());
                for arg in args {
                    values.push(self.eval(arg, env)?);
                }
                self.call_func(&func, values)
            }
            Expr::FuncRef(name) => match self.funcs.get(name).cloned() {
                Some(func) => Ok(Value::Func(Rc::new(FuncValue {
                    label: name.clone(),
                    params: func.params.clone(),
                    body: func.body.clone(),
                    captures: HashMap::new(),
                }))),
                None => Err(format!(
                    "line {}: I don't know a function called '{name}'",
                    self.line
                )),
            },
            Expr::Lambda { params, body } => {
                let mut captures = HashMap::new();
                for name in lambda_free_names(params, body) {
                    if let Some(value) = self.lookup(env, &name) {
                        captures.insert(name, value);
                    }
                }
                Ok(Value::Func(Rc::new(FuncValue {
                    label: "a function value".to_string(),
                    params: params.clone(),
                    body: body.clone(),
                    captures,
                })))
            }
            Expr::View(inner) => self.eval(inner, env),
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
                Value::Func(func) => Ok(Value::Func(func)),
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
            Expr::Argument(index) => {
                let n = self.eval_int(index, env)?;
                if n < 1 || n as usize > self.args.len() {
                    return Ok(Value::Str(Vec::new()));
                }
                Ok(Value::Str(self.args[n as usize - 1].clone()))
            }
            Expr::ArgCount => Ok(Value::Int(self.args.len() as i64)),
            Expr::Environment(name) => {
                let name = match self.eval(name, env)? {
                    Value::Str(bytes) => bytes,
                    other => {
                        return Err(format!(
                            "line {}: 'environment' needs a variable name as text, but this is {}",
                            self.line,
                            kind_name(&other)
                        ))
                    }
                };
                Ok(Value::Str(
                    std::env::var_os(os_string_from_bytes(&name))
                        .map(|v| os_string_bytes(&v))
                        .unwrap_or_default(),
                ))
            }
            Expr::ReadLine => Ok(Value::Str(read_stdin_line()?)),
            Expr::ReadAll => Ok(Value::Str(read_stdin_all()?)),
            Expr::ReadFile(path) => {
                let path = match self.eval(path, env)? {
                    Value::Str(bytes) => bytes,
                    other => {
                        return Err(format!(
                            "line {}: 'read file' needs a path as text, but this is {}",
                            self.line,
                            kind_name(&other)
                        ))
                    }
                };
                let bytes = std::fs::read(path_from_bytes(&path))
                    .map_err(|_| format!("line {}: I can't open that file", self.line))?;
                Ok(Value::Str(bytes))
            }
            Expr::ListDirectory(path) => {
                let path = self.eval_text(path, env, "list directory")?;
                let entries = std::fs::read_dir(path_from_bytes(&path))
                    .map_err(|_| format!("line {}: I can't list that directory", self.line))?;
                let mut names = Vec::new();
                for entry in entries.flatten() {
                    names.push(Value::Str(os_string_bytes(entry.file_name().as_os_str())));
                }
                Ok(Value::Array(Rc::new(RefCell::new(names))))
            }
            Expr::FileExists(path) => {
                let path = self.eval_text(path, env, "file exists")?;
                Ok(Value::Bool(path_from_bytes(&path).exists()))
            }
            Expr::AllArguments => Ok(Value::Array(Rc::new(RefCell::new(
                self.args
                    .iter()
                    .cloned()
                    .map(Value::Str)
                    .collect::<Vec<_>>(),
            )))),
            Expr::RunCommand(command) => {
                let command = self.eval_text(command, env, "run command")?;
                let mut cmd = if cfg!(windows) {
                    let mut c = std::process::Command::new("cmd");
                    c.arg("/C");
                    c
                } else {
                    let mut c = std::process::Command::new("sh");
                    c.arg("-c");
                    c
                };
                cmd.arg(os_string_from_bytes(&command));
                let status = cmd
                    .status()
                    .map_err(|_| format!("line {}: I can't run that command", self.line))?;
                Ok(Value::Int(status.code().unwrap_or(1) as i64))
            }
            Expr::Connect { host, port } => {
                let host = self.eval_text(host, env, "connect")?;
                let port = self.eval_int(port, env)?;
                if !(0..=65535).contains(&port) {
                    return Ok(Value::Int(-1));
                }
                let host = String::from_utf8_lossy(&host).into_owned();
                // Match a baked program: only a numeric dotted address is a
                // real host; every other name means localhost.
                let host = match host.parse::<std::net::Ipv4Addr>() {
                    Ok(address) => address.to_string(),
                    Err(_) => "127.0.0.1".to_string(),
                };
                match TcpStream::connect((host.as_str(), port as u16)) {
                    Ok(stream) => Ok(Value::Int(self.add_connection(Connection::Stream(stream)))),
                    Err(_) => Ok(Value::Int(-1)),
                }
            }
            Expr::Listen(port) => {
                let port = self.eval_int(port, env)?;
                if !(0..=65535).contains(&port) {
                    return Ok(Value::Int(-1));
                }
                match TcpListener::bind(("0.0.0.0", port as u16)) {
                    Ok(listener) => Ok(Value::Int(self.add_connection(Connection::Listener(listener)))),
                    Err(_) => Ok(Value::Int(-1)),
                }
            }
            Expr::Accept(listener) => {
                let listener = self.eval_int(listener, env)?;
                let accepted = match self.connections.get(&listener) {
                    Some(Connection::Listener(listener)) => listener.accept().ok(),
                    _ => None,
                };
                match accepted {
                    Some((stream, _)) => {
                        Ok(Value::Int(self.add_connection(Connection::Stream(stream))))
                    }
                    None => Ok(Value::Int(-1)),
                }
            }
            Expr::ReadFrom { socket, line } => {
                let socket = self.eval_int(socket, env)?;
                let mut out = Vec::new();
                if let Some(Connection::Stream(stream)) = self.connections.get(&socket) {
                    let mut reader = stream;
                    if *line {
                        let mut byte = [0u8; 1];
                        loop {
                            match reader.read(&mut byte) {
                                Ok(0) => break,
                                Ok(_) if byte[0] == b'\n' => break,
                                Ok(_) => out.push(byte[0]),
                                Err(_) => break,
                            }
                        }
                    } else {
                        let _ = reader.read_to_end(&mut out);
                    }
                    if *line && out.last() == Some(&b'\r') {
                        out.pop();
                    }
                }
                Ok(Value::Str(out))
            }
            Expr::ReadKey => Ok(Value::Str(read_stdin_byte()?)),
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

    fn eval_int(&mut self, expr: &Expr, env: &mut HashMap<String, Value>) -> Result<i64, String> {
        match self.eval(expr, env)? {
            Value::Int(n) => Ok(n),
            other => Err(format!(
                "line {}: I expected a number here, but found {}",
                self.line,
                kind_name(&other)
            )),
        }
    }

    fn eval_text(
        &mut self,
        expr: &Expr,
        env: &mut HashMap<String, Value>,
        what: &str,
    ) -> Result<Vec<u8>, String> {
        match self.eval(expr, env)? {
            Value::Str(bytes) => Ok(bytes),
            other => Err(format!(
                "line {}: '{what}' needs text here, but this is {}",
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

    /// Call a function value: run its body with captured names visible
    /// read-only, then bind the arguments over them.
    fn call_func(&mut self, func: &FuncValue, args: Vec<Value>) -> Result<Value, String> {
        if args.len() != func.params.len() {
            return Err(format!(
                "line {}: {} takes {} thing{}, but got {}",
                self.line,
                func.label,
                func.params.len(),
                if func.params.len() == 1 { "" } else { "s" },
                args.len()
            ));
        }
        if self.depth >= MAX_DEPTH {
            return Err(format!(
                "line {}: {} is nested too deeply — try a smaller problem",
                self.line, func.label
            ));
        }

        let mut env = func.captures.clone();
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
                self.write_line(help_text());
                Ok(true)
            }
            "quit" | "exit" | "q" => Ok(false),
            "reset" | "clear" => {
                self.reset();
                self.write_line("forgotten everything.");
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
                let src =
                    std::fs::read_to_string(path).map_err(|e| format!("can't read {rest}: {e}"))?;
                self.submit_at(&src, path.parent())
                    .map_err(|e| format!("in {rest}: {e}"))?;
                Ok(true)
            }
            other => Err(format!("I don't know the command ':{other}' — try :help")),
        }
    }

    fn show_vars(&mut self) {
        let mut lines = Vec::new();
        if self.globals.is_empty() {
            lines.push("nothing yet.".to_string());
        } else {
            let mut names: Vec<&String> = self.globals.keys().collect();
            names.sort();
            for name in names {
                lines.push(format!("{name} = {}", self.globals[name].display()));
            }
        }
        for line in lines {
            self.write_line(&line);
        }
    }

    fn show_funcs(&mut self) {
        let mut lines = Vec::new();
        if self.funcs.is_empty() {
            lines.push("no functions yet.".to_string());
        } else {
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
                    .map(|sig| sig.ret.clone())
                    .unwrap_or(Ty::Nothing);
                if params.is_empty() {
                    lines.push(format!("to {name}: gives back {gives}"));
                } else {
                    lines.push(format!("to {name} with {params}: gives back {gives}"));
                }
            }
        }
        for line in lines {
            self.write_line(&line);
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
        Value::Func(func) => Value::Func(func.clone()),
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
        Value::Func(_) => "a function",
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
    write_value_impl(&mut out, value, newline)
}

fn write_value_err(value: &Value, newline: bool) -> io::Result<()> {
    let stderr = io::stderr();
    let mut out = stderr.lock();
    write_value_impl(&mut out, value, newline)
}

fn write_value_impl<W: Write>(out: &mut W, value: &Value, newline: bool) -> io::Result<()> {
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
        Value::Func(_) => out.write_all(b"<function>")?,
        Value::Nothing => {}
    }
    if newline {
        out.write_all(b"\n")?;
    }
    out.flush()
}

fn read_stdin_line() -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match io::stdin().read(&mut byte) {
            Ok(0) => break,
            Ok(_) => match byte[0] {
                b'\n' => break,
                b'\r' => {}
                b => out.push(b),
            },
            Err(e) => return Err(format!("can't read standard input: {e}")),
        }
    }
    Ok(out)
}

fn read_stdin_all() -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    io::stdin()
        .read_to_end(&mut out)
        .map_err(|e| format!("can't read standard input: {e}"))?;
    Ok(out)
}

fn read_stdin_byte() -> Result<Vec<u8>, String> {
    let mut byte = [0u8; 1];
    match io::stdin().read(&mut byte) {
        Ok(0) => Ok(Vec::new()),
        Ok(_) => Ok(vec![byte[0]]),
        Err(e) => Err(format!("can't read standard input: {e}")),
    }
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
        Value::Func(_) => out.write_all(b"<function>")?,
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
    line.trim_end_matches(['\n', '\r'])
}

/// How many characters (not bytes) a string holds.
fn char_count(text: &str) -> usize {
    text.chars().count()
}

fn char_offset(text: &str, index: usize) -> usize {
    text.char_indices()
        .nth(index)
        .map(|(offset, _)| offset)
        .unwrap_or(text.len())
}

fn insert_char(text: &mut String, index: usize, ch: char) {
    let offset = char_offset(text, index);
    text.insert(offset, ch);
}

fn remove_char(text: &mut String, index: usize) {
    let offset = char_offset(text, index);
    if offset < text.len() {
        text.remove(offset);
    }
}

fn wrap_line(line: &str, width: usize) -> Vec<String> {
    if line.is_empty() {
        return vec![String::new()];
    }
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut used = 0;
    for ch in line.chars() {
        if used == width {
            lines.push(std::mem::take(&mut current));
            used = 0;
        }
        current.push(ch);
        used += 1;
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

fn append_output(transcript: &mut Vec<String>, output: &str) {
    for line in output.split_terminator('\n') {
        transcript.push(line.to_string());
    }
}

fn submit_in_tui(session: &mut Session, transcript: &mut Vec<String>, lines: &[String]) {
    let src = format!("{}\n", lines.join("\n"));
    session.begin_capture();
    let result = session.submit(&src);
    let output = session.end_capture();
    append_output(transcript, &output);
    if let Err(e) = result {
        if e != STOP {
            transcript.push(format!("burnt: {e}"));
        }
    }
}

fn draw_tui(
    terminal: &mut Terminal,
    transcript: &[String],
    current: &str,
    cursor: usize,
    pending: &[String],
    scroll: usize,
) -> Result<(), String> {
    let (screen_width, screen_height) = terminal.size();
    let width = screen_width.max(1) as usize;
    let height = screen_height.max(4) as usize;
    let area = height - 2;

    let wrapped: Vec<String> = transcript
        .iter()
        .flat_map(|line| wrap_line(line, width))
        .collect();
    let max_scroll = wrapped.len().saturating_sub(area);
    let scroll = scroll.min(max_scroll);
    let start = max_scroll - scroll;

    let prompt = if pending.is_empty() {
        "rusk> "
    } else {
        "...   "
    };

    let mut frame = Ansi::new();
    frame.clear();
    frame.hide_cursor();

    for row in 0..area {
        frame.move_to((row + 1) as u16, 1);
        frame.clear_line();
        if let Some(line) = wrapped.get(start + row) {
            frame.text(line);
        }
    }

    frame.move_to((area + 1) as u16, 1);
    frame.clear_line();
    frame.color(Color::BrightCyan);
    frame.text(prompt);
    frame.reset();
    frame.text(current);
    let column = prompt.chars().count() + cursor + 1;
    frame.move_to((area + 1) as u16, column.min(width).max(1) as u16);
    frame.show_cursor();

    frame.move_to(height as u16, 1);
    frame.clear_line();
    frame.color(Color::BrightBlack);
    frame.text("Enter run  ·  Up/Down history  ·  Ctrl-D quit  ·  Ctrl-L clear");
    frame.reset();

    terminal
        .draw(&frame)
        .map_err(|e| format!("can't draw the screen: {e}"))
}

/// A full-screen session built on the native terminal module. When stdin or
/// stdout is not a terminal, this falls back to the ordinary line session, so
/// `rusk tui < program.rk` still works.
pub fn run_tui() -> Result<(), String> {
    let mut terminal = Terminal::open().map_err(|e| format!("can't open the terminal: {e}"))?;
    if !terminal.is_interactive() {
        return run();
    }
    terminal
        .enter()
        .map_err(|e| format!("can't put the terminal in raw mode: {e}"))?;

    let mut session = Session::new();
    session.args = std::env::args()
        .skip(2)
        .map(|arg| arg.into_bytes())
        .collect();

    let mut transcript: Vec<String> = BANNER.lines().map(str::to_string).collect();
    transcript.push(String::new());
    transcript.push(format!(
        "Rusk {} — an English-like language",
        env!("CARGO_PKG_VERSION")
    ));
    transcript.push("Type :help for commands, :quit to leave.".to_string());
    transcript.push(String::new());

    let mut current = String::new();
    let mut cursor = 0usize;
    let mut pending: Vec<String> = Vec::new();
    let mut history: Vec<String> = Vec::new();
    let mut history_at: Option<usize> = None;
    let mut scroll = 0usize;
    let mut quit = false;

    while !quit {
        draw_tui(
            &mut terminal,
            &transcript,
            &current,
            cursor,
            &pending,
            scroll,
        )?;

        let key = match terminal.read_key() {
            Ok(key) => key,
            Err(e) => {
                let _ = terminal.leave();
                return Err(format!("can't read a key: {e}"));
            }
        };

        match key {
            Some(Key::Char(ch)) => {
                insert_char(&mut current, cursor, ch);
                cursor += 1;
                history_at = None;
            }
            Some(Key::Enter) => {
                let line = std::mem::take(&mut current);
                cursor = 0;
                let line_pending = !pending.is_empty();
                transcript.push(format!(
                    "{}{}",
                    if line_pending { "...   " } else { "rusk> " },
                    line
                ));
                if !line.trim().is_empty() {
                    history.push(line.clone());
                }
                history_at = None;

                if line_pending {
                    if line.trim().is_empty() {
                        let lines = std::mem::take(&mut pending);
                        submit_in_tui(&mut session, &mut transcript, &lines);
                        scroll = 0;
                        if session.stop.is_some() {
                            quit = true;
                        }
                    } else {
                        pending.push(line);
                    }
                } else if line.trim().is_empty() {
                    // An empty line at the first prompt is just a new prompt.
                } else if let Some(command) = line.trim().strip_prefix(':') {
                    session.begin_capture();
                    let result = session.command(command);
                    let output = session.end_capture();
                    append_output(&mut transcript, &output);
                    match result {
                        Ok(true) => {}
                        Ok(false) => quit = true,
                        Err(e) => transcript.push(format!("burnt: {e}")),
                    }
                    if session.stop.is_some() {
                        quit = true;
                    }
                    scroll = 0;
                } else if opens_block(&line) || starts_indented(&line) {
                    pending.push(line);
                } else {
                    submit_in_tui(&mut session, &mut transcript, &[line]);
                    scroll = 0;
                    if session.stop.is_some() {
                        quit = true;
                    }
                }
            }
            Some(Key::Backspace) => {
                if cursor > 0 {
                    cursor -= 1;
                    remove_char(&mut current, cursor);
                }
                history_at = None;
            }
            Some(Key::Delete) => {
                remove_char(&mut current, cursor);
                history_at = None;
            }
            Some(Key::Left) => {
                cursor = cursor.saturating_sub(1);
            }
            Some(Key::Right) => {
                cursor = (cursor + 1).min(char_count(&current));
            }
            Some(Key::Home) | Some(Key::Ctrl('a')) => cursor = 0,
            Some(Key::End) | Some(Key::Ctrl('e')) => cursor = char_count(&current),
            Some(Key::Tab) => {
                for _ in 0..4 {
                    insert_char(&mut current, cursor, ' ');
                    cursor += 1;
                }
                history_at = None;
            }
            Some(Key::Up) => {
                if !history.is_empty() {
                    let at = match history_at {
                        None => history.len() - 1,
                        Some(0) => 0,
                        Some(index) => index - 1,
                    };
                    history_at = Some(at);
                    current = history[at].clone();
                    cursor = char_count(&current);
                }
            }
            Some(Key::Down) => match history_at {
                Some(index) if index + 1 < history.len() => {
                    history_at = Some(index + 1);
                    current = history[index + 1].clone();
                    cursor = char_count(&current);
                }
                Some(_) => {
                    history_at = None;
                    current.clear();
                    cursor = 0;
                }
                None => {}
            },
            Some(Key::Ctrl('c')) => {
                current.clear();
                cursor = 0;
                pending.clear();
                history_at = None;
                transcript.push("^C".to_string());
                scroll = 0;
            }
            Some(Key::Ctrl('d')) => {
                if current.is_empty() && pending.is_empty() {
                    quit = true;
                } else if cursor < char_count(&current) {
                    remove_char(&mut current, cursor);
                }
            }
            Some(Key::Ctrl('l')) => {
                transcript.clear();
                scroll = 0;
            }
            Some(Key::Ctrl('u')) => {
                for _ in 0..cursor {
                    remove_char(&mut current, 0);
                }
                cursor = 0;
            }
            Some(Key::Ctrl('k')) => {
                while cursor < char_count(&current) {
                    remove_char(&mut current, cursor);
                }
            }
            Some(Key::PageUp) => {
                scroll = scroll.saturating_add((terminal.size().1.max(4) as usize - 2) / 2);
            }
            Some(Key::PageDown) => {
                scroll = scroll.saturating_sub((terminal.size().1.max(4) as usize - 2) / 2);
            }
            Some(Key::Resize(_, _)) | None => {}
            Some(Key::Eof) => quit = true,
            Some(_) => {}
        }
    }

    terminal
        .leave()
        .map_err(|e| format!("can't restore the terminal: {e}"))?;
    if let Some(code) = session.stop {
        io::stdout().flush().ok();
        io::stderr().flush().ok();
        std::process::exit(code);
    }
    Ok(())
}

/// Run the interactive session on stdin/stdout.
pub fn run() -> Result<(), String> {
    let interactive = io::stdin().is_terminal();
    if interactive {
        println!("{BANNER}");
        println!();
        println!(
            "Rusk {} — an English-like language",
            env!("CARGO_PKG_VERSION")
        );
        println!("Type :help for commands, :quit to leave.\n");
    }

    let stdin = io::stdin();
    let mut session = Session::new();
    session.args = std::env::args()
        .skip(2)
        .map(|arg| arg.into_bytes())
        .collect();

    loop {
        let Some(first) = read_line(&mut stdin.lock(), interactive, "rusk> ")
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
            if let Some(code) = session.stop {
                io::stdout().flush().ok();
                io::stderr().flush().ok();
                std::process::exit(code);
            }
            continue;
        }

        let mut lines = vec![first.to_string()];
        if opens_block(first) || starts_indented(first) {
            loop {
                let Some(more) = read_line(&mut stdin.lock(), interactive, "...   ")
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
            if e != STOP {
                eprintln!("burnt: {e}");
            }
        }
        if let Some(code) = session.stop {
            io::stdout().flush().ok();
            io::stderr().flush().ok();
            std::process::exit(code);
        }
    }

    if interactive {
        println!();
    }
    Ok(())
}

fn help_text() -> &'static str {
    "Commands
  :help            show this list
  :quit            leave the session
  :reset           forget every name and function
  :vars            show the names the session remembers
  :functions       show the functions the session remembers
  :load <file.rk>  run a file in this session

Type Rusk at the prompt. A line ending in ':' opens a block; finish it with a
blank line. An expression on its own line shows its value, as in Python."
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn functions_read_and_change_globals() {
        let mut session = Session::new();
        session.submit("let total be 0\n").unwrap();
        session
            .submit("to bump with n as a number:\n    set total to total + n\n\n")
            .unwrap();
        session.submit("bump with 2\n").unwrap();
        session.submit("bump with 3\n").unwrap();
        assert_eq!(session.globals["total"], Value::Int(5));
    }

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
    fn removing_items_shrinks_the_list_without_moving_it() {
        let mut session = Session::new();
        session.submit("let xs be [1, 2, 3, 4]\n").unwrap();
        session.submit("remove last from xs\n").unwrap();
        assert_eq!(session.globals["xs"].display(), "[1, 2, 3]");
        session.submit("remove item 1 from xs\n").unwrap();
        assert_eq!(session.globals["xs"].display(), "[2, 3]");
        session.submit("add 9 to xs\n").unwrap();
        assert_eq!(session.globals["xs"].display(), "[2, 3, 9]");
        session.submit("let ys be copy of xs\n").unwrap();
        session.submit("remove item 2 from ys\n").unwrap();
        assert_eq!(session.globals["xs"].display(), "[2, 3, 9]");
        assert_eq!(session.globals["ys"].display(), "[2, 9]");
    }

    #[test]
    fn removing_text_and_refused_removals_keep_the_session_alive() {
        let mut session = Session::new();
        session.submit("let words be [\"a\", \"b\"]\n").unwrap();
        session.submit("remove last from words\n").unwrap();
        assert_eq!(session.globals["words"].display(), "[\"a\"]");
        session.submit("remove last from words\n").unwrap();
        assert_eq!(session.globals["words"].display(), "[]");

        let err = session.submit("remove last from words\n").unwrap_err();
        assert!(err.contains("nothing to remove"), "{err}");

        session.submit("let xs be [1]\n").unwrap();
        let err = session.submit("remove item 2 from xs\n").unwrap_err();
        assert!(err.contains("outside the list"), "{err}");
        assert_eq!(session.globals["xs"].display(), "[1]");

        session.submit("let n be 5\n").unwrap();
        let err = session.submit("remove last from n\n").unwrap_err();
        assert!(err.contains("needs a list"), "{err}");
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

    #[test]
    fn arguments_and_files_work_at_the_prompt() {
        let path = std::env::temp_dir().join(format!("rusk-repl-io-{}.txt", std::process::id()));
        // Rusk string literals treat `\` as an escape, so show the path with
        // forward slashes; Windows accepts them too.
        let path_text = path.display().to_string().replace('\\', "/");
        let mut session = Session::new();
        session.args = vec![b"alpha".to_vec(), b"beta".to_vec()];

        session.submit("let first be argument 1\n").unwrap();
        session
            .submit("let count be number of arguments\n")
            .unwrap();
        session
            .submit(&format!(
                "write \"from the prompt\" to file \"{}\"\n",
                path_text
            ))
            .unwrap();
        session
            .submit(&format!("let back be read file \"{}\"\n", path_text))
            .unwrap();

        assert_eq!(session.globals["first"], Value::Str(b"alpha".to_vec()));
        assert_eq!(session.globals["count"], Value::Int(2));
        assert_eq!(
            session.globals["back"],
            Value::Str(b"from the prompt".to_vec())
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn function_values_and_closures_run_at_the_prompt() {
        let mut session = Session::new();
        session
            .submit("to double with n as a number:\n    give back n * 2\n")
            .unwrap();
        session.submit("let f be the function double\n").unwrap();
        session.submit("let out be call f with 21\n").unwrap();
        assert_eq!(session.globals["out"], Value::Int(42));

        session.submit("let x be 10\n").unwrap();
        session
            .submit("let addx be a function with n as a number:\n    give back n + x\n")
            .unwrap();
        session.submit("let sum be call addx with 5\n").unwrap();
        assert_eq!(session.globals["sum"], Value::Int(15));
        session.submit("let by_name be addx with 1\n").unwrap();
        assert_eq!(session.globals["by_name"], Value::Int(11));
    }

    #[test]
    fn a_view_reads_the_same_value_at_the_prompt() {
        let mut session = Session::new();
        session.submit("let xs be [1, 2]\n").unwrap();
        session.submit("let shared be a view of xs\n").unwrap();
        session.submit("let second be item 2 of shared\n").unwrap();
        assert_eq!(session.globals["second"], Value::Int(2));
    }
}
