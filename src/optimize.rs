//! Small, dependency-free compiler optimizations.
//!
//! The pipeline checks the program the person wrote, then hands a copy through
//! this pass before machine code is emitted. Folding arithmetic on literal
//! numbers and dropping code that can never run keeps the generated program
//! smaller and faster without changing which programs are accepted or what a
//! correct program prints. Nothing here removes a function definition, because
//! calls elsewhere still need its label.

use crate::ast::{BinOp, Expr, Program, Stmt, UnOp};

/// What the pass managed to simplify. Callers and benchmarks can use these
/// counts instead of guessing from the source text.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    pub folded: usize,
    pub removed: usize,
    pub constant_branches: usize,
}

/// Optimize a checked program. The input is never mutated.
pub fn optimize(program: &Program) -> (Program, Stats) {
    let mut stats = Stats::default();
    let stmts = optimize_stmts(program.stmts.clone(), &mut stats);
    (Program { stmts }, stats)
}

/// Optimize only when asked. Benchmarks and tests compare the two paths; the
/// compiler itself calls [`optimize`] through `codegen::compile_for`.
pub fn maybe_optimize(program: &Program, enabled: bool) -> (Program, Stats) {
    if enabled {
        optimize(program)
    } else {
        (program.clone(), Stats::default())
    }
}

#[derive(Clone)]
enum Const {
    Int(i64),
    Bool(bool),
    Str(String),
}

fn fold_expr(expr: Expr, stats: &mut Stats) -> Expr {
    match expr {
        Expr::Unary(op, inner) => {
            let inner = fold_expr(*inner, stats);
            match (&op, constant(&inner)) {
                (UnOp::Neg, Some(Const::Int(n))) => n.checked_neg().map(Expr::Int),
                (UnOp::Not, Some(Const::Bool(b))) => Some(Expr::Bool(!b)),
                _ => None,
            }
            .inspect(|_| {
                stats.folded += 1;
            })
            .unwrap_or(Expr::Unary(op, Box::new(inner)))
        }
        Expr::Binary(op @ (BinOp::And | BinOp::Or), left, right) => {
            let left = fold_expr(*left, stats);
            // Short-circuit semantics mean the right side is not evaluated on
            // the deciding path, so folding it away is safe even if it reads a
            // file or input.
            let decides = matches!(
                (op, constant(&left)),
                (BinOp::And, Some(Const::Bool(false))) | (BinOp::Or, Some(Const::Bool(true)))
            );
            if decides {
                // Still visit the discarded side so folds nested in it show up
                // in the stats; it cannot affect the value.
                drop(fold_expr(*right, stats));
                stats.folded += 1;
                return left;
            }
            let right = fold_expr(*right, stats);
            Expr::Binary(op, Box::new(left), Box::new(right))
        }
        Expr::Binary(op, left, right) => {
            let left = fold_expr(*left, stats);
            let right = fold_expr(*right, stats);
            if let Some(value) = fold_binary(op, &left, &right) {
                stats.folded += 1;
                value
            } else {
                Expr::Binary(op, Box::new(left), Box::new(right))
            }
        }
        Expr::Length(inner) => {
            let inner = fold_expr(*inner, stats);
            if let Some(Const::Str(text)) = constant(&inner) {
                stats.folded += 1;
                Expr::Int(text.len() as i64)
            } else {
                Expr::Length(Box::new(inner))
            }
        }
        Expr::Copy(inner) => Expr::Copy(Box::new(fold_expr(*inner, stats))),
        Expr::View(inner) => Expr::View(Box::new(fold_expr(*inner, stats))),
        Expr::Array(items) => Expr::Array(fold_list(items, stats)),
        Expr::Call(name, args) => Expr::Call(name, fold_list(args, stats)),
        Expr::CallValue { func, args } => Expr::CallValue {
            func: Box::new(fold_expr(*func, stats)),
            args: fold_list(args, stats),
        },
        Expr::Lambda { params, body } => Expr::Lambda {
            params,
            body: optimize_stmts(body, stats),
        },
        Expr::Letter(a, b) => Expr::Letter(
            Box::new(fold_expr(*a, stats)),
            Box::new(fold_expr(*b, stats)),
        ),
        Expr::Part { text, from, to } => Expr::Part {
            text: Box::new(fold_expr(*text, stats)),
            from: Box::new(fold_expr(*from, stats)),
            to: Box::new(fold_expr(*to, stats)),
        },
        Expr::Item(index, array) => Expr::Item(
            Box::new(fold_expr(*index, stats)),
            Box::new(fold_expr(*array, stats)),
        ),
        Expr::Argument(value) => Expr::Argument(Box::new(fold_expr(*value, stats))),
        Expr::Environment(value) => Expr::Environment(Box::new(fold_expr(*value, stats))),
        Expr::ReadFile(path) => Expr::ReadFile(Box::new(fold_expr(*path, stats))),
        Expr::ListDirectory(path) => Expr::ListDirectory(Box::new(fold_expr(*path, stats))),
        Expr::FileExists(path) => Expr::FileExists(Box::new(fold_expr(*path, stats))),
        Expr::RunCommand(text) => Expr::RunCommand(Box::new(fold_expr(*text, stats))),
        Expr::Connect { host, port } => Expr::Connect {
            host: Box::new(fold_expr(*host, stats)),
            port: Box::new(fold_expr(*port, stats)),
        },
        Expr::Listen(port) => Expr::Listen(Box::new(fold_expr(*port, stats))),
        Expr::Accept(socket) => Expr::Accept(Box::new(fold_expr(*socket, stats))),
        Expr::ReadFrom { socket, line } => Expr::ReadFrom {
            socket: Box::new(fold_expr(*socket, stats)),
            line,
        },
        Expr::GuiWindow {
            title,
            width,
            height,
        } => Expr::GuiWindow {
            title: Box::new(fold_expr(*title, stats)),
            width: Box::new(fold_expr(*width, stats)),
            height: Box::new(fold_expr(*height, stats)),
        },
        Expr::GuiControl {
            window,
            kind,
            text,
            x,
            y,
            width,
            height,
        } => Expr::GuiControl {
            window: Box::new(fold_expr(*window, stats)),
            kind,
            text: Box::new(fold_expr(*text, stats)),
            x: Box::new(fold_expr(*x, stats)),
            y: Box::new(fold_expr(*y, stats)),
            width: Box::new(fold_expr(*width, stats)),
            height: Box::new(fold_expr(*height, stats)),
        },
        Expr::GuiControlText(control) => Expr::GuiControlText(Box::new(fold_expr(*control, stats))),
        Expr::GuiConfirm(text) => Expr::GuiConfirm(Box::new(fold_expr(*text, stats))),
        other => other,
    }
}

