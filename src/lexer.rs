//! Turns `.rk` source into tokens.
//!
//! Rusk is English-shaped, so keywords are *not* reserved at the lexer level —
//! `say`, `let`, `is` and friends are plain identifiers that the parser matches
//! by text. That keeps the lexer tiny and means you can still write
//! `say say` if you ever want a variable named `say`.
//!
//! Blocks are colon + indentation, like Python, so the lexer emits
//! `Indent` / `Dedent` markers.

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Ident(String),
    Int(i64),
    Str(String),
    Newline,
    Indent,
    Dedent,
    Colon,
    Comma,
    LParen,
    RParen,
    Plus,
    Minus,
    Star,
    Slash,
    Eq,
    EqEq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    Bang,
    Eof,
}

impl Tok {
    /// How this token is described in error messages.
    pub fn describe(&self) -> String {
        match self {
            Tok::Ident(s) => format!("'{s}'"),
            Tok::Int(n) => format!("the number {n}"),
            Tok::Str(_) => "a piece of text".to_string(),
            Tok::Newline => "the end of the line".to_string(),
            Tok::Indent => "an indent".to_string(),
            Tok::Dedent => "a dedent".to_string(),
            Tok::Colon => "':'".to_string(),
            Tok::Comma => "','".to_string(),
            Tok::LParen => "'('".to_string(),
            Tok::RParen => "')'".to_string(),
            Tok::Plus => "'+'".to_string(),
            Tok::Minus => "'-'".to_string(),
            Tok::Star => "'*'".to_string(),
            Tok::Slash => "'/'".to_string(),
            Tok::Eq => "'='".to_string(),
            Tok::EqEq => "'=='".to_string(),
            Tok::NotEq => "'!='".to_string(),
            Tok::Lt => "'<'".to_string(),
            Tok::LtEq => "'<='".to_string(),
            Tok::Gt => "'>'".to_string(),
            Tok::GtEq => "'>='".to_string(),
            Tok::Bang => "'!'".to_string(),
            Tok::Eof => "the end of the file".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub tok: Tok,
    pub line: usize,
}

pub fn lex(src: &str) -> Result<Vec<Token>, String> {
    let chars: Vec<char> = src.chars().collect();
    let n = chars.len();
    let mut toks: Vec<Token> = Vec::new();
    let mut indents: Vec<usize> = vec![0];
    let mut i = 0;
    let mut line = 1;
    let mut at_line_start = true;

    while i < n {
        if at_line_start {
            let mut col = 0;
            while i < n && (chars[i] == ' ' || chars[i] == '\t') {
                col += if chars[i] == '\t' { 4 } else { 1 };
                i += 1;
            }

            // Blank lines and comment-only lines never affect indentation.
            if i >= n || chars[i] == '\n' || chars[i] == '\r' || chars[i] == '#' {
                while i < n && chars[i] != '\n' {
                    i += 1;
                }
                if i < n {
                    i += 1;
                    line += 1;
                }
                continue;
            }

            let cur = *indents.last().unwrap();
            if col > cur {
                indents.push(col);
                toks.push(Token { tok: Tok::Indent, line });
            } else if col < cur {
                while col < *indents.last().unwrap() {
                    indents.pop();
                    toks.push(Token { tok: Tok::Dedent, line });
                }
                if col != *indents.last().unwrap() {
                    return Err(format!(
                        "line {line}: this line is indented {} spaces, but no open block starts there",
                        col
                    ));
                }
            }
            at_line_start = false;
            continue;
        }

        let c = chars[i];
        match c {
            ' ' | '\t' | '\r' => i += 1,
            '\n' => {
                toks.push(Token { tok: Tok::Newline, line });
                line += 1;
                i += 1;
                at_line_start = true;
            }
            '#' => {
                while i < n && chars[i] != '\n' {
                    i += 1;
                }
            }
            '"' => {
                let start_line = line;
                i += 1;
                let mut s = String::new();
                let mut closed = false;
                while i < n {
                    let ch = chars[i];
                    if ch == '"' {
                        closed = true;
                        i += 1;
                        break;
                    }
                    if ch == '\\' {
                        i += 1;
                        if i >= n {
                            break;
                        }
                        let esc = chars[i];
                        s.push(match esc {
                            'n' => '\n',
                            't' => '\t',
                            'r' => '\r',
                            '0' => '\0',
                            '\\' => '\\',
                            '"' => '"',
                            other => {
                                return Err(format!(
                                    "line {line}: '\\{other}' isn't an escape I know (try \\n, \\t, \\\", or \\\\)"
                                ))
                            }
                        });
                        i += 1;
                        continue;
                    }
                    if ch == '\n' {
                        return Err(format!(
                            "line {start_line}: this text never gets closed — add a closing '\"'"
                        ));
                    }
                    s.push(ch);
                    i += 1;
                }
                if !closed {
                    return Err(format!(
                        "line {start_line}: this text never gets closed — add a closing '\"'"
                    ));
                }
                toks.push(Token { tok: Tok::Str(s), line });
            }
            '0'..='9' => {
                let start = i;
                while i < n && chars[i].is_ascii_digit() {
                    i += 1;
                }
                let digits: String = chars[start..i].iter().collect();
                let value = digits.parse::<i64>().map_err(|_| {
                    format!("line {line}: the number {digits} is too big for Rusk (max 9223372036854775807)")
                })?;
                toks.push(Token { tok: Tok::Int(value), line });
            }
            c if c.is_alphabetic() || c == '_' => {
                let start = i;
                while i < n && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                let word: String = chars[start..i].iter().collect();
                toks.push(Token { tok: Tok::Ident(word), line });
            }
            ':' => {
                toks.push(Token { tok: Tok::Colon, line });
                i += 1;
            }
            ',' => {
                toks.push(Token { tok: Tok::Comma, line });
                i += 1;
            }
            '(' => {
                toks.push(Token { tok: Tok::LParen, line });
                i += 1;
            }
            ')' => {
                toks.push(Token { tok: Tok::RParen, line });
                i += 1;
            }
            '+' => {
                toks.push(Token { tok: Tok::Plus, line });
                i += 1;
            }
            '-' => {
                toks.push(Token { tok: Tok::Minus, line });
                i += 1;
            }
            '*' => {
                toks.push(Token { tok: Tok::Star, line });
                i += 1;
            }
            '/' => {
                toks.push(Token { tok: Tok::Slash, line });
                i += 1;
            }
            '=' => {
                if i + 1 < n && chars[i + 1] == '=' {
                    toks.push(Token { tok: Tok::EqEq, line });
                    i += 2;
                } else {
                    toks.push(Token { tok: Tok::Eq, line });
                    i += 1;
                }
            }
            '!' => {
                if i + 1 < n && chars[i + 1] == '=' {
                    toks.push(Token { tok: Tok::NotEq, line });
                    i += 2;
                } else {
                    toks.push(Token { tok: Tok::Bang, line });
                    i += 1;
                }
            }
            '<' => {
                if i + 1 < n && chars[i + 1] == '=' {
                    toks.push(Token { tok: Tok::LtEq, line });
                    i += 2;
                } else {
                    toks.push(Token { tok: Tok::Lt, line });
                    i += 1;
                }
            }
            '>' => {
                if i + 1 < n && chars[i + 1] == '=' {
                    toks.push(Token { tok: Tok::GtEq, line });
                    i += 2;
                } else {
                    toks.push(Token { tok: Tok::Gt, line });
                    i += 1;
                }
            }
            other => {
                return Err(format!("line {line}: I don't know what '{other}' means"));
            }
        }
    }

    if !matches!(toks.last(), Some(t) if t.tok == Tok::Newline) {
        toks.push(Token { tok: Tok::Newline, line });
    }
    while indents.len() > 1 {
        indents.pop();
        toks.push(Token { tok: Tok::Dedent, line });
    }
    toks.push(Token { tok: Tok::Eof, line });
    Ok(toks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_world() {
        let toks = lex("say \"hi\"\n").unwrap();
        assert_eq!(toks[0].tok, Tok::Ident("say".into()));
        assert_eq!(toks[1].tok, Tok::Str("hi".into()));
        assert_eq!(toks[2].tok, Tok::Newline);
        assert_eq!(toks[3].tok, Tok::Eof);
    }

    #[test]
    fn indentation_becomes_indent_and_dedent() {
        let src = "if x:\n    say \"y\"\nsay \"z\"\n";
        let toks = lex(src).unwrap();
        assert!(toks.iter().any(|t| t.tok == Tok::Indent));
        assert!(toks.iter().any(|t| t.tok == Tok::Dedent));
    }

    #[test]
    fn comments_and_blank_lines_are_ignored() {
        let src = "# a comment\n\nsay \"hi\"  # trailing\n";
        let toks = lex(src).unwrap();
        assert_eq!(toks.iter().filter(|t| t.tok == Tok::Ident("say".into())).count(), 1);
    }

    #[test]
    fn bad_indent_is_an_error() {
        let src = "if x:\n    say \"a\"\n  say \"b\"\n";
        assert!(lex(src).is_err());
    }
}
