//! Ownership checking: one owner for every heap value.
//!
//! Rusk has no null and no pointers, but text and lists do live on a heap. To
//! keep that safe before the program ever runs, every text value and every list
//! has exactly one owner:
//!
//! * `let y be x` **moves** an owned value from `x` to `y`. Using `x` after
//!   that is a compile error.
//! * Passing an owned value to a function, putting it in a list, assigning it
//!   with `set`, and giving it back all move it in the same way.
//! * `copy of x` makes a fresh value and leaves `x` alone.
//! * Numbers and truth values are plain machine words, so they are copied
//!   freely and never move.
//! * A constant is read-only and cannot be moved; copy it instead.
//!
//! The checker is flow-sensitive inside a function. A value moved on every
//! path that can reach a line has been moved; a move in only one arm of an
//! `if` is treated as a move after the `if`, which is conservative but safe.
//! `give back` ends a path, so a move followed by a return does not poison the
//! path that falls through.
//!
//! This is deliberately a *single-owner* system rather than a borrow checker.
//! Since Rusk has no threads and no shared mutable references, two names can
//! never race over one value; adding threads later cannot break that invariant
//! without adding a sharing feature first. The checker also complements the
//! runtime checks for bounds, division by zero, heap exhaustion, and deep
//! recursion.

use crate::ast::*;
use crate::types::Sig;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Alive,
    Moved,
}

#[derive(Clone, Copy)]
struct Slot {
    ty: Ty,
    state: State,
    constant: bool,
}

impl Slot {
    fn owned(ty: Ty) -> bool {
        matches!(ty, Ty::Str | Ty::Array(_))
    }
}

type Scope = HashMap<String, Slot>;

/// Check every move and copy in a parsed program.
pub fn check(program: &Program, sigs: &HashMap<String, Sig>) -> Result<(), String> {
    let mut checker = Checker { sigs };

    // Function bodies have their own names; a move inside one never touches a
    // top-level owner.
    for stmt in &program.stmts {
        if let Stmt::FuncDef {
            params, body, ..
        } = stmt
        {
            let mut scope = Scope::new();
            for p in params {
                scope.insert(
                    p.name.clone(),
                    Slot {
                        ty: p.ty,
                        state: State::Alive,
                        constant: false,
                    },
                );
            }
            checker.block(body, &mut scope)?;
        }
    }

    let mut scope = Scope::new();
    checker.block(&program.stmts, &mut scope)?;
    Ok(())
}

struct Checker<'a> {
    sigs: &'a HashMap<String, Sig>,
}

impl<'a> Checker<'a> {
    /// Analyze a sequence.
    fn block(&mut self, stmts: &[Stmt], scope: &mut Scope) -> Result<Outcome, String> {
        let mut breaks = None;
        let mut continues = None;
        for stmt in stmts {
            let out = self.stmt(stmt, scope)?;
            breaks = join_opt(breaks, out.breaks);
            continues = join_opt(continues, out.continues);
            if !out.falls {
                return Ok(Outcome {
                    falls: false,
                    breaks,
                    continues,
                });
            }
        }
        Ok(Outcome {
            falls: true,
            breaks,
            continues,
        })
    }

