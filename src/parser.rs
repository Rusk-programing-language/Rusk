//! Recursive-descent parser for the English surface syntax.
//!
//! Grammar (v0.3):
//!
//! ```text
//! program    := stmt*
//! stmt       := say_stmt | let_stmt | set_stmt | if_stmt | repeat_stmt
//!             | while_stmt | for_each_stmt | func_def | give_back | expr_stmt
//! say_stmt   := "say" expr NEWLINE
//! let_stmt   := "let" NAME "be" expr NEWLINE
//! set_stmt   := "set" NAME "to" expr NEWLINE
//! if_stmt    := "if" expr ":" block ("otherwise" ("if" expr ":" block | ":" block))?
//! repeat_stmt:= "repeat" expr "times" ":" block
//! while_stmt := "while" expr ":" block
//! for_each   := "for" "each" NAME "from" expr "to" expr ":" block
//! func_def   := "to" NAME ("with" param ("and" param)*)? ":" block
//! param      := NAME "as" ("a"|"an")? TYPE
//! give_back  := "give" "back" expr NEWLINE
//! block      := NEWLINE INDENT stmt* DEDENT
//! expr       := sum (comparison_tail)?
//! sum        := term (("+" | "-") term)*
//! term       := unary (("*" | "/") unary)*
//! unary      := ("-" | "not") unary | primary
//! primary    := INT | TEXT | "true" | "false" | NAME ("with" expr ("and" expr)*)?
//!             | "(" expr ")"
//!
//! A bare NAME is a variable, or a zero-argument call if no such variable
//! exists. `with` always introduces arguments, which keeps calls unambiguous.
//! ```

use crate::ast::{BinOp, Expr, Param, Program, Stmt, Ty, UnOp};
use crate::lexer::{Tok, Token};

pub struct Parser {
    toks: Vec<Token>,
    pos: usize,
}

pub fn parse(toks: Vec<Token>) -> Result<Program, String> {
    let mut p = Parser { toks, pos: 0 };
    let mut stmts = Vec::new();
    loop {
        while p.peek() == &Tok::Newline {
            p.bump();
        }
        if p.peek() == &Tok::Eof {
            break;
        }
        stmts.push(p.stmt()?);
    }
    Ok(Program { stmts })
}

impl Parser {
    fn peek(&self) -> &Tok {
        &self.toks[self.pos].tok
    }

    fn line(&self) -> usize {
        self.toks[self.pos].line
    }

    fn bump(&mut self) -> Tok {
        let t = self.toks[self.pos].tok.clone();
        if self.pos + 1 < self.toks.len() {
            self.pos += 1;
        }
        t
    }

    fn is_word(&self, w: &str) -> bool {
        matches!(self.peek(), Tok::Ident(s) if s == w)
    }