fn fold_list(items: Vec<Expr>, stats: &mut Stats) -> Vec<Expr> {
    items
        .into_iter()
        .map(|item| fold_expr(item, stats))
        .collect()
}

fn constant(expr: &Expr) -> Option<Const> {
    match expr {
        Expr::Int(n) => Some(Const::Int(*n)),
        Expr::Bool(b) => Some(Const::Bool(*b)),
        Expr::Str(text) => Some(Const::Str(text.clone())),
        _ => None,
    }
}

fn fold_binary(op: BinOp, left: &Expr, right: &Expr) -> Option<Expr> {
    use BinOp::*;
    match (constant(left)?, constant(right)?) {
        (Const::Int(a), Const::Int(b)) => match op {
            Add => a.checked_add(b).map(Expr::Int),
            Sub => a.checked_sub(b).map(Expr::Int),
            Mul => a.checked_mul(b).map(Expr::Int),
            Div => a.checked_div(b).map(Expr::Int),
            Eq => Some(Expr::Bool(a == b)),
            NotEq => Some(Expr::Bool(a != b)),
            Lt => Some(Expr::Bool(a < b)),
            LtEq => Some(Expr::Bool(a <= b)),
            Gt => Some(Expr::Bool(a > b)),
            GtEq => Some(Expr::Bool(a >= b)),
            _ => None,
        },
        (Const::Bool(a), Const::Bool(b)) => match op {
            Eq => Some(Expr::Bool(a == b)),
            NotEq => Some(Expr::Bool(a != b)),
            And => Some(Expr::Bool(a && b)),
            Or => Some(Expr::Bool(a || b)),
            _ => None,
        },
        (Const::Str(a), Const::Str(b)) => match op {
            Add => Some(Expr::Str(format!("{a}{b}"))),
            Eq => Some(Expr::Bool(a == b)),
            NotEq => Some(Expr::Bool(a != b)),
            _ => None,
        },
        _ => None,
    }
}