    fn stmt(&mut self, stmt: &Stmt, scope: &mut Scope) -> Result<Outcome, String> {
        match stmt {
            Stmt::Say(e, line) => {
                Self::line_at(*line, self.expr(e, scope, Use::Read))?;
            }
            Stmt::Let { name, value, line } => {
                let ty = self.ty_of(value, scope)?;
                Self::line_at(*line, self.expr(value, scope, Use::Move))?;
                scope.insert(
                    name.clone(),
                    Slot {
                        ty: ty.unwrap_or(Ty::Nothing),
                        state: State::Alive,
                        constant: false,
                    },
                );
            }
            Stmt::Const { name, value, line } => {
                let ty = self.ty_of(value, scope)?;
                Self::line_at(*line, self.expr(value, scope, Use::Move))?;
                scope.insert(
                    name.clone(),
                    Slot {
                        ty: ty.unwrap_or(Ty::Nothing),
                        state: State::Alive,
                        constant: true,
                    },
                );
            }
            Stmt::Set { name, value, line } => {
                let Some(old) = scope.get(name).copied() else {
                    return Err(at(
                        *line,
                        format!("I don't know a thing called '{name}'"),
                    ));
                };
                if old.constant {
                    return Err(at(
                        *line,
                        format!("'{name}' is a constant — it can't be changed with 'set'"),
                    ));
                }
                Self::line_at(*line, self.expr(value, scope, Use::Move))?;
                // The old value is released and the name owns the new one.
                scope.get_mut(name).unwrap().state = State::Alive;
            }
            Stmt::SetItem {
                index,
                array,
                value,
                line,
            } => {
                Self::line_at(*line, self.expr(index, scope, Use::Read))?;
                Self::line_at(*line, self.expr(array, scope, Use::Read))?;
                Self::line_at(*line, self.expr(value, scope, Use::Move))?;
            }
            Stmt::AddTo { value, name, line } => {
                Self::line_at(*line, self.expr(value, scope, Use::Move))?;
                Self::line_at(*line, self.expr(&Expr::Var(name.clone()), scope, Use::Read))?;
            }
            Stmt::If {
                cond,
                then,
                other,
                line,
            } => {
                Self::line_at(*line, self.expr(cond, scope, Use::Read))?;
                let mut then_scope = scope.clone();
                let then_out = self.block(then, &mut then_scope)?;
                let mut other_scope = scope.clone();
                let other_out = self.block(other, &mut other_scope)?;
                *scope = merge(
                    scope,
                    &[
                        (then_scope, then_out.falls),
                        (other_scope, other_out.falls),
                    ],
                );
                return Ok(Outcome {
                    falls: then_out.falls || other_out.falls,
                    breaks: join_opt(then_out.breaks, other_out.breaks),
                    continues: join_opt(then_out.continues, other_out.continues),
                });
            }
            Stmt::Repeat { count, body, line } => {
                Self::line_at(*line, self.expr(count, scope, Use::Read))?;
                self.loop_body(body, scope)?;
            }
            Stmt::While { cond, body, line } => {
                Self::line_at(*line, self.expr(cond, scope, Use::Read))?;
                self.loop_body(body, scope)?;
            }
            Stmt::ForEach {
                name,
                from,
                to,
                body,
                line,
            } => {
                Self::line_at(*line, self.expr(from, scope, Use::Read))?;
                Self::line_at(*line, self.expr(to, scope, Use::Read))?;
                let previous = scope.insert(
                    name.clone(),
                    Slot {
                        ty: Ty::Int,
                        state: State::Alive,
                        constant: false,
                    },
                );
                let result = self.loop_body(body, scope);
                restore(scope, name, previous);
                result?;
            }
            Stmt::ForEachIn {
                name,
                array,
                body,
                line,
            } => {
                let item_ty = self.ty_of(array, scope)?;
                Self::line_at(*line, self.expr(array, scope, Use::Read))?;
                let item_ty = match item_ty {
                    Some(Ty::Array(elem)) => elem.ty(),
                    _ => Ty::Nothing,
                };
                let previous = scope.insert(
                    name.clone(),
                    Slot {
                        ty: item_ty,
                        state: State::Alive,
                        constant: false,
                    },
                );
                let result = self.loop_body(body, scope);
                restore(scope, name, previous);
                result?;
            }
            Stmt::FuncDef { .. } => {}
            Stmt::Import { line, .. } => {
                return Err(at(
                    *line,
                    "this import wasn't resolved — imports only work at the top level"
                        .to_string(),
                ));
            }
            Stmt::GiveBack(e, line) => {
                Self::line_at(*line, self.expr(e, scope, Use::Move))?;
                return Ok(Outcome::returns());
            }
            Stmt::Break(_) => {
                return Ok(Outcome {
                    falls: false,
                    breaks: Some(scope.clone()),
                    continues: None,
                });
            }
            Stmt::Continue(_) => {
                return Ok(Outcome {
                    falls: false,
                    breaks: None,
                    continues: Some(scope.clone()),
                });
            }
            Stmt::Expr(e, line) => Self::line_at(*line, self.expr(e, scope, Use::Read))?,
        }
        Ok(Outcome::falls())
    }

