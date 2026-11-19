//! `rdb` — the small Rusk debugger.
//!
//! It steps a file one top-level statement at a time. That granularity is the
//! honest one for the tree-walking session: a `say`, a `let`, or an entire
//! function definition is executed, checked, and then reported before the next
//! one starts. Breakpoints are line numbers inside those statements.
//!
//! `:rdb` starts it from the REPL, and `rusk rdb file.rk` starts the same
//! commands from the command line.

use crate::diagnostics::Reporter;
use crate::editor::{count_chars, Editor};
use crate::repl::Session;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[derive(Clone)]
struct Chunk {
    /// One-based line of the first source line in this statement.
    start: usize,
    /// One-based line of the last source line in this statement.
    end: usize,
    text: String,
}

/// The debugger state kept beside a REPL session.
pub struct Debugger {
    source: String,
    path: Option<PathBuf>,
    chunks: Vec<Chunk>,
    next: usize,
    breakpoints: BTreeSet<usize>,
    last_line: usize,
    at_stop: bool,
    finished: bool,
    last_command: String,
}

impl Debugger {
    /// Build a debugger around source already in memory. `path` is only a
    /// label and the base for relative imports.
    pub fn new(source: &str, path: Option<PathBuf>) -> Self {
        let source = source.to_string();
        let chunks = build_chunks(&source);
        Debugger {
            source,
            path,
            chunks,
            next: 0,
            breakpoints: BTreeSet::new(),
            last_line: 0,
            at_stop: false,
            finished: false,
            last_command: String::new(),
        }
    }

    /// Read a file and start at its first statement.
    pub fn from_file(path: &Path) -> Result<Self, String> {
        let source = std::fs::read_to_string(path)
            .map_err(|e| format!("can't read {}: {e}", path.display()))?;
        Ok(Debugger::new(&source, Some(path.to_path_buf())))
    }

