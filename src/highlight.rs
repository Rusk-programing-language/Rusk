//! Source colouring for the interactive editor.
//!
//! This is deliberately a small scanner rather than a second lexer: it never
//! fails, keeps every byte of the input, and only decides which spans should
//! look like keywords, text, numbers, comments, names, and punctuation. The
//! real lexer still owns the language; this module owns the ink.

use crate::tui::{Ansi, Color};

/// How one span of source should be drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    Plain,
    Keyword,
    Name,
    Text,
    Number,
    Comment,
    Operator,
    Punct,
}

/// One coloured span. The text borrowed from the source is never changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment<'a> {
    pub text: &'a str,
    pub style: Style,
}

/// Split one line into spans. The spans always join back to the exact input.
pub fn segments(line: &str) -> Vec<Segment<'_>> {
    let mut spans = Vec::new();
    let len = line.len();
    let mut at = 0;
    let mut first_word = true;

    while at < len {
        let ch = line[at..]
            .chars()
            .next()
            .expect("a character at the offset");
        let width = ch.len_utf8();

        if ch == '#' {
            spans.push(Segment {
                text: &line[at..],
                style: Style::Comment,
            });
            break;
        }
        if ch == '"' {
            let start = at;
            at += width;
            while at < len {
                let next = line[at..].chars().next().expect("a character");
                at += next.len_utf8();
                if next == '\\' {
                    if at < len {
                        let escaped = line[at..].chars().next().expect("an escaped character");
                        at += escaped.len_utf8();
                    }
                } else if next == '"' {
                    break;
                }
            }
            spans.push(Segment {
                text: &line[start..at],
                style: Style::Text,
            });
            first_word = false;
            continue;
        }
        if ch.is_whitespace() {
            let start = at;
            at += width;
            while at < len {
                let next = line[at..].chars().next().expect("a character");
                if next.is_whitespace() {
                    at += next.len_utf8();
                } else {
                    break;
                }
            }
            spans.push(Segment {
                text: &line[start..at],
                style: Style::Plain,
            });
            first_word = false;
            continue;
        }
        if ch.is_ascii_digit() {
            let start = at;
            at += width;
            while at < len && line.as_bytes()[at].is_ascii_digit() {
                at += 1;
            }
            spans.push(Segment {
                text: &line[start..at],
                style: Style::Number,
            });
            first_word = false;
            continue;
        }
        if ch.is_alphabetic() || ch == '_' {
            let start = at;
            at += width;
            while at < len {
                let next = line[at..].chars().next().expect("a character");
                if next.is_alphanumeric() || next == '_' {
                    at += next.len_utf8();
                } else {
                    break;
                }
            }
            let word = &line[start..at];
            spans.push(Segment {
                text: word,
                style: if is_keyword(word) {
                    Style::Keyword
                } else {
                    Style::Name
                },
            });
            first_word = false;
            continue;
        }
        if ch == ':' && first_word {
            // Commands at the prompt read a little better as keywords.
            let start = at;
            at += width;
            while at < len {
                let next = line[at..].chars().next().expect("a character");
                if next.is_alphanumeric() || next == '_' || next == '-' {
                    at += next.len_utf8();
                } else {
                    break;
                }
            }
            spans.push(Segment {
                text: &line[start..at],
                style: Style::Keyword,
            });
            first_word = false;
            continue;
        }

        let style = if matches!(ch, '+' | '-' | '*' | '/' | '=' | '<' | '>' | '!') {
            Style::Operator
        } else {
            Style::Punct
        };
        spans.push(Segment {
            text: &line[at..at + width],
            style,
        });
        at += width;
        first_word = false;
    }

    spans
}

/// Draw a highlighted line into an existing ANSI frame.
pub fn paint(frame: &mut Ansi, line: &str) {
    for span in segments(line) {
        match span.style {
            Style::Plain => frame.text(span.text),
            Style::Keyword => {
                frame.bold();
                frame.color(Color::BrightMagenta);
                frame.text(span.text);
                frame.reset();
            }
            Style::Name => {
                frame.color(Color::BrightCyan);
                frame.text(span.text);
                frame.reset();
            }
            Style::Text => {
                frame.color(Color::Green);
                frame.text(span.text);
                frame.reset();
            }
            Style::Number => {
                frame.color(Color::BrightYellow);
                frame.text(span.text);
                frame.reset();
            }
            Style::Comment => {
                frame.color(Color::BrightBlack);
                frame.text(span.text);
                frame.reset();
            }
            Style::Operator => {
                frame.color(Color::BrightBlue);
                frame.text(span.text);
                frame.reset();
            }
            Style::Punct => {
                frame.color(Color::BrightWhite);
                frame.text(span.text);
                frame.reset();
            }
        }
    }
}

fn is_keyword(word: &str) -> bool {
    matches!(
        word,
        "add"
            | "all"
            | "an"
            | "and"
            | "arguments"
            | "as"
            | "at"
            | "back"
            | "be"
            | "break"
            | "close"
            | "command"
            | "constant"
            | "continue"
            | "copy"
            | "count"
            | "directory"
            | "each"
            | "empty"
            | "end"
            | "equal"
            | "false"
            | "file"
            | "first"
            | "for"
            | "from"
            | "function"
            | "give"
            | "gives"
            | "giving"
            | "if"
            | "import"
            | "in"
            | "input"
            | "is"
            | "item"
            | "key"
            | "last"
            | "least"
            | "length"
            | "less"
            | "let"
            | "line"
            | "list"
            | "load"
            | "make"
            | "more"
            | "most"
            | "not"
            | "nothing"
            | "of"
            | "off"
            | "on"
            | "or"
            | "otherwise"
            | "print"
            | "raw"
            | "remove"
            | "rename"
            | "repeat"
            | "run"
            | "say"
            | "send"
            | "set"
            | "stop"
            | "terminal"
            | "than"
            | "the"
            | "times"
            | "to"
            | "true"
            | "turn"
            | "value"
            | "values"
            | "variable"
            | "view"
            | "warn"
            | "while"
            | "with"
            | "write"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn joined(line: &str) -> String {
        segments(line).iter().map(|span| span.text).collect()
    }

    #[test]
    fn highlighting_never_changes_the_line() {
        let line = "say \"hi # not a comment\" # real comment";
        assert_eq!(joined(line), line);
        assert_eq!(joined("if x is 1:"), "if x is 1:");
        assert_eq!(joined(""), "");
    }

    #[test]
    fn words_text_numbers_and_comments_get_their_own_style() {
        let spans = segments("say \"hi\" 42 # note");
        assert_eq!(spans[0].style, Style::Keyword);
        assert_eq!(spans[2].style, Style::Text);
        assert_eq!(spans[3].style, Style::Plain);
        assert_eq!(spans[4].style, Style::Number);
        assert_eq!(spans[5].style, Style::Plain);
        assert_eq!(spans[6].style, Style::Comment);
    }

    #[test]
    fn prompt_commands_are_keywords() {
        let spans = segments(":rdb demo.rk");
        assert_eq!(spans[0].style, Style::Keyword);
        assert_eq!(spans[0].text, ":rdb");
    }

    #[test]
    fn painting_adds_ansi_but_keeps_the_visible_text() {
        let mut frame = Ansi::new();
        paint(&mut frame, "let x be 1");
        assert!(frame.as_str().contains("\x1b["));
        assert!(frame.as_str().contains("let"));
        assert!(frame.as_str().contains("1"));
    }
}
