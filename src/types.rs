//! Type checking, and working out what each function gives back.
//!
//! Return kinds are inferred, so `to double with n as a number:` needs no
//! annotation: the `give back` says it. Recursion is handled by iterating to a
//! fixed point — start with "unknown", narrow each function until nothing
//! changes.
//!
//! This pass runs before codegen, so the backend can trust the program.

use crate::ast::*;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub struct Sig {
    pub params: Vec<Ty>,
    pub ret: Ty,
}

#[derive(Clone)]
struct SigInfer {
    params: Vec<Ty>,
    ret: Option<Ty>,
}

type Sigs = HashMap<String, SigInfer>;

#[derive(Clone, Copy)]
struct Scoped {
    ty: Option<Ty>,
    constant: bool,
}

type Scope = HashMap<String, Scoped>;

pub fn check(program: &Program) -> Result<HashMap<String, Sig>, String> {
    // Collect top-level definitions first, so calls can appear before them.
    let mut defs: Vec<(&String, &Vec<Param>, &Vec<Stmt>, usize)> = Vec::new();
    for stmt in &program.stmts {
        if let Stmt::FuncDef {
            name,
            params,
            body,
            line,
        } = stmt
        {
            if defs.iter().any(|(n, ..)| *n == name) {
                return Err(format!("line {line}: there's already a function called '{name}'"));
            }
            defs.push((name, params, body, *line));
        }
    }

    let mut rets: HashMap<String, Option<Ty>> = defs
        .iter()
        .map(|(name, ..)| ((*name).clone(), None))
        .collect();
    let mut saw_return: HashMap<String, bool> = defs
        .iter()
        .map(|(name, ..)| ((*name).clone(), false))
        .collect();

    // Fixed-point iteration: each pass may learn a return kind from a function
    // we could not type before.
    let mut sigs: Sigs = build_sigs(&defs, &rets);
    for _ in 0..defs.len() + 2 {
        let mut changed = false;
        for (name, params, body, _line) in &defs {
            let mut scope: Scope = HashMap::new();
            for p in params.iter() {
                scope.insert(
                    p.name.clone(),
                    Scoped {
                        ty: Some(p.ty),
                        constant: false,
                    },
                );
            }
            let mut fresh: Option<Ty> = None;
            infer_stmts(body, &mut scope, &sigs, &mut fresh, true, 0)?;
            if fresh.is_some() {
                saw_return.insert((*name).clone(), true);
            }
            if fresh.is_some() && fresh != rets[*name] {
                rets.insert((*name).clone(), fresh);
                changed = true;
            }
        }
        if !changed {
            break;
        }
        sigs = build_sigs(&defs, &rets);
    }

    // Top-level statements, with no functions' returns in play.
    let mut scope: Scope = HashMap::new();
    let mut stray: Option<Ty> = None;
    for stmt in &program.stmts {
        if matches!(stmt, Stmt::FuncDef { .. }) {
            continue;
        }
        infer_stmts(
            std::slice::from_ref(stmt),
            &mut scope,
            &sigs,
            &mut stray,
            false,
            0,
        )?;
    }

    let mut out = HashMap::new();
    for (name, params, _body, line) in &defs {
        let ret = match rets[*name] {
            Some(t) => t,
            None if saw_return[*name] => {
                return Err(format!(
                    "line {line}: I can't work out what '{name}' gives back — its 'give back' depends only on calls I can't type"
                ))
            }
            None => Ty::Nothing,
        };
        out.insert(
            (*name).clone(),
            Sig {
                params: params.iter().map(|p| p.ty).collect(),
                ret,
            },
        );
    }
    Ok(out)
}

fn build_sigs(
    defs: &[(&String, &Vec<Param>, &Vec<Stmt>, usize)],
    rets: &HashMap<String, Option<Ty>>,
) -> Sigs {
    defs.iter()
        .map(|(name, params, ..)| {
            (
                (*name).clone(),
                SigInfer {
                    params: params.iter().map(|p| p.ty).collect(),
                    ret: rets[*name],
                },
            )
        })
        .collect()
}