    /// The greeting shown when `:rdb` or `rusk rdb` starts.
    pub fn hello(&self) -> Vec<String> {
        let label = self
            .path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "the current session".to_string());
        vec![
            format!("rdb — Rusk debugger, watching {label}"),
            format!(
                "{} top-level statement{} available.",
                self.chunks.len(),
                if self.chunks.len() == 1 { "" } else { "s" }
            ),
            "Type help for commands. step runs one statement; continue runs to a breakpoint or the end.".to_string(),
        ]
    }

    pub fn is_active(&self) -> bool {
        !self.finished || self.next < self.chunks.len()
    }

    /// Handle one debugger command. The boolean says whether rdb should stay
    /// open. Output lines are returned so the REPL can capture them.
    pub(crate) fn command(
        &mut self,
        session: &mut Session,
        line: &str,
    ) -> Result<(Vec<String>, bool), String> {
        let line = line.trim();
        let line = if line.is_empty() {
            if self.last_command.is_empty() {
                return Ok((vec![self.where_line()], true));
            }
            self.last_command.clone()
        } else {
            self.last_command = line.to_string();
            line.to_string()
        };

        let (name, rest) = match line.split_once(char::is_whitespace) {
            Some((name, rest)) => (name, rest.trim()),
            None => (line.as_str(), ""),
        };

        match name {
            "help" | "h" | "?" => Ok((help_text(), true)),
            "run" | "r" | "continue" | "c" => Ok((self.continue_run(session)?, true)),
            "step" | "s" | "next" | "n" => Ok((self.step(session)?, true)),
            "break" | "b" => Ok((vec![self.break_at(rest)?], true)),
            "delete" | "d" => Ok((vec![self.delete_break(rest)?], true)),
            "breaks" | "breakpoints" => Ok((self.show_breaks(), true)),
            "info" => {
                if rest.is_empty() || rest.starts_with("break") {
                    Ok((self.show_breaks(), true))
                } else {
                    Err(format!("rdb doesn't know 'info {rest}' — try help"))
                }
            }
            "list" | "l" => Ok((self.list(rest), true)),
            "where" | "bt" | "backtrace" => Ok((vec![self.where_line()], true)),
            "print" | "p" => Ok((vec![self.print(session, rest)?], true)),
            "vars" => Ok((session.variable_lines(), true)),
            "functions" | "funcs" => Ok((session.function_lines(), true)),
            "load" | "file" => Ok((vec![self.load(rest)?], true)),
            "quit" | "q" | "exit" => Ok((vec!["leaving rdb.".to_string()], false)),
            other => Err(format!(
                "rdb doesn't know '{other}' — try help, step, continue, break N, list, print NAME, or quit"
            )),
        }
    }

    fn base(&self) -> Option<&Path> {
        self.path.as_ref().and_then(|path| path.parent())
    }

    fn continue_run(&mut self, session: &mut Session) -> Result<Vec<String>, String> {
        let mut out = Vec::new();
        loop {
            if self.next >= self.chunks.len() {
                self.finished = true;
                out.push("the program has finished.".to_string());
                break;
            }
            if !self.at_stop && self.breakpoint_here() {
                self.at_stop = true;
                out.extend(self.stop_lines());
                break;
            }
            self.at_stop = false;
            if let Some(done) = self.run_one(session)? {
                out.push(done);
                break;
            }
        }
        Ok(out)
    }

    fn step(&mut self, session: &mut Session) -> Result<Vec<String>, String> {
        self.at_stop = false;
        let mut out = Vec::new();
        if let Some(done) = self.run_one(session)? {
            out.push(done);
        } else {
            out.extend(self.stop_lines());
        }
        Ok(out)
    }

    fn run_one(&mut self, session: &mut Session) -> Result<Option<String>, String> {
        if self.next >= self.chunks.len() {
            self.finished = true;
            return Ok(Some("the program has finished.".to_string()));
        }
        let chunk = self.chunks[self.next].clone();
        session.submit_at(&chunk.text, self.base())?;
        self.last_line = chunk.start;
        self.next += 1;
        if self.next >= self.chunks.len() {
            self.finished = true;
            return Ok(Some(format!(
                "line {} ran; the program has finished.",
                chunk.start
            )));
        }
        Ok(None)
    }

    fn breakpoint_here(&self) -> bool {
        match self.chunks.get(self.next) {
            Some(chunk) => self
                .breakpoints
                .iter()
                .any(|line| *line >= chunk.start && *line <= chunk.end),
            None => false,
        }
    }

    fn stop_lines(&self) -> Vec<String> {
        let line = self.current_line();
        let mut out = vec![format!(
            "stopped at line {}: {}",
            line,
            self.source_line(line).trim()
        )];
        if !self.breakpoints.is_empty() {
            let hits: Vec<String> = self
                .breakpoints
                .iter()
                .filter(|at| {
                    self.chunks
                        .get(self.next)
                        .is_some_and(|chunk| **at >= chunk.start && **at <= chunk.end)
                })
                .map(usize::to_string)
                .collect();
            if !hits.is_empty() {
                out.push(format!("breakpoint at line {}.", hits.join(", ")));
            }
        }
        out.push("next: step, continue, list, where, print NAME, or quit.".to_string());
        out
    }

    fn where_line(&self) -> String {
        let label = self
            .path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "<session>".to_string());
        if self.next >= self.chunks.len() {
            return format!(
                "at the end of {label} (top-level statement {} of {})",
                self.chunks.len(),
                self.chunks.len()
            );
        }
        format!(
            "at {label}: line {} (top-level statement {} of {})",
            self.current_line(),
            self.next + 1,
            self.chunks.len()
        )
    }

    fn break_at(&mut self, rest: &str) -> Result<String, String> {
        let line = if rest.is_empty() {
            let at = self.current_line();
            if at == 0 {
                return Err("there is no current line to break at".to_string());
            }
            at
        } else {
            rest.parse::<usize>()
                .map_err(|_| format!("rdb needs a line number after 'break', not {rest:?}"))?
        };
        let count = self.line_count();
        if line == 0 || line > count {
            return Err(format!(
                "line {line} is outside {count} line{}",
                if count == 1 { "" } else { "s" }
            ));
        }
        self.breakpoints.insert(line);
        Ok(format!("breakpoint set at line {line}."))
    }

    fn delete_break(&mut self, rest: &str) -> Result<String, String> {
        if rest.eq_ignore_ascii_case("all") {
            self.breakpoints.clear();
            return Ok("all breakpoints cleared.".to_string());
        }
        if rest.is_empty() {
            return Err("rdb needs a line number after 'delete', or 'delete all'".to_string());
        }
        let line = rest
            .parse::<usize>()
            .map_err(|_| format!("rdb needs a line number after 'delete', not {rest:?}"))?;
        if self.breakpoints.remove(&line) {
            Ok(format!("breakpoint at line {line} removed."))
        } else {
            Err(format!("there is no breakpoint at line {line}"))
        }
    }

    fn show_breaks(&self) -> Vec<String> {
        if self.breakpoints.is_empty() {
            return vec!["no breakpoints yet — use break N.".to_string()];
        }
        self.breakpoints
            .iter()
            .map(|line| format!("line {:>4}: {}", line, self.source_line(*line).trim()))
            .collect()
    }

    fn list(&self, rest: &str) -> Vec<String> {
        let center = if rest.is_empty() {
            let current = self.current_line();
            if current == 0 {
                1
            } else {
                current
            }
        } else {
            match rest.parse::<usize>() {
                Ok(line) => line,
                Err(_) => {
                    return vec![format!(
                        "rdb needs a line number after 'list', not {rest:?}"
                    )]
                }
            }
        };
        let count = self.line_count();
        if count == 0 {
            return vec!["the file is empty.".to_string()];
        }
        let center = center.clamp(1, count);
        let start = center.saturating_sub(3).max(1);
        let end = (center + 3).min(count);
        let next = self.current_line();
        (start..=end)
            .map(|line| {
                let marker = if line == next {
                    "->"
                } else if self.breakpoints.contains(&line) {
                    "b "
                } else {
                    "  "
                };
                format!("{marker} {:>4}  {}", line, self.source_line(line))
            })
            .collect()
    }

    fn print(&self, session: &Session, rest: &str) -> Result<String, String> {
        let name = rest.trim();
        if name.is_empty() {
            return Err("rdb needs a name after 'print'".to_string());
        }
        match session.value_display(name) {
            Some(value) => Ok(format!("{name} = {value}")),
            None => Err(format!("rdb doesn't know a name called '{name}'")),
        }
    }

    fn load(&mut self, rest: &str) -> Result<String, String> {
        if rest.is_empty() {
            return Err("rdb needs a file after 'load'".to_string());
        }
        let path = PathBuf::from(rest);
        let fresh = Debugger::from_file(&path)?;
        let count = fresh.chunks.len();
        *self = fresh;
        Ok(format!(
            "loaded {} ({} top-level statement{}).",
            path.display(),
            count,
            if count == 1 { "" } else { "s" }
        ))
    }

    fn current_line(&self) -> usize {
        if self.next < self.chunks.len() {
            self.chunks[self.next].start
        } else {
            self.last_line
        }
    }

    fn line_count(&self) -> usize {
        self.source.lines().count()
    }

    fn source_line(&self, line: usize) -> &str {
        if line == 0 {
            return "";
        }
        self.source.lines().nth(line - 1).unwrap_or("")
    }
}

