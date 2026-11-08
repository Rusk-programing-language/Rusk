//! Expands `import` statements before type checking.
//!
//! A plain `import "file.rk"` is still a textual inclusion in the one global
//! namespace. Two newer forms keep modules apart:
//!
//! ```rusk
//! import "math.rk" as math        # definitions become math.double, math.pi, ...
//! from "math.rk" import double    # pull one name in, without the prefix
//! from "math.rk" import double as twice
//! ```
//!
//! A namespaced module's own plain imports are private to it: they receive the
//! same prefix, so they do not leak into the main program. Paths resolve
//! relative to the file that writes the import, then relative to the current
//! directory, then inside a nearby `ruskmods/` folder. An import may name a
//! file directly (`import "lib/text.rk"`) or leave off the `.rk`
//! (`import "lib/text"`).

use crate::ast::{Expr, Program, Stmt};
use crate::lexer::lex;
use crate::parser::parse;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

struct Loader {
    /// A file and a namespace are loaded together at most once per bake.
    loaded: HashSet<(PathBuf, String)>,
    stack: Vec<PathBuf>,
    /// Namespace prefixes already handed out, so two files with the same stem
    /// do not quietly share one name.
    claimed: HashMap<String, PathBuf>,
}

/// Expand imports in a source string. `base` is the directory relative paths
/// should be resolved from; `None` means the current directory.
pub fn expand(source: &str, base: Option<&Path>) -> Result<Program, String> {
    Loader::new().expand_source(source, base, None, "")
}

/// Read a file and expand every import it reaches. Errors name the file they
/// came from.
pub fn expand_file(path: &Path) -> Result<Program, String> {
    let source =
        std::fs::read_to_string(path).map_err(|e| format!("can't read {}: {e}", path.display()))?;
    let canon = canonical(path);
    let mut loader = Loader::new();
    loader.stack.push(canon);
    let program = loader.expand_source(&source, path.parent(), Some(path), "")?;
    loader.stack.pop();
    Ok(program)
}

impl Loader {
    fn new() -> Self {
        Loader {
            loaded: HashSet::new(),
            stack: Vec::new(),
            claimed: HashMap::new(),
        }
    }

    fn expand_source(
        &mut self,
        source: &str,
        base: Option<&Path>,
        label: Option<&Path>,
        prefix: &str,
    ) -> Result<Program, String> {
        let parsed = parse(lex(source)?).map_err(|e| match label {
            Some(path) => format!("in {}: {e}", path.display()),
            None => e,
        })?;

        enum Item {
            Imported(Vec<Stmt>),
            Own(Stmt),
        }

        let mut items = Vec::new();
        let mut renames: HashMap<String, String> = HashMap::new();

        for stmt in parsed.stmts {
            let Stmt::Import {
                path,
                alias,
                names,
                line,
            } = stmt
            else {
                items.push(Item::Own(stmt));
                continue;
            };

            let resolved = resolve(&path, base).map_err(|e| match label {
                Some(file) => format!("in {}: line {line}: {e}", file.display()),
                None => format!("line {line}: {e}"),
            })?;
            let canon = canonical(&resolved);

            if self.stack.contains(&canon) {
                let mut chain: Vec<String> =
                    self.stack.iter().map(|p| p.display().to_string()).collect();
                chain.push(canon.display().to_string());
                let message = format!("this import makes a cycle: {}", chain.join(" -> "));
                return Err(match label {
                    Some(file) => format!("in {}: line {line}: {message}", file.display()),
                    None => format!("line {line}: {message}"),
                });
            }

            let namespace = if !names.is_empty() {
                let ns = self.namespace_for(&canon, &resolved, prefix)?;
                for (name, alias) in &names {
                    let local = alias.clone().unwrap_or_else(|| name.clone());
                    let target = format!("{ns}{name}");
                    if let Some(old) = renames.insert(local.clone(), target.clone()) {
                        if old != target {
                            let message = format!(
                                "'{local}' is already imported from another module — give one of them an 'as' name"
                            );
                            return Err(match label {
                                Some(file) => {
                                    format!("in {}: line {line}: {message}", file.display())
                                }
                                None => format!("line {line}: {message}"),
                            });
                        }
                    }
                }
                ns
            } else if let Some(alias) = alias {
                let ns = format!("{prefix}{alias}.");
                self.claim(&ns, &canon, label, line)?;
                ns
            } else {
                prefix.to_string()
            };

            let key = (canon.clone(), namespace.clone());
            if self.loaded.contains(&key) {
                continue;
            }

            let imported = std::fs::read_to_string(&resolved)
                .map_err(|e| format!("can't read {}: {e}", resolved.display()))?;
            self.stack.push(canon.clone());
            let expanded =
                self.expand_source(&imported, resolved.parent(), Some(&resolved), &namespace)?;
            self.stack.pop();
            self.loaded.insert(key);
            items.push(Item::Imported(expanded.stmts));
        }

        let mut shadow = HashSet::new();
        for item in &items {
            if let Item::Own(stmt) = item {
                if let Some(name) = top_level_name(stmt) {
                    shadow.insert(name.to_string());
                }
            }
        }

        let mut rewriter = Rewriter {
            prefix,
            renames: &renames,
            shadow: &shadow,
        };
        let mut stmts = Vec::new();
        for item in items {
            match item {
                Item::Imported(mut expanded) => stmts.append(&mut expanded),
                Item::Own(stmt) => {
                    stmts.push(rewriter.stmt(stmt, &mut HashSet::new(), true));
                }
            }
        }
        Ok(Program { stmts })
    }

