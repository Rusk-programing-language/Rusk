//! Numbered explanations for the mistakes Rusk reports.
//!
//! Every error that reaches a person gets a short code (`E01`, `E02`, …), a
//! category, the line it came from when one is known, a plain-English
//! explanation, and a hint for the next step. The code identifies the category:
//! `E01` is always a syntax error, `E02` a type error, and so on.
//!
//! The compiler pipeline still returns `String` errors. This module reads the
//! conventions those strings already use (`line N: …`, `in file.rk: line N: …`)
//! rather than changing every stage, which keeps the diagnostics honest about
//! what the pipeline actually knows.

use crate::tui::{Ansi, Color};
use std::io::{self, IsTerminal, Write};

/// The question shown after an error, exactly as the session asks it.
pub const EDIT_QUESTION: &str = "Do you want to edit this code using the REPL y/n";

/// The broad family a mistake belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Syntax,
    Type,
    Ownership,
    Runtime,
    Import,
    Usage,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Syntax => "syntax error",
            Kind::Type => "type error",
            Kind::Ownership => "ownership error",
            Kind::Runtime => "runtime error",
            Kind::Import => "import error",
            Kind::Usage => "usage error",
        }
    }

    /// The stable number for this category: E01 syntax, E02 type, E03
    /// ownership, E04 runtime, E05 import, E06 usage.
    pub fn code(self) -> &'static str {
        match self {
            Kind::Syntax => "E01",
            Kind::Type => "E02",
            Kind::Ownership => "E03",
            Kind::Runtime => "E04",
            Kind::Import => "E05",
            Kind::Usage => "E06",
        }
    }

    fn color(self) -> Color {
        match self {
            Kind::Syntax => Color::BrightRed,
            Kind::Type => Color::BrightYellow,
            Kind::Ownership => Color::BrightMagenta,
            Kind::Runtime => Color::BrightRed,
            Kind::Import => Color::BrightBlue,
            Kind::Usage => Color::BrightCyan,
        }
    }
}

/// One reported mistake, ready to print.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// The category's stable code, such as `E01` for a syntax error.
    pub code: String,
    pub kind: Kind,
    /// The one-based line in the submitted source or file, when known.
    pub line: Option<usize>,
    /// A file the mistake came from, when the error string named one.
    pub file: Option<String>,
    /// The cleaned-up message from the pipeline.
    pub message: String,
    /// The line of code itself, when the caller supplied the source.
    pub snippet: Option<String>,
}

impl Diagnostic {
    /// Build a diagnostic from a raw pipeline error and, when available, the
    /// source it came from. The code comes from the error's category, so the
    /// same kind of mistake always gets the same number.
    pub fn new(raw: &str, source: Option<&str>) -> Self {
        let line = find_line(raw);
        let file = find_file(raw);
        let message = clean_message(raw);
        let kind = classify(raw);
        let snippet = source
            .and_then(|text| line.and_then(|line| text.lines().nth(line - 1)))
            .map(|text| text.trim_end().to_string())
            .filter(|text| !text.trim().is_empty());
        Diagnostic {
            code: kind.code().to_string(),
            kind,
            line,
            file,
            message,
            snippet,
        }
    }

