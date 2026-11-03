//! Recursive-descent parser for the English surface syntax.
//!
//! Grammar (v0.6):
//!
//! ```text
//! program    := (import_stmt | stmt)*
//! import_stmt:= "import" (TEXT | NAME) NEWLINE
//! stmt       := say_stmt | let_stmt | const_stmt | set_stmt | if_stmt
//!             | repeat_stmt | while_stmt | for_each_stmt | func_def
//!             | give_back | expr_stmt
//! say_stmt   := "say" expr NEWLINE
//! let_stmt   := ("let" | "variable") NAME "be" expr NEWLINE
//! const_stmt := "constant" NAME "be" expr NEWLINE
//! set_stmt   := "set" NAME "to" expr NEWLINE
//!             | "set" "item" expr "of" primary "to" expr NEWLINE
//! if_stmt    := "if" expr ":" block ("otherwise" ("if" expr ":" block | ":" block))?
//! repeat_stmt:= "repeat" expr "times" ":" block
//! while_stmt := "while" expr ":" block
//! for_each   := "for" "each" NAME ("from" expr "to" expr | "in" expr) ":" block
//! func_def   := "to" NAME ("with" param ("and" param)*)? ":" block
//! param      := NAME "as" ("a"|"an")? TYPE
//! give_back  := "give" "back" expr NEWLINE
//! block      := NEWLINE INDENT stmt* DEDENT
//! expr       := sum (comparison_tail)?
//! sum        := term (("+" | "-") term)*
//! term       := unary (("*" | "/") unary)*
//! unary      := ("-" | "not") unary | primary
//! primary    := INT | TEXT | "true" | "false" | NAME ("with" expr ("and" expr)*)?
//!             | "length" "of" primary
//!             | "letter" expr "of" primary
//!             | "part" expr "to" expr "of" primary
//!             | "item" expr "of" primary
//!             | "copy" "of" primary
//!             | "[" (expr ("," expr)*)? "]"
//!             | ("an")? "empty" "list" "of" TYPE
//!             | "(" expr ")"
//! TYPE       := "a number" | "text" | "a truth value"
//!             | "a list of" ("numbers" | "text" | "truth values")
//!
//! A bare NAME is a variable, or a zero-argument call if no such variable
//! exists. `with` always introduces arguments, which keeps calls unambiguous.
//! `length`, `letter`, `part`, `item`, and `empty` are contextual: they only act
//! as words when the sentence around them fits, so they can still be names.
//! ```