    fn namespace_for(
        &mut self,
        canon: &Path,
        resolved: &Path,
        prefix: &str,
    ) -> Result<String, String> {
        let stem = module_stem(resolved);
        let mut candidate = format!("{prefix}{stem}.");
        let mut n = 2;
        loop {
            match self.claimed.get(&candidate) {
                Some(old) if old == canon => return Ok(candidate),
                Some(_) => {
                    candidate = format!("{prefix}{stem}_{n}.");
                    n += 1;
                }
                None => {
                    self.claimed.insert(candidate.clone(), canon.to_path_buf());
                    return Ok(candidate);
                }
            }
        }
    }

    fn claim(
        &mut self,
        namespace: &str,
        canon: &Path,
        label: Option<&Path>,
        line: usize,
    ) -> Result<(), String> {
        if let Some(old) = self.claimed.get(namespace) {
            if old != canon {
                let name = namespace.trim_end_matches('.');
                let message = format!(
                    "the module name '{name}' is already used by {}",
                    old.display()
                );
                return Err(match label {
                    Some(file) => format!("in {}: line {line}: {message}", file.display()),
                    None => format!("line {line}: {message}"),
                });
            }
        }
        self.claimed
            .insert(namespace.to_string(), canon.to_path_buf());
        Ok(())
    }
}

struct Rewriter<'a> {
    prefix: &'a str,
    renames: &'a HashMap<String, String>,
    /// Names declared at the top level of this file. A local declaration wins
    /// over a selectively imported name with the same spelling.
    shadow: &'a HashSet<String>,
}