    /// The explanation shown under the code: a category sentence plus a hint
    /// chosen from the wording Rusk already uses.
    pub fn hint(&self) -> String {
        let lower = self.message.to_ascii_lowercase();
        let specific = if lower.contains("never gets closed") {
            "Text starts and ends with a double quote. Close it on the same line and escape inner quotes with \\\"."
        } else if lower.contains("isn't an escape") {
            "Rusk understands \\n, \\t, \\r, \\0, \\\\ and \\\". Some other escapes need to be written literally."
        } else if lower.contains("moved already") {
            "Text and lists have one owner. Use 'copy of x' when you need to keep the original name alive."
        } else if lower.contains("is a view") || lower.contains("read-only") {
            "A view is shared read-only. Make an owned value with 'copy of x' before changing or moving it."
        } else if lower.contains("is a constant") {
            "Constants are set once. Use 'let' for a name that will change, or 'copy of' when a move is needed."
        } else if lower.contains("too big for rusk") {
            "Rusk numbers are signed 64-bit integers, so the largest value is 9223372036854775807."
        } else if lower.contains("indented") && lower.contains("no open block") {
            "An indent must follow a line that ends with ':'. Keep the spaces in a block consistent."
        } else if lower.contains("empty body") {
            "A block needs at least one indented statement under its ':' line."
        } else if lower.contains("i expected") || lower.contains("i need") {
            "Read the line word by word around this point. The parser names the word it expected."
        } else if lower.contains("already exists") || lower.contains("already a function") {
            "A name can only be declared once in a scope. Use 'set' to change an existing value."
        } else if lower.contains("don't know a thing called") {
            "Declare the name first with 'let name be ...', or check the spelling."
        } else if lower.contains("needs a") && lower.contains("list") {
            "Lists use item numbers starting at 1. Check the value and the item number."
        } else if lower.contains("outside the list") {
            "List positions start at 1 and stop at 'length of the list'."
        } else if lower.contains("division by zero") || lower.contains("divide by zero") {
            "Check the divisor before dividing; Rusk stops rather than inventing a value."
        } else if lower.contains("can't read") || lower.contains("couldn't read") {
            "Check the path and that the file can be read by this user."
        } else if lower.contains("import") || lower.contains("cycle") {
            "Imports resolve relative to the file that writes them. Check the path and avoid a loop of imports."
        } else {
            match self.kind {
                Kind::Syntax => "Rusk could not read this line. Check the words, quotes, colons, and indentation.",
                Kind::Type => "A value has the wrong kind for this operation. Check what each name holds.",
                Kind::Ownership => "Owned values move. Use 'copy of' when both names need the value.",
                Kind::Runtime => "The program stopped while running. Check the values reaching this line.",
                Kind::Import => "The import could not be resolved. Check the path and the names it exports.",
                Kind::Usage => "The command was not understood. Try :help for the commands the session knows.",
            }
        };
        specific.to_string()
    }

    /// A multi-line report. Colour is only added when the caller asks for it,
    /// which keeps tests and redirects plain.
    pub fn render(&self, color: bool) -> String {
        let mut lines = Vec::new();
        if color {
            let mut heading = Ansi::new();
            heading.color(self.kind.color());
            heading.bold();
            heading.text(&self.code);
            heading.reset();
            heading.text("  ");
            heading.color(self.kind.color());
            heading.text(self.kind.label());
            heading.reset();
            if let Some(line) = self.line {
                heading.color(Color::BrightBlack);
                heading.text(&format!("  line {line}"));
                heading.reset();
            }
            if let Some(file) = &self.file {
                heading.color(Color::BrightBlack);
                heading.text(&format!("  in {file}"));
                heading.reset();
            }
            lines.push(heading.as_str().to_string());

            lines.push(format!("  {}", self.message));
            if let Some(snippet) = &self.snippet {
                let mut code = Ansi::new();
                code.color(Color::BrightBlack);
                code.text(&format!("  {} | ", self.line.unwrap_or(0)));
                code.reset();
                code.text(snippet);
                lines.push(code.as_str().to_string());
            }
            let mut hint = Ansi::new();
            hint.color(Color::BrightBlack);
            hint.text("  hint: ");
            hint.reset();
            hint.text(&self.hint());
            lines.push(hint.as_str().to_string());
        } else {
            let mut heading = format!("{}  {}", self.code, self.kind.label());
            if let Some(line) = self.line {
                heading.push_str(&format!("  line {line}"));
            }
            if let Some(file) = &self.file {
                heading.push_str(&format!("  in {file}"));
            }
            lines.push(heading);
            lines.push(format!("  {}", self.message));
            if let Some(snippet) = &self.snippet {
                lines.push(format!("  {} | {snippet}", self.line.unwrap_or(0)));
            }
            lines.push(format!("  hint: {}", self.hint()));
        }
        lines.join("\n")
    }
}

/// Builds diagnostics. The category decides the code, so there is no counter
/// to reset between sessions and a type error is never mistaken for `E01`.
#[derive(Debug, Default)]
pub struct Reporter;

impl Reporter {
    pub fn new() -> Self {
        Reporter
    }

