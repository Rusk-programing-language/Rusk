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
type Scope = HashMap<String, Option<Ty>>;

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
                scope.insert(p.name.clone(), Some(p.ty));
            }
            let mut fresh: Option<Ty> = None;
            infer_stmts(body, &mut scope, &sigs, &mut fresh, true)?;
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
                scope.insert(name.clone(), t);
            }
            Stmt::Set { name, value, line } => {
                let old = scope
                    .get(name)
                    .copied()
                    .ok_or_else(|| at(*line, format!("I don't know a thing called '{name}'")))?;
                let t = type_of(value, scope, sigs).map_err(|m| at(*line, m))?;
                if let (Some(a), Some(b)) = (old, t) {
                    if a != b {
                        return Err(at(
                            *line,
                            format!("'{name}' holds {a}, but that's {b} — Rusk won't change a thing's kind"),
                        ));
                    }
                }
                if t.is_some() {
                    scope.insert(name.clone(), t);
                }
            }
            Stmt::Expr(e, line) => {
                type_of(e, scope, sigs).map_err(|m| at(*line, m))?;
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
                infer_stmts(then, scope, sigs, out, allow_return)?;
                infer_stmts(other, scope, sigs, out, allow_return)?;
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
                infer_stmts(body, scope, sigs, out, allow_return)?;
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
                infer_stmts(body, scope, sigs, out, allow_return)?;
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
                let previous = scope.insert(name.clone(), Some(Ty::Int));
                infer_stmts(body, scope, sigs, out, allow_return)?;
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
            Stmt::FuncDef { line, .. } => {
                return Err(at(
                    *line,
                    "functions have to live at the top level, not inside a block".to_string(),
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
                return Ok(*t);
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
}