    /// Analyze a loop body once and merge every way it can leave into the
    /// enclosing scope. A `break` or `continue` ends one path but its moves
    /// still happened, so those paths have to join the state after the loop.
    fn loop_body(&mut self, body: &[Stmt], scope: &mut Scope) -> Result<(), String> {
        let base = scope.clone();
        let mut body_scope = base.clone();
        let out = self.block(body, &mut body_scope)?;
        let mut arms: Vec<(Scope, bool)> = vec![(base.clone(), true)];
        if out.falls {
            arms.push((body_scope, true));
        }
        if let Some(exit) = out.breaks {
            arms.push((exit, true));
        }
        if let Some(exit) = out.continues {
            arms.push((exit, true));
        }
        *scope = merge(&base, &arms);
        Ok(())
    }
    fn expr(&mut self, e: &Expr, scope: &mut Scope, use_: Use) -> Result<(), String> {
        match e {
            Expr::Int(_) | Expr::Str(_) | Expr::Bool(_) | Expr::EmptyArray(_) => {}
            Expr::Var(name) => {
                let Some(slot) = scope.get_mut(name) else {
                    // A bare name can also be a zero-argument call.
                    if self
                        .sigs
                        .get(name)
                        .is_some_and(|sig| sig.params.is_empty())
                    {
                        return Ok(());
                    }
                    return Err(format!(
                        "I don't know a thing called '{name}' — declare it with 'let {name} be ...' first"
                    ));
                };
                if !Slot::owned(slot.ty) {
                    return Ok(());
                }
                if slot.state == State::Moved {
                    return Err(format!(
                        "'{name}' was moved already — 'copy of {name}' keeps the original"
                    ));
                }
                if use_ == Use::Move {
                    if slot.constant {
                        return Err(format!(
                            "'{name}' is a constant — use 'copy of {name}' to make a copy"
                        ));
                    }
                    slot.state = State::Moved;
                }
            }
            Expr::Copy(inner) => self.expr(inner, scope, Use::Read)?,
            Expr::Call(_name, args) => {
                for arg in args {
                    self.expr(arg, scope, Use::Move)?;
                }
            }
            Expr::Length(inner) | Expr::Unary(_, inner) => {
                self.expr(inner, scope, Use::Read)?;
            }
            Expr::Letter(a, b) | Expr::Item(a, b) => {
                self.expr(a, scope, Use::Read)?;
                self.expr(b, scope, Use::Read)?;
            }
            Expr::Part { text, from, to } => {
                self.expr(text, scope, Use::Read)?;
                self.expr(from, scope, Use::Read)?;
                self.expr(to, scope, Use::Read)?;
            }
            Expr::Array(items) => {
                // The new list takes ownership of its text items.
                for item in items {
                    self.expr(item, scope, Use::Move)?;
                }
            }
            Expr::Binary(_, left, right) => {
                self.expr(left, scope, Use::Read)?;
                self.expr(right, scope, Use::Read)?;
            }
        }
        Ok(())
    }

    fn ty_of(&self, e: &Expr, scope: &Scope) -> Result<Option<Ty>, String> {
        let vars: HashMap<String, Ty> = scope
            .iter()
            .map(|(name, slot)| (name.clone(), slot.ty))
            .collect();
        crate::types::type_of_with(e, &vars, self.sigs)
    }