    /// Number the mistake by its category and attach the source line when
    /// possible.
    pub fn report(&self, raw: &str, source: Option<&str>) -> Diagnostic {
        Diagnostic::new(raw, source)
    }
}

/// Print the whole submitted source before the edit question, so a person can
/// see exactly what will be reopened. `line` is kept for the caller's report
/// but does not narrow the code shown here.
pub fn show_full_code(source: &str, line: Option<usize>) {
    let _ = line;
    if source.is_empty() {
        return;
    }
    let width = source.lines().count().to_string().len();
    for (index, text) in source.lines().enumerate() {
        println!("{:>width$} | {text}", index + 1);
    }
}

/// Ask whether the code should be reopened in the REPL. The full source is
/// printed first, followed by the exact question. This is deliberately a plain
/// cooked-mode question: it runs after the full-screen editor has already let
/// go of the terminal, and it is skipped entirely when stdin is a pipe.
pub fn wants_to_edit(source: Option<&str>, line: Option<usize>) -> bool {
    if !io::stdin().is_terminal() {
        return false;
    }
    if let Some(source) = source {
        show_full_code(source, line);
    }
    print!("{EDIT_QUESTION} ");
    let _ = io::stdout().flush();
    let mut answer = String::new();
    if io::stdin().read_line(&mut answer).is_err() {
        return false;
    }
    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// The lines a person gets when the REPL reopens: the entire submitted source,
/// with the line numbers removed by the editor. Showing the full code matches
/// what the question promises, even when the mistake is deep inside a block.
pub fn edit_lines(source: &str) -> Vec<String> {
    source.lines().map(|text| text.to_string()).collect()
}

/// Pull the first `line N` out of a pipeline message.
fn find_line(raw: &str) -> Option<usize> {
    let mut rest = raw;
    while let Some(at) = rest.find("line ") {
        let after = &rest[at + 5..];
        let digits: String = after.chars().take_while(|ch| ch.is_ascii_digit()).collect();
        if !digits.is_empty() {
            if let Ok(line) = digits.parse::<usize>() {
                if line > 0 {
                    return Some(line);
                }
            }
        }
        rest = after;
    }
    None
}

/// Recognize the `in path: …` and `can't read path` forms the import loader and
/// CLI already produce.
fn find_file(raw: &str) -> Option<String> {
    if let Some(rest) = raw.strip_prefix("in ") {
        if let Some((path, _)) = rest.split_once(':') {
            if !path.trim().is_empty() {
                return Some(path.trim().to_string());
            }
        }
    }
    for lead in ["can't read ", "couldn't read "] {
        if let Some(rest) = raw.strip_prefix(lead) {
            if let Some((path, _)) = rest.split_once(':') {
                return Some(path.trim().to_string());
            }
        }
    }
    None
}

/// Remove the location prefixes so the explanation itself is the message.
fn clean_message(raw: &str) -> String {
    let mut text = raw.trim();
    if text.starts_with('\0') {
        return "internal stop".to_string();
    }
    if let Some(rest) = text.strip_prefix("in ") {
        if let Some((_, tail)) = rest.split_once(':') {
            text = tail.trim();
        }
    }
    if let Some(rest) = text.strip_prefix("line ") {
        let digits = rest.chars().take_while(|ch| ch.is_ascii_digit()).count();
        if digits > 0 {
            let tail = rest[digits..].trim_start();
            if let Some(message) = tail.strip_prefix(':') {
                text = message.trim();
            }
        }
    }
    text.to_string()
}

fn classify(raw: &str) -> Kind {
    let text = raw.to_ascii_lowercase();
    if text.contains("i don't know the command")
        || text.contains("i don't know the option")
        || text.contains("try 'rusk help'")
        || text.contains("needs a file")
    {
        return Kind::Usage;
    }
    if text.starts_with("can't read ")
        || text.contains("couldn't read")
        || text.contains("this import")
        || text.contains("import wasn't")
        || text.contains("export wasn't")
        || text.contains("cycle")
        || text.contains("imports only work")
        || text.contains("in toast")
        || text.contains("toast")
    {
        return Kind::Import;
    }
    if text.contains("moved already")
        || text.contains("please borrow")
        || text.contains("shared read-only")
        || text.contains("is a view")
        || text.contains("owned value")
        || text.contains("borrow")
        || text.contains("was moved")
    {
        return Kind::Ownership;
    }
    if text.contains("i expected")
        || text.contains("i need ")
        || text.contains("after '")
        || text.contains("never gets closed")
        || text.contains("isn't an escape")
        || text.contains("too big for rusk")
        || text.contains("indented")
        || text.contains("empty body")
        || text.contains("end of the line")
        || text.contains("this text")
        || text.contains("unexpected")
        || text.contains("unclosed")
    {
        return Kind::Syntax;
    }
    if text.contains("holds")
        || text.contains("needs a")
        || text.contains("must be")
        || text.contains("already exists")
        || text.contains("already a function")
        || text.contains("don't know a thing called")
        || text.contains("can't be changed")
        || text.contains("doesn't give anything back")
        || text.contains("kind")
        || text.contains("parameter")
        || text.contains("argument")
        || text.contains("wrong type")
    {
        return Kind::Type;
    }
    Kind::Runtime
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_edit_question_has_the_required_wording() {
        assert_eq!(
            EDIT_QUESTION,
            "Do you want to edit this code using the REPL y/n"
        );
    }

    #[test]
    fn codes_identify_the_category_and_keep_their_source_line() {
        let source = "let x be 1\nsay \"oops\nsay x\n";
        let reporter = Reporter::new();
        let first = reporter.report("line 2: this text never gets closed", Some(source));
        let second = reporter.report("line 3: I expected the end of the line", Some(source));
        let type_error = reporter.report(
            "line 3: 'x' holds text, but this needs a number",
            Some(source),
        );
        let ownership = reporter.report("line 4: 'text' was moved already", Some(source));
        let runtime = reporter.report("line 5: division by zero", Some(source));
        let import = reporter.report("can't read missing.rk: No such file", Some(source));
        let usage = reporter.report("I don't know the command ':wat' — try :help", Some(source));
        assert_eq!(first.code, "E01");
        assert_eq!(second.code, "E01");
        assert_eq!(type_error.code, "E02");
        assert_eq!(ownership.code, "E03");
        assert_eq!(runtime.code, "E04");
        assert_eq!(import.code, "E05");
        assert_eq!(usage.code, "E06");
        assert_eq!(first.line, Some(2));
        assert_eq!(first.snippet.as_deref(), Some("say \"oops"));
        assert_eq!(first.kind, Kind::Syntax);
        assert!(first.render(false).contains("syntax error"));
        assert!(first.render(false).contains("hint:"));
    }

    #[test]
    fn file_and_line_prefixes_are_understood() {
        let diagnostic = Diagnostic::new(
            "in lib/math.rk: line 12: I don't know a thing called 'x'",
            None,
        );
        assert_eq!(diagnostic.code, "E02");
        assert_eq!(diagnostic.file.as_deref(), Some("lib/math.rk"));
        assert_eq!(diagnostic.line, Some(12));
        assert_eq!(diagnostic.message, "I don't know a thing called 'x'");
        assert_eq!(diagnostic.kind, Kind::Type);
    }

    #[test]
    fn ownership_and_import_errors_are_classified() {
        assert_eq!(
            Diagnostic::new("line 4: 'text' was moved already", None).kind,
            Kind::Ownership
        );
        assert_eq!(
            Diagnostic::new("can't read missing.rk: No such file", None).kind,
            Kind::Import
        );
        assert_eq!(
            Diagnostic::new("I don't know the command ':wat' — try :help", None).kind,
            Kind::Usage
        );
    }

    #[test]
    fn edit_lines_returns_the_whole_source() {
        let source = "let x be 1\nif x is 1:\n    say \"one\"\n    say \"two\"\nsay x\n";
        assert_eq!(
            edit_lines(source),
            source
                .lines()
                .map(|line| line.to_string())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_coloured_report_has_ansi_but_no_control_words() {
        let diagnostic = Diagnostic::new("line 2: this text never gets closed", None);
        let coloured = diagnostic.render(true);
        assert!(coloured.contains("\x1b["));
        assert!(coloured.contains("E01"));
        assert!(coloured.contains("syntax error"));
    }
}