use crate::ast::{BinOp, Elem, Expr, Param, Program, Stmt, Ty, UnOp};
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
        if p.is_word("import") {
            stmts.push(p.import_stmt()?);
        } else {
            stmts.push(p.stmt()?);
        }
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

    /// Look `n` tokens ahead, clamped to the end of the stream.
    fn peek_at(&self, n: usize) -> &Tok {
        let at = (self.pos + n).min(self.toks.len() - 1);
        &self.toks[at].tok
    }

    /// Does the token after `letter` begin the index expression?
    ///
    /// This keeps a variable called `letter` usable: `say letter` and
    /// `say letter + 1` still mean the variable, because neither is followed by
    /// something that could be an index.
    fn starts_index(&self) -> bool {
        match self.peek_at(1) {
            Tok::Int(_) | Tok::Str(_) | Tok::LParen | Tok::Minus => true,
            Tok::Ident(w) => !matches!(
                w.as_str(),
                "is" | "and" | "to" | "from" | "of" | "otherwise"
            ),
            _ => false,
        }
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

    fn import_stmt(&mut self) -> Result<Stmt, String> {
        let line = self.line();
        self.bump(); // "import"
        let path = match self.peek().clone() {
            Tok::Str(s) => {
                self.bump();
                s
            }
            Tok::Ident(s) => {
                self.bump();
                s
            }
            other => {
                return Err(format!(
                    "line {}: after 'import' I expected a file name in quotes, but found {}",
                    self.line(),
                    other.describe()
                ))
            }
        };
        self.end_of_line()?;
        Ok(Stmt::Import { path, line })
    }

    fn stmt(&mut self) -> Result<Stmt, String> {
        let line = self.line();

        if self.is_word("import") {
            return Err(format!(
                "line {}: 'import' only works at the top level, not inside a block",
                self.line()
            ));
        }

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

        if self.is_word("let") || self.is_word("variable") {
            let word = if self.is_word("let") { "let" } else { "variable" };
            self.bump();
            let name = self.name(&format!("after '{word}'"))?;
            self.need_word("be", &format!("after '{word} {name}',"))?;
            let value = self.expr()?;
            self.end_of_line()?;
            return Ok(Stmt::Let { name, value, line });
        }

        if self.is_word("constant") {
            self.bump();
            let name = self.name("after 'constant'")?;
            self.need_word("be", &format!("after 'constant {name}',"))?;
            let value = self.expr()?;
            self.end_of_line()?;
            return Ok(Stmt::Const { name, value, line });
        }

        if self.is_word("set") {
            self.bump();
            if self.is_word("item") && self.starts_index() {
                self.bump(); // "item"
                let index = self.expr()?;
                self.need_word("of", "after the item number,")?;
                let array = self.primary()?;
                self.need_word("to", "after the list in 'set item',")?;
                let value = self.expr()?;
                self.end_of_line()?;
                return Ok(Stmt::SetItem {
                    index,
                    array,
                    value,
                    line,
                });
            }
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
            if self.eat_word("in") {
                let array = self.expr()?;
                self.need(Tok::Colon, "after the list in 'for each',")?;
                let body = self.block("this 'for each'")?;
                return Ok(Stmt::ForEachIn {
                    name,
                    array,
                    body,
                    line,
                });
            }
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
                if word == "list" {
                    self.bump();
                    self.need_word("of", "after 'list',")?;
                    let inner = self.type_name("after 'list of',")?;
                    let elem = match inner {
                        Ty::Int => Elem::Int,
                        Ty::Str => Elem::Str,
                        Ty::Bool => Elem::Bool,
                        other => {
                            return Err(format!(
                                "line {}: {after} a list can hold numbers, text, or truth values, not {other}",
                                self.line()
                            ))
                        }
                    };
                    return Ok(Ty::Array(elem));
                }
                if word == "truth" {
                    self.bump();
                    if !self.eat_word("value") {
                        self.eat_word("values");
                    }
                    return Ok(Ty::Bool);
                }
                let ty = match word.as_str() {
                    "number" | "numbers" => Ty::Int,
                    "text" => Ty::Str,
                    "boolean" | "booleans" => Ty::Bool,
                    _ => {
                        return Err(format!(
                            "line {}: {after} I expected 'a number', 'text', 'a truth value', or 'a list of ...', but found '{word}'",
                            self.line()
                        ))
                    }
                };
                self.bump();
                Ok(ty)
            }
            other => Err(format!(
                "line {}: {after} I expected a kind of value ('a number', 'text', 'a truth value', or 'a list of ...'), but found {}",
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
            Tok::LBracket => {
                self.bump();
                let mut items = Vec::new();
                while self.peek() != &Tok::RBracket {
                    items.push(self.expr()?);
                    if self.peek() == &Tok::Comma {
                        self.bump();
                    } else {
                        break;
                    }
                }
                self.need(Tok::RBracket, "to close this '['")?;
                Ok(Expr::Array(items))
            }
            Tok::Ident(word) => {
                if (word == "empty"
                    && matches!(self.peek_at(1), Tok::Ident(s) if s == "list"))
                    || (word == "an"
                        && matches!(self.peek_at(1), Tok::Ident(s) if s == "empty")
                        && matches!(self.peek_at(2), Tok::Ident(s) if s == "list"))
                {
                    self.eat_word("an");
                    self.bump(); // "empty"
                    self.need_word("list", "after 'empty',")?;
                    self.need_word("of", "after 'empty list',")?;
                    let inner = self.type_name("after 'empty list of',")?;
                    let elem = match inner {
                        Ty::Int => Elem::Int,
                        Ty::Str => Elem::Str,
                        Ty::Bool => Elem::Bool,
                        other => {
                            return Err(format!(
                                "line {}: an empty list can hold numbers, text, or truth values, not {other}",
                                self.line()
                            ))
                        }
                    };
                    return Ok(Expr::EmptyArray(elem));
                }
                if word == "length" && matches!(self.peek_at(1), Tok::Ident(s) if s == "of") {
                    self.bump(); // "length"
                    self.bump(); // "of"
                    let inner = self.primary()?;
                    return Ok(Expr::Length(Box::new(inner)));
                }
                if word == "copy" && matches!(self.peek_at(1), Tok::Ident(s) if s == "of") {
                    self.bump(); // "copy"
                    self.bump(); // "of"
                    let inner = self.primary()?;
                    return Ok(Expr::Copy(Box::new(inner)));
                }
                if word == "part" && self.starts_index() {
                    self.bump(); // "part"
                    let from = self.expr()?;
                    self.need_word("to", "after the start of the part,")?;
                    let to = self.expr()?;
                    self.need_word("of", "after the end of the part,")?;
                    let text = self.primary()?;
                    return Ok(Expr::Part {
                        text: Box::new(text),
                        from: Box::new(from),
                        to: Box::new(to),
                    });
                }
                if word == "letter" && self.starts_index() {
                    self.bump(); // "letter"
                    let n = self.expr()?;
                    self.need_word("of", "after the letter number,")?;
                    let text = self.primary()?;
                    return Ok(Expr::Letter(Box::new(n), Box::new(text)));
                }
                if word == "item" && self.starts_index() {
                    self.bump(); // "item"
                    let index = self.expr()?;
                    self.need_word("of", "after the item number,")?;
                    let array = self.primary()?;
                    return Ok(Expr::Item(Box::new(index), Box::new(array)));
                }

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

    #[test]
    fn text_operations_parse() {
        let prog = p("say length of \"hello\"\n");
        assert!(matches!(&prog.stmts[0], Stmt::Say(Expr::Length(_), _)));

        let prog = p("say letter 2 of \"hello\"\n");
        assert!(matches!(&prog.stmts[0], Stmt::Say(Expr::Letter(..), _)));

        let prog = p("say part 2 to 4 of \"hello\"\n");
        assert!(matches!(&prog.stmts[0], Stmt::Say(Expr::Part { .. }, _)));
    }

    #[test]
    fn letter_can_still_be_a_variable_name() {
        let prog = p("say letter\n");
        assert!(matches!(&prog.stmts[0], Stmt::Say(Expr::Var(name), _) if name == "letter"));

        let prog = p("say letter + 1\n");
        assert!(matches!(&prog.stmts[0], Stmt::Say(Expr::Binary(BinOp::Add, ..), _)));
    }

    #[test]
    fn lists_and_items_parse() {
        let prog = p("let xs be [1, 2, 3]\n");
        match &prog.stmts[0] {
            Stmt::Let {
                value: Expr::Array(items),
                ..
            } => assert_eq!(items.len(), 3),
            other => panic!("unexpected: {other:?}"),
        }

        let prog = p("let empty be an empty list of text\n");
        match &prog.stmts[0] {
            Stmt::Let {
                value: Expr::EmptyArray(Elem::Str),
                ..
            } => {}
            other => panic!("unexpected: {other:?}"),
        }

        let prog = p("say item 2 of xs\n");
        assert!(matches!(&prog.stmts[0], Stmt::Say(Expr::Item(..), _)));

        let prog = p("set item 2 of xs to 9\n");
        assert!(matches!(&prog.stmts[0], Stmt::SetItem { .. }));

        let prog = p("for each x in xs:\n    say x\n");
        match &prog.stmts[0] {
            Stmt::ForEachIn { name, body, .. } => {
                assert_eq!(name, "x");
                assert_eq!(body.len(), 1);
            }
            other => panic!("unexpected: {other:?}"),
        }

        let prog = p("to takes with xs as a list of numbers:\n    give back length of xs\n");
        match &prog.stmts[0] {
            Stmt::FuncDef { params, .. } => assert_eq!(params[0].ty, Ty::Array(Elem::Int)),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn constants_and_imports_parse() {
        let prog = p("constant pi be 3\n");
        assert!(matches!(&prog.stmts[0], Stmt::Const { name, .. } if name == "pi"));

        let prog = p("variable x be 1\n");
        assert!(matches!(&prog.stmts[0], Stmt::Let { name, .. } if name == "x"));

        let prog = p("import \"lib/helpers\"\n");
        match &prog.stmts[0] {
            Stmt::Import { path, line } => {
                assert_eq!(path, "lib/helpers");
                assert_eq!(*line, 1);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn empty_can_still_be_a_variable_name() {
        let prog = p("let empty be 0\nsay empty + 1\n");
        assert!(matches!(
            &prog.stmts[1],
            Stmt::Say(Expr::Binary(BinOp::Add, ..), _)
        ));
    }
}