    fn eat_word(&mut self, w: &str) -> bool {
        if self.is_word(w) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn need_word(&mut self, w: &str, after: &str) -> Result<(), String> {
        if self.eat_word(w) {
            Ok(())
        } else {
            Err(format!(
                "line {}: {after} I expected '{w}', but found {}",
                self.line(),
                self.peek().describe()
            ))
        }
    }

    fn need(&mut self, want: Tok, after: &str) -> Result<(), String> {
        if self.peek() == &want {
            self.bump();
            Ok(())
        } else {
            Err(format!(
                "line {}: {after} I expected {}, but found {}",
                self.line(),
                want.describe(),
                self.peek().describe()
            ))
        }
    }

    fn end_of_line(&mut self) -> Result<(), String> {
        match self.peek() {
            Tok::Newline => {
                self.bump();
                Ok(())
            }
            Tok::Eof => Ok(()),
            other => Err(format!(
                "line {}: I expected the end of the line here, but found {}",
                self.line(),
                other.describe()
            )),
        }
    }

    fn stmt(&mut self) -> Result<Stmt, String> {
        let line = self.line();

        if self.is_word("to") {
            self.bump();
            let name = self.name("after 'to'")?;
            let mut params = Vec::new();
            if self.eat_word("with") {
                loop {
                    let pname = self.name("after 'with'")?;
                    self.need_word("as", &format!("after the parameter '{pname}',"))?;
                    // "a number", "an answer" and bare "number" all read fine.
                    if !self.eat_word("a") {
                        self.eat_word("an");
                    }
                    let ty = self.type_name(&format!("for the parameter '{pname}',"))?;
                    params.push(Param { name: pname, ty });
                    if !self.eat_word("and") {
                        break;
                    }
                }
            }
            self.need(Tok::Colon, "after the function header,")?;
            let body = self.block("this 'to'")?;
            return Ok(Stmt::FuncDef {
                name,
                params,
                body,
                line,
            });
        }

        if self.is_word("give") {
            self.bump();
            self.need_word("back", "after 'give',")?;
            let value = self.expr()?;
            self.end_of_line()?;
            return Ok(Stmt::GiveBack(value, line));
        }

        if self.is_word("say") {
            self.bump();
            let value = self.expr()?;
            self.end_of_line()?;
            return Ok(Stmt::Say(value, line));
        }

        if self.is_word("let") {
            self.bump();
            let name = self.name("after 'let'")?;
            self.need_word("be", &format!("after 'let {name}',"))?;
            let value = self.expr()?;
            self.end_of_line()?;
            return Ok(Stmt::Let { name, value, line });
        }

        if self.is_word("set") {
            self.bump();
            let name = self.name("after 'set'")?;
            self.need_word("to", &format!("after 'set {name}',"))?;
            let value = self.expr()?;
            self.end_of_line()?;
            return Ok(Stmt::Set { name, value, line });
        }

        if self.is_word("if") {
            self.bump();
            let cond = self.expr()?;
            self.need(Tok::Colon, "after the condition,")?;
            let then = self.block("this 'if'")?;
            let other = if self.eat_word("otherwise") {
                // `otherwise if ...:` reads like the English it is, and is
                // simply an `if` living in the else branch.
                if self.is_word("if") {
                    vec![self.stmt()?]
                } else {
                    self.need(Tok::Colon, "after 'otherwise',")?;
                    self.block("this 'otherwise'")?
                }
            } else {
                Vec::new()
            };
            return Ok(Stmt::If { cond, then, other, line });
        }

        if self.is_word("repeat") {
            self.bump();
            let count = self.expr()?;
            self.need_word("times", "after the repeat count,")?;
            self.need(Tok::Colon, "after 'times',")?;
            let body = self.block("this 'repeat'")?;
            return Ok(Stmt::Repeat { count, body, line });
        }

        if self.is_word("while") {
            self.bump();
            let cond = self.expr()?;
            self.need(Tok::Colon, "after the 'while' condition,")?;
            let body = self.block("this 'while'")?;
            return Ok(Stmt::While { cond, body, line });
        }

        if self.is_word("for") {
            self.bump();
            self.need_word("each", "after 'for',")?;
            let name = self.name("after 'for each'")?;
            self.need_word("from", &format!("after 'for each {name}',"))?;
            let from = self.expr()?;
            self.need_word("to", "after the start of the range,")?;
            let to = self.expr()?;
            self.need(Tok::Colon, "after the end of the range,")?;
            let body = self.block("this 'for each'")?;
            return Ok(Stmt::ForEach {
                name,
                from,
                to,
                body,
                line,
            });
        }

        let value = self.expr()?;
        self.end_of_line()?;
        Ok(Stmt::Expr(value, line))
    }

    fn type_name(&mut self, after: &str) -> Result<Ty, String> {
        match self.peek().clone() {
            Tok::Ident(word) => {
                let ty = match word.as_str() {
                    "number" => Ty::Int,
                    "text" => Ty::Str,
                    "truth" | "boolean" => Ty::Bool,
                    _ => {
                        return Err(format!(
                            "line {}: {after} I expected 'a number', 'text', or 'a truth value', but found '{word}'",
                            self.line()
                        ))
                    }
                };
                self.bump();
                Ok(ty)
            }
            other => Err(format!(
                "line {}: {after} I expected a kind of value ('a number', 'text', or 'a truth value'), but found {}",
                self.line(),
                other.describe()
            )),
        }
    }

    fn name(&mut self, after: &str) -> Result<String, String> {
        match self.peek().clone() {
            Tok::Ident(s) => {
                self.bump();
                Ok(s)
            }
            other => Err(format!(
                "line {}: {after} I expected a name, but found {}",
                self.line(),
                other.describe()
            )),
        }
    }

    fn block(&mut self, what: &str) -> Result<Vec<Stmt>, String> {
        self.need(Tok::Newline, &format!("after the ':' on {what} line,"))?;
        self.need(Tok::Indent, &format!("to open the body of {what},"))?;
        let mut stmts = Vec::new();
        loop {
            while self.peek() == &Tok::Newline {
                self.bump();
            }
            match self.peek() {
                Tok::Dedent => {
                    self.bump();
                    break;
                }
                Tok::Eof => {
                    return Err(format!(
                        "line {}: {what} never gets its body — indent the lines under it",
                        self.line()
                    ))
                }
                _ => stmts.push(self.stmt()?),
            }
        }
        if stmts.is_empty() {
            return Err(format!("line {}: {what} has an empty body", self.line()));
        }
        Ok(stmts)
    }

    fn expr(&mut self) -> Result<Expr, String> {
        self.comparison()
    }

    fn comparison(&mut self) -> Result<Expr, String> {
        let left = self.sum()?;

        let symbol_op = match self.peek() {
            Tok::EqEq => Some(BinOp::Eq),
            Tok::NotEq => Some(BinOp::NotEq),
            Tok::Lt => Some(BinOp::Lt),
            Tok::LtEq => Some(BinOp::LtEq),
            Tok::Gt => Some(BinOp::Gt),
            Tok::GtEq => Some(BinOp::GtEq),
            _ => None,
        };
        if let Some(op) = symbol_op {
            self.bump();
            let right = self.sum()?;
            return Ok(Expr::Binary(op, Box::new(left), Box::new(right)));
        }

        if self.eat_word("is") {
            let op = if self.eat_word("not") {
                BinOp::NotEq
            } else if self.eat_word("more") {
                self.need_word("than", "after 'is more',")?;
                BinOp::Gt
            } else if self.eat_word("less") {
                self.need_word("than", "after 'is less',")?;
                BinOp::Lt
            } else if self.eat_word("equal") {
                self.need_word("to", "after 'is equal',")?;
                BinOp::Eq
            } else if self.eat_word("at") {
                if self.eat_word("least") {
                    BinOp::GtEq
                } else if self.eat_word("most") {
                    BinOp::LtEq
                } else {
                    return Err(format!(
                        "line {}: after 'is at' I expected 'least' or 'most', but found {}",
                        self.line(),
                        self.peek().describe()
                    ));
                }
            } else {
                // A bare `is` is plain equality: `if name is "Rusk":`.
                BinOp::Eq
            };
            let right = self.sum()?;
            return Ok(Expr::Binary(op, Box::new(left), Box::new(right)));
        }

        Ok(left)
    }

    fn sum(&mut self) -> Result<Expr, String> {
        let mut left = self.term()?;
        loop {
            let op = match self.peek() {
                Tok::Plus => BinOp::Add,
                Tok::Minus => BinOp::Sub,
                _ => break,
            };
            self.bump();
            let right = self.term()?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn term(&mut self) -> Result<Expr, String> {
        let mut left = self.unary()?;
        loop {
            let op = match self.peek() {
                Tok::Star => BinOp::Mul,
                Tok::Slash => BinOp::Div,
                _ => break,
            };
            self.bump();
            let right = self.unary()?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn unary(&mut self) -> Result<Expr, String> {
        if self.peek() == &Tok::Minus {
            self.bump();
            let inner = self.unary()?;
            return Ok(Expr::Unary(UnOp::Neg, Box::new(inner)));
        }
        if self.is_word("not") {
            self.bump();
            let inner = self.unary()?;
            return Ok(Expr::Unary(UnOp::Not, Box::new(inner)));
        }
        self.primary()
    }

    fn primary(&mut self) -> Result<Expr, String> {
        match self.peek().clone() {
            Tok::Int(n) => {
                self.bump();
                Ok(Expr::Int(n))
            }
            Tok::Str(s) => {
                self.bump();
                Ok(Expr::Str(s))
            }
            Tok::LParen => {
                self.bump();
                let e = self.expr()?;
                self.need(Tok::RParen, "to close this '('")?;
                Ok(e)
            }
            Tok::Ident(word) => {
                self.bump();
                match word.as_str() {
                    "true" => Ok(Expr::Bool(true)),
                    "false" => Ok(Expr::Bool(false)),
                    _ if self.eat_word("with") => {
                        let mut args = Vec::new();
                        loop {
                            args.push(self.expr()?);
                            if !self.eat_word("and") {
                                break;
                            }
                        }
                        Ok(Expr::Call(word, args))
                    }
                    _ => Ok(Expr::Var(word)),
                }
            }
            other => Err(format!(
                "line {}: I expected a value here, but found {}",
                self.line(),
                other.describe()
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::lex;

    fn p(src: &str) -> Program {
        parse(lex(src).unwrap()).unwrap()
    }

    #[test]
    fn say_a_number() {
        let prog = p("say 42\n");
        assert_eq!(prog.stmts, vec![Stmt::Say(Expr::Int(42), 1)]);
    }

    #[test]
    fn english_comparison() {
        let prog = p("if x is more than 3:\n    say \"big\"\n");
        match &prog.stmts[0] {
            Stmt::If { cond, .. } => match cond {
                Expr::Binary(BinOp::Gt, _, _) => {}
                other => panic!("expected a 'more than' comparison, got {other:?}"),
            },
            other => panic!("expected an if, got {other:?}"),
        }
    }

    #[test]
    fn precedence_is_multiplication_first() {
        let prog = p("say 1 + 2 * 3\n");
        match &prog.stmts[0] {
            Stmt::Say(Expr::Binary(BinOp::Add, _, right), _) => {
                assert!(matches!(**right, Expr::Binary(BinOp::Mul, _, _)));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn otherwise_attaches_to_if() {
        let prog = p("if x:\n    say \"a\"\notherwise:\n    say \"b\"\n");
        match &prog.stmts[0] {
            Stmt::If { other, .. } => assert_eq!(other.len(), 1),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn otherwise_if_is_a_nested_if() {
        let src = "if x:\n    say \"a\"\notherwise if y:\n    say \"b\"\n";
        let prog = p(src);
        match &prog.stmts[0] {
            Stmt::If { other, .. } => match &other[0] {
                Stmt::If { .. } => {}
                other => panic!("expected a nested if, got {other:?}"),
            },
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn while_parses() {
        let prog = p("while x is less than 3:\n    say x\n");
        match &prog.stmts[0] {
            Stmt::While { body, .. } => assert_eq!(body.len(), 1),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn for_each_parses() {
        let prog = p("for each n from 1 to 3:\n    say n\n");
        match &prog.stmts[0] {
            Stmt::ForEach { name, body, .. } => {
                assert_eq!(name, "n");
                assert_eq!(body.len(), 1);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn bare_is_means_equality() {
        let prog = p("say \"a\" is \"a\"\n");
        assert!(matches!(
            &prog.stmts[0],
            Stmt::Say(Expr::Binary(BinOp::Eq, ..), _)
        ));
    }
}