fn infer_stmts(
    stmts: &[Stmt],
    scope: &mut Scope,
    sigs: &Sigs,
    out: &mut Option<Ty>,
    allow_return: bool,
    loops: usize,
) -> Result<(), String> {
    for stmt in stmts {
        match stmt {
            Stmt::Say(e, line) => {
                let t = type_of(e, scope, sigs).map_err(|m| at(*line, m))?;
                if t == Some(Ty::Nothing) {
                    return Err(at(*line, "that function doesn't give anything back, so there's nothing to say".into()));
                }
            }
            Stmt::Let { name, value, line } => {
                if scope.contains_key(name) {
                    return Err(at(
                        *line,
                        format!("'{name}' already exists — use 'set {name} to ...' to change it"),
                    ));
                }
                let t = type_of(value, scope, sigs).map_err(|m| at(*line, m))?;
                scope.insert(
                    name.clone(),
                    Scoped {
                        ty: t,
                        constant: false,
                    },
                );
            }
            Stmt::Const { name, value, line } => {
                if scope.contains_key(name) {
                    return Err(at(*line, format!("'{name}' already exists")));
                }
                let t = type_of(value, scope, sigs).map_err(|m| at(*line, m))?;
                scope.insert(
                    name.clone(),
                    Scoped {
                        ty: t,
                        constant: true,
                    },
                );
            }
            Stmt::Set { name, value, line } => {
                let old = scope
                    .get(name)
                    .copied()
                    .ok_or_else(|| at(*line, format!("I don't know a thing called '{name}'")))?;
                if old.constant {
                    return Err(at(
                        *line,
                        format!("'{name}' is a constant — it can't be changed with 'set'"),
                    ));
                }
                let t = type_of(value, scope, sigs).map_err(|m| at(*line, m))?;
                if let (Some(a), Some(b)) = (old.ty, t) {
                    if a != b {
                        return Err(at(
                            *line,
                            format!("'{name}' holds {a}, but that's {b} — Rusk won't change a thing's kind"),
                        ));
                    }
                }
                scope.insert(
                    name.clone(),
                    Scoped {
                        ty: t.or(old.ty),
                        constant: false,
                    },
                );
            }
            Stmt::SetItem {
                index,
                array,
                value,
                line,
            } => {
                let it = type_of(index, scope, sigs).map_err(|m| at(*line, m))?;
                if let Some(t) = it {
                    if t != Ty::Int {
                        return Err(at(
                            *line,
                            format!("the item number must be a number, but this is {t}"),
                        ));
                    }
                }
                let at_ = type_of(array, scope, sigs).map_err(|m| at(*line, m))?;
                let elem = match at_ {
                    Some(Ty::Array(elem)) => Some(elem),
                    Some(t) => {
                        return Err(at(
                            *line,
                            format!("'set item' needs a list, but this is {t}"),
                        ))
                    }
                    None => None,
                };
                let vt = type_of(value, scope, sigs).map_err(|m| at(*line, m))?;
                if let (Some(elem), Some(vt)) = (elem, vt) {
                    if vt != elem.ty() {
                        return Err(at(
                            *line,
                            format!(
                                "this list holds {}, but the new item is {vt}",
                                elem.ty()
                            ),
                        ));
                    }
                }
            }
            Stmt::AddTo { value, name, line } => {
                let old = scope
                    .get(name)
                    .copied()
                    .ok_or_else(|| at(*line, format!("I don't know a thing called '{name}'")))?;
                if old.constant {
                    return Err(at(
                        *line,
                        format!("'{name}' is a constant — it can't be grown"),
                    ));
                }
                let elem = match old.ty {
                    Some(Ty::Array(elem)) => Some(elem),
                    Some(t) => {
                        return Err(at(
                            *line,
                            format!("'add' needs a list, but '{name}' holds {t}"),
                        ))
                    }
                    None => None,
                };
                let vt = type_of(value, scope, sigs).map_err(|m| at(*line, m))?;
                if let (Some(elem), Some(vt)) = (elem, vt) {
                    if vt != elem.ty() {
                        return Err(at(
                            *line,
                            format!(
                                "this list holds {}, but the new item is {vt}",
                                elem.ty()
                            ),
                        ));
                    }
                }
            }
            Stmt::Expr(e, line) => {
                type_of(e, scope, sigs).map_err(|m| at(*line, m))?;
            }
            Stmt::Warn(e, line) => {
                let t = type_of(e, scope, sigs).map_err(|m| at(*line, m))?;
                if t == Some(Ty::Nothing) {
                    return Err(at(
                        *line,
                        "that function doesn't give anything back, so there's nothing to warn about"
                            .into(),
                    ));
                }
            }
            Stmt::Stop(e, line) => {
                let t = type_of(e, scope, sigs).map_err(|m| at(*line, m))?;
                if let Some(t) = t {
                    if t != Ty::Int {
                        return Err(at(
                            *line,
                            format!("'stop with' needs an exit code as a number, but this is {t}"),
                        ));
                    }
                }
            }
            Stmt::WriteFile { text, path, line } => {
                for (what, e) in [("text", text), ("path", path)] {
                    let t = type_of(e, scope, sigs).map_err(|m| at(*line, m))?;
                    if let Some(t) = t {
                        if t != Ty::Str {
                            return Err(at(
                                *line,
                                format!("'write ... to file' needs {what} as text, but this is {t}"),
                            ));
                        }
                    }
                }
            }
            Stmt::If {
                cond,
                then,
                other,
                line,
            } => {
                let t = type_of(cond, scope, sigs).map_err(|m| at(*line, m))?;
                if let Some(t) = t {
                    if t != Ty::Bool {
                        return Err(at(
                            *line,
                            format!("'if' needs a condition that is true or false, but this is {t}"),
                        ));
                    }
                }
                infer_stmts(then, scope, sigs, out, allow_return, loops)?;
                infer_stmts(other, scope, sigs, out, allow_return, loops)?;
            }
            Stmt::Repeat { count, body, line } => {
                let t = type_of(count, scope, sigs).map_err(|m| at(*line, m))?;
                if let Some(t) = t {
                    if t != Ty::Int {
                        return Err(at(
                            *line,
                            format!("'repeat' needs a number, but the count is {t}"),
                        ));
                    }
                }
                infer_stmts(body, scope, sigs, out, allow_return, loops + 1)?;
            }
            Stmt::While { cond, body, line } => {
                let t = type_of(cond, scope, sigs).map_err(|m| at(*line, m))?;
                if let Some(t) = t {
                    if t != Ty::Bool {
                        return Err(at(
                            *line,
                            format!(
                                "'while' needs a condition that is true or false, but this is {t}"
                            ),
                        ));
                    }
                }
                infer_stmts(body, scope, sigs, out, allow_return, loops + 1)?;
            }
            Stmt::ForEach {
                name,
                from,
                to,
                body,
                line,
            } => {
                for (what, e) in [("start", from), ("end", to)] {
                    let t = type_of(e, scope, sigs).map_err(|m| at(*line, m))?;
                    if let Some(t) = t {
                        if t != Ty::Int {
                            return Err(at(
                                *line,
                                format!("the {what} of a 'for each' range must be a number, but this is {t}"),
                            ));
                        }
                    }
                }
                let previous = scope.insert(
                    name.clone(),
                    Scoped {
                        ty: Some(Ty::Int),
                        constant: false,
                    },
                );
                infer_stmts(body, scope, sigs, out, allow_return, loops + 1)?;
                match previous {
                    Some(t) => {
                        scope.insert(name.clone(), t);
                    }
                    None => {
                        scope.remove(name);
                    }
                }
            }
            Stmt::ForEachIn {
                name,
                array,
                body,
                line,
            } => {
                let t = type_of(array, scope, sigs).map_err(|m| at(*line, m))?;
                let elem = match t {
                    Some(Ty::Array(elem)) => Some(elem),
                    Some(t) => {
                        return Err(at(
                            *line,
                            format!("'for each ... in' needs a list, but this is {t}"),
                        ))
                    }
                    None => None,
                };
                let previous = scope.insert(
                    name.clone(),
                    Scoped {
                        ty: elem.map(|e| e.ty()),
                        constant: false,
                    },
                );
                infer_stmts(body, scope, sigs, out, allow_return, loops + 1)?;
                match previous {
                    Some(t) => {
                        scope.insert(name.clone(), t);
                    }
                    None => {
                        scope.remove(name);
                    }
                }
            }
            Stmt::GiveBack(e, line) => {
                if !allow_return {
                    return Err(at(
                        *line,
                        "'give back' only works inside a function".to_string(),
                    ));
                }
                let t = type_of(e, scope, sigs).map_err(|m| at(*line, m))?;
                if let Some(t) = t {
                    match out {
                        Some(old) if *old != t => {
                            return Err(at(
                                *line,
                                format!("this function gives back {old} elsewhere, but here it's {t}"),
                            ))
                        }
                        _ => *out = Some(t),
                    }
                }
            }
            Stmt::Break(line) => {
                if loops == 0 {
                    return Err(at(*line, "'break' only works inside a loop".to_string()));
                }
            }
            Stmt::Continue(line) => {
                if loops == 0 {
                    return Err(at(
                        *line,
                        "'continue' only works inside a loop".to_string(),
                    ));
                }
            }
            Stmt::FuncDef { line, .. } => {
                return Err(at(
                    *line,
                    "functions have to live at the top level, not inside a block".to_string(),
                ))
            }
            Stmt::Import { line, .. } => {
                return Err(at(
                    *line,
                    "this import wasn't resolved — imports only work at the top level".to_string(),
                ))
            }
        }
    }
    Ok(())
}