fn help_text() -> Vec<String> {
    [
        "rdb commands",
        "  step, s             run the next top-level statement",
        "  continue, c         run until a breakpoint or the end",
        "  break N, b N        stop before the statement on line N",
        "  break               stop at the current or next statement",
        "  delete N, d N       remove a breakpoint; 'delete all' clears them",
        "  breaks              list breakpoints",
        "  list [N], l [N]     show source around N, or around the current line",
        "  where, bt           show the file, line, and statement number",
        "  print NAME, p NAME  show a session name",
        "  vars, functions     show what the session remembers",
        "  load FILE           debug a different file",
        "  quit, q             leave rdb and return to the Rusk prompt",
        "",
        "rdb works one top-level statement at a time: a whole function definition",
        "runs as one step, while calls inside it are not stepped into yet.",
    ]
    .iter()
    .map(|line| line.to_string())
    .collect()
}

fn starts_indented(line: &str) -> bool {
    line.starts_with(' ') || line.starts_with('\t')
}

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

fn build_chunks(source: &str) -> Vec<Chunk> {
    let lines: Vec<&str> = source.lines().collect();
    let mut chunks = Vec::new();
    let mut at = 0;
    while at < lines.len() {
        let line = lines[at];
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            at += 1;
            continue;
        }
        let start = at;
        let mut end = at + 1;
        if opens_block(line) || starts_indented(line) {
            while end < lines.len() {
                let next = lines[end];
                if next.trim().is_empty() || starts_indented(next) {
                    end += 1;
                } else {
                    break;
                }
            }
        }
        chunks.push(Chunk {
            start: start + 1,
            end,
            text: format!("{}\n", lines[start..end].join("\n")),
        });
        at = end;
    }
    chunks
}