impl Rewriter<'_> {
    fn reference(&self, name: &str, locals: &HashSet<String>) -> String {
        if locals.contains(name) {
            return name.to_string();
        }
        if self.shadow.contains(name) {
            return format!("{}{}", self.prefix, name);
        }
        if let Some(target) = self.renames.get(name) {
            return target.clone();
        }
        format!("{}{}", self.prefix, name)
    }

    fn definition(&self, name: &str) -> String {
        format!("{}{}", self.prefix, name)
    }

    fn block(&mut self, stmts: Vec<Stmt>, locals: &HashSet<String>) -> Vec<Stmt> {
        let mut inner = locals.clone();
        stmts
            .into_iter()
            .map(|stmt| self.stmt(stmt, &mut inner, false))
            .collect()
    }

    fn stmt(&mut self, stmt: Stmt, locals: &mut HashSet<String>, top: bool) -> Stmt {
        match stmt {
            Stmt::Say(e, line) => Stmt::Say(self.expr(e, locals), line),
            Stmt::Let { name, value, line } => {
                let value = self.expr(value, locals);
                if !top {
                    locals.insert(name.clone());
                }
                Stmt::Let {
                    name: if top { self.definition(&name) } else { name },
                    value,
                    line,
                }
            }
            Stmt::Const { name, value, line } => {
                let value = self.expr(value, locals);
                if !top {
                    locals.insert(name.clone());
                }
                Stmt::Const {
                    name: if top { self.definition(&name) } else { name },
                    value,
                    line,
                }
            }
            Stmt::Set { name, value, line } => Stmt::Set {
                name: self.reference(&name, locals),
                value: self.expr(value, locals),
                line,
            },
            Stmt::SetItem {
                index,
                array,
                value,
                line,
            } => Stmt::SetItem {
                index: self.expr(index, locals),
                array: self.expr(array, locals),
                value: self.expr(value, locals),
                line,
            },
            Stmt::AddTo { value, name, line } => Stmt::AddTo {
                value: self.expr(value, locals),
                name: self.reference(&name, locals),
                line,
            },
            Stmt::Remove { index, name, line } => Stmt::Remove {
                index: index.map(|e| self.expr(e, locals)),
                name: self.reference(&name, locals),
                line,
            },
            Stmt::Warn(e, line) => Stmt::Warn(self.expr(e, locals), line),
            Stmt::Stop(e, line) => Stmt::Stop(self.expr(e, locals), line),
            Stmt::WriteFile { text, path, line } => Stmt::WriteFile {
                text: self.expr(text, locals),
                path: self.expr(path, locals),
                line,
            },
            Stmt::If {
                cond,
                then,
                other,
                line,
            } => Stmt::If {
                cond: self.expr(cond, locals),
                then: self.block(then, locals),
                other: self.block(other, locals),
                line,
            },
            Stmt::Repeat { count, body, line } => Stmt::Repeat {
                count: self.expr(count, locals),
                body: self.block(body, locals),
                line,
            },
            Stmt::While { cond, body, line } => Stmt::While {
                cond: self.expr(cond, locals),
                body: self.block(body, locals),
                line,
            },
            Stmt::ForEach {
                name,
                from,
                to,
                body,
                line,
            } => {
                let from = self.expr(from, locals);
                let to = self.expr(to, locals);
                let mut inner = locals.clone();
                inner.insert(name.clone());
                Stmt::ForEach {
                    name,
                    from,
                    to,
                    body: self.block(body, &inner),
                    line,
                }
            }
            Stmt::ForEachIn {
                name,
                array,
                body,
                line,
            } => {
                let array = self.expr(array, locals);
                let mut inner = locals.clone();
                inner.insert(name.clone());
                Stmt::ForEachIn {
                    name,
                    array,
                    body: self.block(body, &inner),
                    line,
                }
            }
            Stmt::FuncDef {
                name,
                params,
                body,
                line,
            } => {
                let mut inner = HashSet::new();
                for param in &params {
                    inner.insert(param.name.clone());
                }
                Stmt::FuncDef {
                    name: self.definition(&name),
                    params,
                    body: self.block(body, &inner),
                    line,
                }
            }
            Stmt::GiveBack(e, line) => Stmt::GiveBack(self.expr(e, locals), line),
            Stmt::Break(line) => Stmt::Break(line),
            Stmt::Continue(line) => Stmt::Continue(line),
            Stmt::Expr(e, line) => Stmt::Expr(self.expr(e, locals), line),
            // Resolved imports never reach here; leave one alone so the normal
            // "import wasn't resolved" check can report it.
            Stmt::Import { .. } => stmt,
        }
    }

    fn expr(&mut self, expr: Expr, locals: &HashSet<String>) -> Expr {
        match expr {
            Expr::Int(_) | Expr::Str(_) | Expr::Bool(_) | Expr::ReadLine | Expr::ReadAll => expr,
            Expr::Var(name) => Expr::Var(self.reference(&name, locals)),
            Expr::Call(name, args) => Expr::Call(
                self.reference(&name, locals),
                args.into_iter().map(|e| self.expr(e, locals)).collect(),
            ),
            Expr::CallValue { func, args } => Expr::CallValue {
                func: Box::new(self.expr(*func, locals)),
                args: args.into_iter().map(|e| self.expr(e, locals)).collect(),
            },
            Expr::FuncRef(name) => Expr::FuncRef(self.reference(&name, locals)),
            Expr::Lambda { params, body } => {
                let mut inner = locals.clone();
                for param in &params {
                    inner.insert(param.name.clone());
                }
                Expr::Lambda {
                    params,
                    body: self.block(body, &inner),
                }
            }
            Expr::View(inner) => Expr::View(Box::new(self.expr(*inner, locals))),
            Expr::Length(inner) => Expr::Length(Box::new(self.expr(*inner, locals))),
            Expr::Letter(index, text) => Expr::Letter(
                Box::new(self.expr(*index, locals)),
                Box::new(self.expr(*text, locals)),
            ),
            Expr::Part { text, from, to } => Expr::Part {
                text: Box::new(self.expr(*text, locals)),
                from: Box::new(self.expr(*from, locals)),
                to: Box::new(self.expr(*to, locals)),
            },
            Expr::Array(items) => Expr::Array(
                items
                    .into_iter()
                    .map(|item| self.expr(item, locals))
                    .collect(),
            ),
            Expr::EmptyArray(elem) => Expr::EmptyArray(elem),
            Expr::Item(index, array) => Expr::Item(
                Box::new(self.expr(*index, locals)),
                Box::new(self.expr(*array, locals)),
            ),
            Expr::Copy(inner) => Expr::Copy(Box::new(self.expr(*inner, locals))),
            Expr::Argument(index) => Expr::Argument(Box::new(self.expr(*index, locals))),
            Expr::ArgCount => Expr::ArgCount,
            Expr::Environment(name) => Expr::Environment(Box::new(self.expr(*name, locals))),
            Expr::ReadFile(path) => Expr::ReadFile(Box::new(self.expr(*path, locals))),
            Expr::Unary(op, inner) => Expr::Unary(op, Box::new(self.expr(*inner, locals))),
            Expr::Binary(op, left, right) => Expr::Binary(
                op,
                Box::new(self.expr(*left, locals)),
                Box::new(self.expr(*right, locals)),
            ),
        }
    }
}