fn type_of(e: &Expr, scope: &Scope, sigs: &Sigs) -> Result<Option<Ty>, String> {
    match e {
        Expr::Int(_) => Ok(Some(Ty::Int)),
        Expr::Str(_) => Ok(Some(Ty::Str)),
        Expr::Bool(_) => Ok(Some(Ty::Bool)),
        Expr::Var(name) => {
            if let Some(t) = scope.get(name) {
                return Ok(t.ty);
            }
            if let Some(sig) = sigs.get(name) {
                if !sig.params.is_empty() {
                    return Err(format!(
                        "'{name}' needs {} thing{} — call it with 'with ...'",
                        sig.params.len(),
                        if sig.params.len() == 1 { "" } else { "s" }
                    ));
                }
                return Ok(sig.ret);
            }
            Err(format!(
                "I don't know a thing called '{name}' — declare it with 'let {name} be ...' first"
            ))
        }
        Expr::Call(name, args) => {
            let Some(sig) = sigs.get(name) else {
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
            for (i, arg) in args.iter().enumerate() {
                let got = type_of(arg, scope, sigs)?;
                if let Some(got) = got {
                    if got != sig.params[i] {
                        return Err(format!(
                            "argument {} of '{name}' should be {}, but this is {got}",
                            i + 1,
                            sig.params[i]
                        ));
                    }
                }
            }
            Ok(sig.ret)
        }
        Expr::Length(inner) => {
            let t = type_of(inner, scope, sigs)?;
            match t {
                None => Ok(None),
                Some(Ty::Str) | Some(Ty::Array(_)) => Ok(Some(Ty::Int)),
                Some(t) => Err(format!(
                    "'length of' counts the letters in text or the items in a list, but this is {t}"
                )),
            }
        }
        Expr::Letter(index, text) => {
            let it = type_of(index, scope, sigs)?;
            let tt = type_of(text, scope, sigs)?;
            if let Some(t) = it {
                if t != Ty::Int {
                    return Err(format!(
                        "the letter number must be a number, but this is {t}"
                    ));
                }
            }
            if let Some(t) = tt {
                if t != Ty::Str {
                    return Err(format!("'letter ... of' needs text, but this is {t}"));
                }
            }
            if it.is_none() || tt.is_none() {
                return Ok(None);
            }
            Ok(Some(Ty::Str))
        }
        Expr::Part { text, from, to } => {
            let tt = type_of(text, scope, sigs)?;
            if let Some(t) = tt {
                if t != Ty::Str {
                    return Err(format!("'part ... of' needs text, but this is {t}"));
                }
            }
            let mut unknown = tt.is_none();
            for (what, e) in [("start", from), ("end", to)] {
                let t = type_of(e, scope, sigs)?;
                if let Some(t) = t {
                    if t != Ty::Int {
                        return Err(format!(
                            "the {what} of a 'part ... of' must be a number, but this is {t}"
                        ));
                    }
                } else {
                    unknown = true;
                }
            }
            if unknown {
                return Ok(None);
            }
            Ok(Some(Ty::Str))
        }
        Expr::Array(items) => {
            if items.is_empty() {
                return Err(
                    "an empty list needs a kind — write 'an empty list of numbers', 'an empty list of text', or 'an empty list of truth values'"
                        .to_string(),
                );
            }
            let mut elem: Option<Elem> = None;
            let mut unknown = false;
            for item in items {
                match type_of(item, scope, sigs)? {
                    None => unknown = true,
                    Some(Ty::Array(_)) => {
                        return Err(
                            "a list can hold numbers, text, or truth values, not other lists"
                                .to_string(),
                        )
                    }
                    Some(t) => {
                        let this = match t {
                            Ty::Int => Elem::Int,
                            Ty::Str => Elem::Str,
                            Ty::Bool => Elem::Bool,
                            _ => unreachable!(),
                        };
                        match elem {
                            Some(old) if old != this => {
                                return Err(format!(
                                    "this list mixes {} and {} — a list holds one kind of thing",
                                    old.ty(),
                                    this.ty()
                                ))
                            }
                            _ => elem = Some(this),
                        }
                    }
                }
            }
            if unknown {
                return Ok(None);
            }
            Ok(elem.map(|e| Ty::Array(e)))
        }
        Expr::EmptyArray(elem) => Ok(Some(Ty::Array(*elem))),
        Expr::Copy(inner) => {
            let t = type_of(inner, scope, sigs)?;
            match t {
                Some(Ty::Nothing) => Err("there's nothing to copy — that function gives nothing back".to_string()),
                other => Ok(other),
            }
        }
        Expr::Argument(index) => {
            let t = type_of(index, scope, sigs)?;
            if let Some(t) = t {
                if t != Ty::Int {
                    return Err(format!(
                        "the argument number must be a number, but this is {t}"
                    ));
                }
            }
            Ok(Some(Ty::Str))
        }
        Expr::ArgCount => Ok(Some(Ty::Int)),
        Expr::Environment(name) => {
            let t = type_of(name, scope, sigs)?;
            if let Some(t) = t {
                if t != Ty::Str {
                    return Err(format!(
                        "'environment' needs a variable name as text, but this is {t}"
                    ));
                }
            }
            Ok(Some(Ty::Str))
        }
        Expr::ReadLine | Expr::ReadAll => Ok(Some(Ty::Str)),
        Expr::ReadFile(path) => {
            let t = type_of(path, scope, sigs)?;
            if let Some(t) = t {
                if t != Ty::Str {
                    return Err(format!(
                        "'read file' needs a path as text, but this is {t}"
                    ));
                }
            }
            Ok(Some(Ty::Str))
        }
        Expr::Item(index, array) => {
            let it = type_of(index, scope, sigs)?;
            if let Some(t) = it {
                if t != Ty::Int {
                    return Err(format!(
                        "the item number must be a number, but this is {t}"
                    ));
                }
            }
            let at = type_of(array, scope, sigs)?;
            match at {
                None => Ok(None),
                Some(Ty::Array(elem)) => Ok(Some(elem.ty())),
                Some(t) => Err(format!("'item ... of' needs a list, but this is {t}")),
            }
        }
        Expr::Unary(op, inner) => {
            let t = type_of(inner, scope, sigs)?;
            match (op, t) {
                (_, None) => Ok(None),
                (UnOp::Neg, Some(Ty::Int)) => Ok(Some(Ty::Int)),
                (UnOp::Neg, Some(t)) => Err(format!("'-' flips a number, but this is {t}")),
                (UnOp::Not, Some(Ty::Bool)) => Ok(Some(Ty::Bool)),
                (UnOp::Not, Some(t)) => {
                    Err(format!("'not' flips something true or false, but this is {t}"))
                }
            }
        }
        Expr::Binary(op, left, right) => {
            let lt = type_of(left, scope, sigs)?;
            let rt = type_of(right, scope, sigs)?;
            let (Some(lt), Some(rt)) = (lt, rt) else {
                return Ok(None);
            };
            if matches!(lt, Ty::Array(_)) || matches!(rt, Ty::Array(_)) {
                return Err(
                    "lists don't compare yet — compare their items one at a time".to_string(),
                );
            }
            match op {
                BinOp::Add => {
                    if lt == Ty::Int && rt == Ty::Int {
                        Ok(Some(Ty::Int))
                    } else if lt == Ty::Str && rt == Ty::Str {
                        Ok(Some(Ty::Str))
                    } else {
                        Err(format!(
                            "'+' adds numbers or joins text, but this has {lt} and {rt}"
                        ))
                    }
                }
                BinOp::Sub | BinOp::Mul | BinOp::Div => {
                    if lt != Ty::Int || rt != Ty::Int {
                        return Err(format!(
                            "'{}' works on numbers, but this has {lt} and {rt}",
                            op_word(*op)
                        ));
                    }
                    Ok(Some(Ty::Int))
                }
                BinOp::And | BinOp::Or => {
                    if lt != Ty::Bool || rt != Ty::Bool {
                        return Err(format!(
                            "'{}' joins truth values, but this has {lt} and {rt}",
                            op_word(*op)
                        ));
                    }
                    Ok(Some(Ty::Bool))
                }
                _ => {
                    if lt != rt {
                        return Err(format!(
                            "'{}' can't compare {lt} with {rt}",
                            op_word(*op)
                        ));
                    }
                    Ok(Some(Ty::Bool))
                }
            }
        }
    }
}

/// Type one expression against already-known variable kinds and signatures.
///
/// The ownership pass runs after `check`, so it knows every name's kind but
/// still needs to ask the same rules about a value. This is a thin public
/// wrapper around the ordinary checker.
pub fn type_of_with(
    e: &Expr,
    vars: &HashMap<String, Ty>,
    sigs: &HashMap<String, Sig>,
) -> Result<Option<Ty>, String> {
    let scope: Scope = vars
        .iter()
        .map(|(name, ty)| {
            (
                name.clone(),
                Scoped {
                    ty: Some(*ty),
                    constant: false,
                },
            )
        })
        .collect();
    let inferred: Sigs = sigs
        .iter()
        .map(|(name, sig)| {
            (
                name.clone(),
                SigInfer {
                    params: sig.params.clone(),
                    ret: Some(sig.ret),
                },
            )
        })
        .collect();
    type_of(e, &scope, &inferred)
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

fn at(line: usize, msg: String) -> String {
    format!("line {line}: {msg}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::lex;
    use crate::parser::parse;

    fn check_src(src: &str) -> Result<HashMap<String, Sig>, String> {
        let prog = parse(lex(src).unwrap())?;
        check(&prog)
    }

    #[test]
    fn infers_a_numeric_return() {
        let sigs = check_src("to double with n as a number:\n    give back n * 2\n").unwrap();
        assert_eq!(sigs["double"].ret, Ty::Int);
        assert_eq!(sigs["double"].params, vec![Ty::Int]);
    }

    #[test]
    fn functions_without_give_back_return_nothing() {
        let sigs = check_src("to greet with who as text:\n    say who\n").unwrap();
        assert_eq!(sigs["greet"].ret, Ty::Nothing);
    }

    #[test]
    fn recursion_is_typed_through_the_base_case() {
        let src = "\
to fact with n as a number:
    if n is at most 1:
        give back 1
    give back n * fact with n - 1
";
        let sigs = check_src(src).unwrap();
        assert_eq!(sigs["fact"].ret, Ty::Int);
    }

    #[test]
    fn arity_is_checked() {
        let src = "\
to add with a as a number and b as a number:
    give back a + b
say add with 1
";
        let err = check_src(src).unwrap_err();
        assert!(err.contains("takes 2 things"), "{err}");
    }

    #[test]
    fn argument_kinds_are_checked() {
        let src = "\
to double with n as a number:
    give back n * 2
say double with \"hi\"
";
        let err = check_src(src).unwrap_err();
        assert!(err.contains("should be a number"), "{err}");
    }

    #[test]
    fn give_back_at_top_level_is_an_error() {
        let err = check_src("give back 1\n").unwrap_err();
        assert!(err.contains("inside a function"), "{err}");
    }

    #[test]
    fn inconsistent_returns_are_an_error() {
        let src = "\
to confused with n as a number:
    if n is more than 0:
        give back 1
    give back \"no\"
";
        let err = check_src(src).unwrap_err();
        assert!(err.contains("gives back"), "{err}");
    }

    #[test]
    fn text_can_be_joined_and_compared() {
        let src = "\
let hello be \"hel\" + \"lo\"
let yes be hello is \"hello\"
say yes
";
        check_src(src).unwrap();
    }

    #[test]
    fn text_cannot_be_added_to_numbers() {
        let err = check_src("say \"a\" + 1\n").unwrap_err();
        assert!(err.contains("adds numbers or joins text"), "{err}");
    }

    #[test]
    fn while_needs_a_truth_value() {
        let err = check_src("while 1:\n    say \"x\"\n").unwrap_err();
        assert!(err.contains("'while' needs a condition"), "{err}");
    }

    #[test]
    fn for_each_needs_numbers_and_scopes_its_name() {
        let src = "\
for each n from 1 to 3:
    say n
say n
";
        let err = check_src(src).unwrap_err();
        assert!(err.contains("don't know a thing called 'n'"), "{err}");

        let err = check_src("for each n from \"a\" to 3:\n    say n\n").unwrap_err();
        assert!(err.contains("start of a 'for each' range"), "{err}");
    }

    #[test]
    fn text_operations_have_the_right_kinds() {
        let src = "\
let n be length of \"hello\"
let c be letter 1 of \"hello\"
let middle be part 2 to 4 of \"hello\"
say c + middle
";
        check_src(src).unwrap();

        let err = check_src("say length of 5\n").unwrap_err();
        assert!(err.contains("counts the letters in text"), "{err}");

        let err = check_src("say letter \"a\" of \"abc\"\n").unwrap_err();
        assert!(err.contains("letter number must be a number"), "{err}");

        let err = check_src("say part 1 to 2 of 5\n").unwrap_err();
        assert!(err.contains("'part ... of' needs text"), "{err}");
    }

    #[test]
    fn a_constant_cannot_be_changed() {
        let err = check_src("constant pi be 3\nset pi to 4\n").unwrap_err();
        assert!(err.contains("constant"), "{err}");
    }

    #[test]
    fn lists_are_homogeneous() {
        check_src("let xs be [1, 2, 3]\n").unwrap();

        let err = check_src("let xs be [1, \"two\"]\n").unwrap_err();
        assert!(err.contains("mixes"), "{err}");

        let err = check_src("let xs be [1, [2]]\n").unwrap_err();
        assert!(err.contains("not other lists"), "{err}");

        let err = check_src("let xs be []\n").unwrap_err();
        assert!(err.contains("empty list needs a kind"), "{err}");

        check_src("let xs be an empty list of truth values\n").unwrap();
    }

    #[test]
    fn items_have_the_list_kind() {
        let src = "\
let xs be [\"a\", \"b\"]
let s be item 1 of xs
say s + \"!\"
";
        check_src(src).unwrap();

        let err = check_src("let xs be [1, 2]\nset item 1 of xs to \"one\"\n").unwrap_err();
        assert!(err.contains("list holds a number"), "{err}");

        let err = check_src("say item 1 of 5\n").unwrap_err();
        assert!(err.contains("needs a list"), "{err}");
    }

    #[test]
    fn for_each_in_scopes_its_name_and_uses_the_item_kind() {
        let src = "\
let xs be [1, 2]
for each x in xs:
    say x * 2
say x
";
        let err = check_src(src).unwrap_err();
        assert!(err.contains("don't know a thing called 'x'"), "{err}");

        let err = check_src("for each x in 5:\n    say x\n").unwrap_err();
        assert!(err.contains("needs a list"), "{err}");
    }

    #[test]
    fn lists_can_be_parameters_and_returns() {
        let src = "\
to first_text with xs as a list of text:
    give back item 1 of xs

say first_text with [\"a\", \"b\"]
";
        check_src(src).unwrap();
    }
}