    fn line_at(line: usize, result: Result<(), String>) -> Result<(), String> {
        result.map_err(|e| at(line, e))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Use {
    Read,
    Move,
}

/// How a statement or block can leave. `breaks` and `continues` hold the state
/// at each such point so a loop can merge the moves that happened there.
struct Outcome {
    falls: bool,
    breaks: Option<Scope>,
    continues: Option<Scope>,
}

impl Outcome {
    fn falls() -> Self {
        Outcome {
            falls: true,
            breaks: None,
            continues: None,
        }
    }

    fn returns() -> Self {
        Outcome {
            falls: false,
            breaks: None,
            continues: None,
        }
    }
}

fn join_opt(a: Option<Scope>, b: Option<Scope>) -> Option<Scope> {
    match (a, b) {
        (None, other) | (other, None) => other,
        (Some(a), Some(b)) => Some(merge(&a, &[(a.clone(), true), (b, true)])),
    }
}

fn restore(scope: &mut Scope, name: &str, previous: Option<Slot>) {
    match previous {
        Some(slot) => {
            scope.insert(name.to_string(), slot);
        }
        None => {
            scope.remove(name);
        }
    }
}

/// Join the states of the arms that can fall through. A name that only exists
/// in one arm is considered moved: after the `if` it is not safe to use.
fn merge(base: &Scope, arms: &[(Scope, bool)]) -> Scope {
    let falling: Vec<&Scope> = arms
        .iter()
        .filter(|(_, falls)| *falls)
        .map(|(scope, _)| scope)
        .collect();
    if falling.is_empty() {
        return base.clone();
    }

    let mut out = base.clone();
    for (name, old) in base {
        let moved = falling.iter().any(|arm| match arm.get(name) {
            Some(slot) => slot.state == State::Moved || slot.ty != old.ty,
            None => true,
        });
        if moved {
            out.get_mut(name).unwrap().state = State::Moved;
        }
    }

    let mut new_names: HashSet<&String> = HashSet::new();
    for arm in &falling {
        for name in arm.keys() {
            if !base.contains_key(name) {
                new_names.insert(name);
            }
        }
    }
    for name in new_names {
        let first = *falling
            .iter()
            .find_map(|arm| arm.get(name))
            .expect("a new name comes from a falling arm");
        let everywhere_alive = falling.len() == arms.len()
            && falling.iter().all(|arm| {
                matches!(
                    arm.get(name),
                    Some(slot) if slot.state == State::Alive && slot.ty == first.ty
                )
            });
        out.insert(
            name.clone(),
            Slot {
                state: if everywhere_alive {
                    State::Alive
                } else {
                    State::Moved
                },
                ..first
            },
        );
    }
    out
}

fn at(line: usize, msg: String) -> String {
    format!("line {line}: {msg}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::lex;
    use crate::parser::parse;

    fn check_src(src: &str) -> Result<(), String> {
        let program = parse(lex(src).unwrap()).unwrap();
        let sigs = crate::types::check(&program)?;
        check(&program, &sigs)
    }

    fn err(src: &str) -> String {
        check_src(src).expect_err("this program should be rejected")
    }

    #[test]
    fn moving_text_then_using_it_is_an_error() {
        let message = err(
            "\
let a be \"hi\"
let b be a
say a
",
        );
        assert!(message.contains("moved already"), "{message}");
    }

    #[test]
    fn copy_of_keeps_the_source_alive() {
        check_src(
            "\
let a be \"hi\"
let b be copy of a
say a
say b
",
        )
        .unwrap();
    }

    #[test]
    fn copy_of_a_list_keeps_the_source_alive() {
        check_src(
            "\
let xs be [\"a\", \"b\"]
let ys be copy of xs
say xs
say ys
",
        )
        .unwrap();
    }

    #[test]
    fn numbers_and_truth_values_copy_freely() {
        check_src(
            "\
let n be 1
let flag be true
let m be n
let other be flag
say n
say flag
",
        )
        .unwrap();
    }

    #[test]
    fn passing_text_to_a_function_moves_it() {
        let message = err(
            "\
to shout with s as text:
    say s

let a be \"hi\"
shout with a
say a
",
        );
        assert!(message.contains("moved already"), "{message}");
    }

    #[test]
    fn putting_text_in_a_list_moves_it() {
        let message = err(
            "\
let a be \"hi\"
let xs be [a]
say a
",
        );
        assert!(message.contains("moved already"), "{message}");
    }

    #[test]
    fn setting_an_item_takes_ownership() {
        let message = err(
            "\
let a be \"hi\"
let xs be [\"x\"]
set item 1 of xs to a
say a
",
        );
        assert!(message.contains("moved already"), "{message}");
    }

    #[test]
    fn a_move_inside_a_function_is_local_to_it() {
        let message = err(
            "\
to consume with s as text:
    let t be s
    say s
",
        );
        assert!(message.contains("moved already"), "{message}");
    }

    #[test]
    fn a_move_in_one_branch_poisons_the_name_after_it() {
        let message = err(
            "\
let a be \"hi\"
if true:
    let b be a
say a
",
        );
        assert!(message.contains("moved already"), "{message}");
    }

    #[test]
    fn a_returning_branch_does_not_poison_the_other_path() {
        check_src(
            "\
to pick with s as text and flag as a truth value:
    if flag:
        give back s
    say s
",
        )
        .unwrap();
    }

    #[test]
    fn a_move_inside_a_loop_poisons_the_name_after_it() {
        let message = err(
            "\
let a be \"hi\"
repeat 2 times:
    let b be a
say a
",
        );
        assert!(message.contains("moved already"), "{message}");
    }

    #[test]
    fn a_constant_cannot_be_moved() {
        let message = err(
            "\
constant greeting be \"hi\"
let a be greeting
",
        );
        assert!(message.contains("constant"), "{message}");
    }

    #[test]
    fn a_constant_can_be_copied() {
        check_src(
            "\
constant greeting be \"hi\"
let a be copy of greeting
say greeting
say a
",
        )
        .unwrap();
    }

    #[test]
    fn assigning_a_fresh_value_revives_a_moved_name() {
        check_src(
            "\
let a be \"hi\"
let b be a
set a to \"there\"
say a
",
        )
        .unwrap();
    }

    #[test]
    fn iterating_a_list_does_not_consume_it() {
        check_src(
            "\
let xs be [\"a\", \"b\"]
for each word in xs:
    say word
say xs
",
        )
        .unwrap();
    }

    #[test]
    fn a_zero_argument_call_is_not_a_missing_variable() {
        check_src(
            "\
to answer:
    give back 42

say answer
",
        )
        .unwrap();
    }

    #[test]
    fn a_break_before_a_move_leaves_the_value_alive() {
        check_src(
            "\
let a be \"hi\"
repeat 2 times:
    break
    let b be a
say a
",
        )
        .unwrap();
    }

    #[test]
    fn a_move_before_a_break_poisons_the_name_after_the_loop() {
        let message = err(
            "\
let a be \"hi\"
repeat 2 times:
    let b be a
    break
say a
",
        );
        assert!(message.contains("moved already"), "{message}");
    }

    #[test]
    fn a_move_before_a_continue_poisons_the_name_after_the_loop() {
        let message = err(
            "\
let a be \"hi\"
repeat 2 times:
    let b be a
    continue
say a
",
        );
        assert!(message.contains("moved already"), "{message}");
    }

    #[test]
    fn a_break_inside_one_branch_still_poisons_the_name() {
        let message = err(
            "\
let a be \"hi\"
repeat 2 times:
    if true:
        let b be a
        break
say a
",
        );
        assert!(message.contains("moved already"), "{message}");
    }
}