/// The `rusk rdb [file]` command line.
pub fn run_cli(path: Option<PathBuf>) -> Result<(), String> {
    let mut debugger = match path.as_ref() {
        Some(path) => Debugger::from_file(path)?,
        None => Debugger::new("", None),
    };
    let mut session = Session::new();
    let mut editor = Editor::new();
    let reporter = Reporter::new();

    for line in debugger.hello() {
        println!("{line}");
    }
    if debugger.chunks.is_empty() {
        println!("Load a file with: load file.rk");
    }

    while let Some(line) = editor
        .read_line("(rdb) ", "")
        .map_err(|e| format!("can't read the rdb prompt: {e}"))?
    {
        if line.trim().is_empty() {
            continue;
        }
        match debugger.command(&mut session, &line) {
            Ok((output, keep)) => {
                for line in output {
                    println!("{line}");
                }
                if !keep {
                    break;
                }
            }
            Err(e) => {
                let diagnostic = reporter.report(&e, Some(&debugger.source));
                eprintln!("{}", diagnostic.render(true));
            }
        }
    }
    Ok(())
}

/// Exposed for the REPL's `:rdb` message when the editor is active.
pub fn prompt_width(prompt: &str) -> usize {
    count_chars(prompt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_level_statements_become_steps() {
        let source = "say \"one\"\nif true:\n    say \"two\"\nsay \"three\"\n";
        let chunks = build_chunks(source);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].start, 1);
        assert_eq!(chunks[1].start, 2);
        assert_eq!(chunks[1].end, 3);
        assert_eq!(chunks[2].start, 4);
    }

    #[test]
    fn breakpoints_can_be_cleared_and_reported() {
        let mut debugger = Debugger::new("say \"one\"\nsay \"two\"\n", None);
        assert!(debugger.break_at("2").unwrap().contains("line 2"));
        assert!(debugger.break_at("9").is_err());
        assert_eq!(debugger.show_breaks().len(), 1);
        debugger.delete_break("2").unwrap();
        assert!(debugger.show_breaks()[0].contains("no breakpoints"));
    }

    #[test]
    fn list_marks_the_current_line() {
        let debugger = Debugger::new("say \"one\"\nsay \"two\"\n", None);
        let listed = debugger.list("");
        assert!(listed.iter().any(|line| line.contains("->")));
    }

    #[test]
    fn hello_describes_the_step_count() {
        let debugger = Debugger::new("say \"one\"\nsay \"two\"\n", None);
        assert!(debugger
            .hello()
            .iter()
            .any(|line| line.contains("2 top-level")));
    }
}