fn top_level_name(stmt: &Stmt) -> Option<&str> {
    match stmt {
        Stmt::Let { name, .. } | Stmt::Const { name, .. } | Stmt::FuncDef { name, .. } => {
            Some(name)
        }
        _ => None,
    }
}

fn module_stem(path: &Path) -> String {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("module");
    let mut out = String::new();
    for ch in stem.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        out.push_str("module");
    }
    out
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn resolve(raw: &str, base: Option<&Path>) -> Result<PathBuf, String> {
    let raw_path = Path::new(raw);
    let mut names = vec![raw_path.to_path_buf()];
    if raw_path.extension().is_none() {
        names.push(raw_path.with_extension("rk"));
    }

    let mut candidates: Vec<PathBuf> = Vec::new();
    if raw_path.is_absolute() {
        candidates.extend(names.iter().cloned());
    } else {
        if let Some(base) = base {
            for name in &names {
                candidates.push(base.join(name));
            }
            for name in &names {
                candidates.push(base.join("ruskmods").join(name));
            }
        }
        for name in &names {
            candidates.push(name.clone());
        }
        for name in &names {
            candidates.push(Path::new("ruskmods").join(name));
        }
    }

    for candidate in candidates {
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(format!(
        "I can't find the file '{raw}' — I looked next to the program and in ruskmods/"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    fn scratch() -> PathBuf {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("rusk-imports-{}-{}", std::process::id(), n));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn one_file_can_import_several_others() {
        let dir = scratch();
        write(
            &dir,
            "math.rk",
            "to math_double with n as a number:\n    give back n * 2\n",
        );
        write(
            &dir,
            "text.rk",
            "to text_bang with s as text:\n    give back s + \"!\"\n",
        );
        let main = write(
            &dir,
            "main.rk",
            "import \"math\"\nimport \"text\"\nsay math_double with 3\n",
        );

        let program = expand_file(&main).unwrap();
        assert_eq!(program.stmts.len(), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nested_imports_load_only_once() {
        let dir = scratch();
        write(
            &dir,
            "base.rk",
            "to base_add with a as a number and b as a number:\n    give back a + b\n",
        );
        write(
            &dir,
            "extra.rk",
            "import \"base\"\nto extra_zero:\n    give back 0\n",
        );
        let main = write(
            &dir,
            "main.rk",
            "import \"base\"\nimport \"extra\"\nsay base_add with 1 and 2\n",
        );

        let program = expand_file(&main).unwrap();
        let funcs = program
            .stmts
            .iter()
            .filter(|s| matches!(s, Stmt::FuncDef { .. }))
            .count();
        assert_eq!(funcs, 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_import_may_leave_off_the_extension() {
        let dir = scratch();
        write(&dir, "helper.rk", "to helper:\n    give back 1\n");
        let main = write(&dir, "main.rk", "import \"helper\"\n");
        let program = expand_file(&main).unwrap();
        assert!(matches!(program.stmts[0], Stmt::FuncDef { .. }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_import_names_the_line_and_file() {
        let dir = scratch();
        let main = write(&dir, "main.rk", "import \"nope\"\n");
        let err = expand_file(&main).unwrap_err();
        assert!(err.contains("main.rk"), "{err}");
        assert!(err.contains("line 1"), "{err}");
        assert!(err.contains("can't find"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cycles_are_reported_with_the_chain() {
        let dir = scratch();
        write(&dir, "a.rk", "import \"b\"\n");
        write(&dir, "b.rk", "import \"a\"\n");
        let main = write(&dir, "main.rk", "import \"a\"\n");
        let err = expand_file(&main).unwrap_err();
        assert!(err.contains("cycle"), "{err}");
        assert!(err.contains("main.rk"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn imports_inside_blocks_are_rejected() {
        let dir = scratch();
        write(&dir, "helper.rk", "to helper:\n    give back 1\n");
        let main = write(&dir, "main.rk", "if true:\n    import \"helper\"\n");
        let err = expand_file(&main).unwrap_err();
        assert!(err.contains("top level"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn func_names(program: &Program) -> Vec<String> {
        program
            .stmts
            .iter()
            .filter_map(|s| match s {
                Stmt::FuncDef { name, .. } => Some(name.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn an_alias_puts_definitions_in_a_namespace() {
        let dir = scratch();
        write(
            &dir,
            "math.rk",
            "let answer be 42\nto double with n as a number:\n    give back n * 2\n",
        );
        let main = write(
            &dir,
            "main.rk",
            "import \"math\" as math\nsay math.double with 21\nsay math.answer\n",
        );
        let program = expand_file(&main).unwrap();
        assert_eq!(func_names(&program), vec!["math.double"]);
        assert!(matches!(
            &program.stmts[2],
            Stmt::Say(Expr::Call(name, _), _) if name == "math.double"
        ));
        assert!(matches!(
            &program.stmts[3],
            Stmt::Say(Expr::Var(name), _) if name == "math.answer"
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn from_imports_pull_names_out_and_can_rename_them() {
        let dir = scratch();
        write(
            &dir,
            "math.rk",
            "to double with n as a number:\n    give back n * 2\n",
        );
        let main = write(
            &dir,
            "main.rk",
            "from \"math\" import double\nfrom \"math\" import double as twice\nsay double with 3\nsay twice with 4\n",
        );
        let program = expand_file(&main).unwrap();
        assert!(matches!(
            &program.stmts[1],
            Stmt::Say(Expr::Call(name, _), _) if name == "math.double"
        ));
        assert!(matches!(
            &program.stmts[2],
            Stmt::Say(Expr::Call(name, _), _) if name == "math.double"
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_namespaced_modules_plain_imports_are_private() {
        let dir = scratch();
        write(&dir, "helper.rk", "to hidden:\n    give back 7\n");
        write(
            &dir,
            "thing.rk",
            "import \"helper\"\nto use_hidden:\n    give back hidden\n",
        );
        let main = write(
            &dir,
            "main.rk",
            "import \"thing\" as thing\nsay thing.use_hidden\n",
        );
        let program = expand_file(&main).unwrap();
        let names = func_names(&program);
        assert!(names.contains(&"thing.use_hidden".to_string()), "{names:?}");
        assert!(names.contains(&"thing.hidden".to_string()), "{names:?}");
        assert!(!names.contains(&"hidden".to_string()), "{names:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn from_imports_of_same_stem_files_get_distinct_namespaces() {
        let dir = scratch();
        write(&dir, "a/math.rk", "to which:\n    give back 1\n");
        write(&dir, "b/math.rk", "to which:\n    give back 2\n");
        let main = write(
            &dir,
            "main.rk",
            "from \"a/math\" import which\nfrom \"b/math\" import which as other\nsay which\nsay other\n",
        );
        let program = expand_file(&main).unwrap();
        assert!(matches!(
            &program.stmts[2],
            Stmt::Say(Expr::Var(name), _) if name == "math.which"
        ));
        assert!(matches!(
            &program.stmts[3],
            Stmt::Say(Expr::Var(name), _) if name == "math_2.which"
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn two_selective_imports_cannot_share_a_local_name() {
        let dir = scratch();
        write(&dir, "a/math.rk", "to which:\n    give back 1\n");
        write(&dir, "b/math.rk", "to which:\n    give back 2\n");
        let main = write(
            &dir,
            "main.rk",
            "from \"a/math\" import which\nfrom \"b/math\" import which\n",
        );
        let err = expand_file(&main).unwrap_err();
        assert!(err.contains("already imported"), "{err}");
        assert!(err.contains("'as'"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn two_aliases_cannot_share_a_module_name() {
        let dir = scratch();
        write(&dir, "a.rk", "to one:\n    give back 1\n");
        write(&dir, "b.rk", "to two:\n    give back 2\n");
        let main = write(
            &dir,
            "main.rk",
            "import \"a\" as shared\nimport \"b\" as shared\n",
        );
        let err = expand_file(&main).unwrap_err();
        assert!(err.contains("already used"), "{err}");
        assert!(err.contains("shared"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