fn optimize_stmts(stmts: Vec<Stmt>, stats: &mut Stats) -> Vec<Stmt> {
    let mut out = Vec::with_capacity(stmts.len());
    let mut ended = false;
    for stmt in stmts {
        let stmt = optimize_stmt(stmt, stats);
        match stmt {
            // Definitions are code, not execution: keep every label even in a
            // branch or loop that will never run.
            Stmt::FuncDef { .. } => out.push(stmt),
            Stmt::If {
                cond: Expr::Bool(value),
                then,
                other,
                ..
            } => {
                stats.constant_branches += 1;
                let (taken, skipped) = if value { (then, other) } else { (other, then) };
                stats.removed += skipped.len();
                out.extend(taken);
                out.extend(funcdefs(skipped));
            }
            Stmt::Repeat {
                count: Expr::Int(n),
                body,
                ..
            } if n <= 0 => {
                stats.removed += 1;
                out.extend(funcdefs(body));
            }
            Stmt::While {
                cond: Expr::Bool(false),
                body,
                ..
            } => {
                stats.constant_branches += 1;
                stats.removed += body.len() + 1;
                out.extend(funcdefs(body));
            }
            Stmt::Stop(..) | Stmt::GiveBack(..) | Stmt::Break(..) | Stmt::Continue(..) => {
                ended = true;
                out.push(stmt);
            }
            _ if ended => stats.removed += 1,
            other => out.push(other),
        }
    }
    out
}

/// Recover function definitions from code that will not execute. Without this,
/// a call in live code could point at a label the optimizer dropped.
fn funcdefs(stmts: Vec<Stmt>) -> Vec<Stmt> {
    stmts
        .into_iter()
        .filter(|stmt| matches!(stmt, Stmt::FuncDef { .. }))
        .collect()
}

fn optimize_stmt(stmt: Stmt, stats: &mut Stats) -> Stmt {
    match stmt {
        Stmt::Say(expr, line) => Stmt::Say(fold_expr(expr, stats), line),
        Stmt::Let { name, value, line } => Stmt::Let {
            name,
            value: fold_expr(value, stats),
            line,
        },
        Stmt::Const { name, value, line } => Stmt::Const {
            name,
            value: fold_expr(value, stats),
            line,
        },
        Stmt::Set { name, value, line } => Stmt::Set {
            name,
            value: fold_expr(value, stats),
            line,
        },
        Stmt::SetItem {
            index,
            array,
            value,
            line,
        } => Stmt::SetItem {
            index: fold_expr(index, stats),
            array: fold_expr(array, stats),
            value: fold_expr(value, stats),
            line,
        },
        Stmt::AddTo { value, name, line } => Stmt::AddTo {
            value: fold_expr(value, stats),
            name,
            line,
        },
        Stmt::Remove { index, name, line } => Stmt::Remove {
            index: index.map(|expr| fold_expr(expr, stats)),
            name,
            line,
        },
        Stmt::Warn(expr, line) => Stmt::Warn(fold_expr(expr, stats), line),
        Stmt::Stop(expr, line) => Stmt::Stop(fold_expr(expr, stats), line),
        Stmt::WriteFile { text, path, line } => Stmt::WriteFile {
            text: fold_expr(text, stats),
            path: fold_expr(path, stats),
            line,
        },
        Stmt::MakeDirectory { path, line } => Stmt::MakeDirectory {
            path: fold_expr(path, stats),
            line,
        },
        Stmt::RemoveFile { path, line } => Stmt::RemoveFile {
            path: fold_expr(path, stats),
            line,
        },
        Stmt::RemoveDirectory { path, line } => Stmt::RemoveDirectory {
            path: fold_expr(path, stats),
            line,
        },
        Stmt::RenamePath { from, to, line } => Stmt::RenamePath {
            from: fold_expr(from, stats),
            to: fold_expr(to, stats),
            line,
        },
        Stmt::Send { text, socket, line } => Stmt::Send {
            text: fold_expr(text, stats),
            socket: fold_expr(socket, stats),
            line,
        },
        Stmt::Close { socket, line } => Stmt::Close {
            socket: fold_expr(socket, stats),
            line,
        },
        Stmt::RawTerminal { on, line } => Stmt::RawTerminal { on, line },
        Stmt::GuiShow(expr, line) => Stmt::GuiShow(fold_expr(expr, stats), line),
        Stmt::GuiSetText {
            control,
            text,
            line,
        } => Stmt::GuiSetText {
            control: fold_expr(control, stats),
            text: fold_expr(text, stats),
            line,
        },
        Stmt::GuiMessage(expr, line) => Stmt::GuiMessage(fold_expr(expr, stats), line),
        Stmt::GuiPump(line) => Stmt::GuiPump(line),
        Stmt::GuiClose(line) => Stmt::GuiClose(line),
        Stmt::If {
            cond,
            then,
            other,
            line,
        } => Stmt::If {
            cond: fold_expr(cond, stats),
            then: optimize_stmts(then, stats),
            other: optimize_stmts(other, stats),
            line,
        },
        Stmt::Repeat { count, body, line } => Stmt::Repeat {
            count: fold_expr(count, stats),
            body: optimize_stmts(body, stats),
            line,
        },
        Stmt::While { cond, body, line } => Stmt::While {
            cond: fold_expr(cond, stats),
            body: optimize_stmts(body, stats),
            line,
        },
        Stmt::ForEach {
            name,
            from,
            to,
            body,
            line,
        } => Stmt::ForEach {
            name,
            from: fold_expr(from, stats),
            to: fold_expr(to, stats),
            body: optimize_stmts(body, stats),
            line,
        },
        Stmt::ForEachIn {
            name,
            array,
            body,
            line,
        } => Stmt::ForEachIn {
            name,
            array: fold_expr(array, stats),
            body: optimize_stmts(body, stats),
            line,
        },
        Stmt::Import {
            path,
            alias,
            names,
            line,
        } => Stmt::Import {
            path,
            alias,
            names,
            line,
        },
        Stmt::Export {
            path,
            alias,
            names,
            line,
        } => Stmt::Export {
            path,
            alias,
            names,
            line,
        },
        Stmt::FuncDef {
            name,
            params,
            body,
            line,
        } => Stmt::FuncDef {
            name,
            params,
            body: optimize_stmts(body, stats),
            line,
        },
        Stmt::GiveBack(expr, line) => Stmt::GiveBack(fold_expr(expr, stats), line),
        Stmt::Break(line) => Stmt::Break(line),
        Stmt::Continue(line) => Stmt::Continue(line),
        Stmt::Expr(expr, line) => Stmt::Expr(fold_expr(expr, stats), line),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{lexer, parser};

    fn parse(source: &str) -> Program {
        let tokens = lexer::lex(source).unwrap();
        parser::parse(tokens).unwrap()
    }

    #[test]
    fn folds_literal_arithmetic() {
        let program = parse("say 2 + 3 * 4\n");
        let (optimized, stats) = optimize(&program);
        match &optimized.stmts[0] {
            Stmt::Say(Expr::Int(14), _) => {}
            other => panic!("expected a folded 14, got {other:?}"),
        }
        assert!(stats.folded >= 2);
    }

    #[test]
    fn folds_constant_branches_and_drops_dead_code() {
        let program = parse(
            "if false:\n    say \"no\"\notherwise:\n    say \"yes\"\nstop with 0\nsay \"later\"\n",
        );
        let (optimized, stats) = optimize(&program);
        assert_eq!(optimized.stmts.len(), 2);
        assert!(matches!(&optimized.stmts[0], Stmt::Say(Expr::Str(text), _) if text == "yes"));
        assert!(matches!(optimized.stmts[1], Stmt::Stop(Expr::Int(0), _)));
        assert_eq!(stats.constant_branches, 1);
        assert_eq!(stats.removed, 2);
    }

    #[test]
    fn removes_loops_that_cannot_run() {
        let program = parse("repeat 0 times:\n    say \"never\"\nsay \"once\"\n");
        let (optimized, stats) = optimize(&program);
        assert_eq!(optimized.stmts.len(), 1);
        assert!(matches!(&optimized.stmts[0], Stmt::Say(Expr::Str(text), _) if text == "once"));
        assert_eq!(stats.removed, 1);
    }

    #[test]
    fn keeps_function_labels_from_discarded_code() {
        let program =
            parse("if false:\n    to helper:\n        give back 1\notherwise:\n    say helper\n");
        let (optimized, _) = optimize(&program);
        assert_eq!(
            optimized
                .stmts
                .iter()
                .filter(|stmt| matches!(stmt, Stmt::FuncDef { .. }))
                .count(),
            1
        );
    }
}
