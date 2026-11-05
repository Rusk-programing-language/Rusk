//! Expands `import` statements before type checking.
//!
//! Imports are textual inclusions with one twist: a file is loaded at most
//! once per bake, so several modules can import the same helper without
//! defining it twice. Definitions land in one global namespace, which is why
//! the modules in `ruskmods/` prefix their names.
//!
//! Paths resolve relative to the file that writes the import, then relative to
//! the current directory, then inside a nearby `ruskmods/` folder. An import
//! may name a file directly (`import "lib/text.rk"`) or leave off the `.rk`
//! (`import "lib/text"`).

use crate::ast::{Program, Stmt};
use crate::lexer::lex;
use crate::parser::parse;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

struct Loader {
    loaded: HashSet<PathBuf>,
    stack: Vec<PathBuf>,
}

/// Expand imports in a source string. `base` is the directory relative paths
/// should be resolved from; `None` means the current directory.
pub fn expand(source: &str, base: Option<&Path>) -> Result<Program, String> {
    Loader::new().expand_source(source, base, None)
}

/// Read a file and expand every import it reaches. Errors name the file they
/// came from.
pub fn expand_file(path: &Path) -> Result<Program, String> {
    let source =
        std::fs::read_to_string(path).map_err(|e| format!("can't read {}: {e}", path.display()))?;
    let canon = canonical(path);
    let mut loader = Loader::new();
    loader.stack.push(canon);
    let program = loader.expand_source(&source, path.parent(), Some(path))?;
    loader.stack.pop();
    Ok(program)
}

impl Loader {
    fn new() -> Self {
        Loader {
            loaded: HashSet::new(),
            stack: Vec::new(),
        }
    }

    fn expand_source(
        &mut self,
        source: &str,
        base: Option<&Path>,
        label: Option<&Path>,
    ) -> Result<Program, String> {
        let parsed = parse(lex(source)?).map_err(|e| match label {
            Some(path) => format!("in {}: {e}", path.display()),
            None => e,
        })?;

        let mut stmts = Vec::new();
        for stmt in parsed.stmts {
            let Stmt::Import { path, line } = stmt else {
                stmts.push(stmt);
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
            if self.loaded.contains(&canon) {
                continue;
            }

            let imported = std::fs::read_to_string(&resolved)
                .map_err(|e| format!("can't read {}: {e}", resolved.display()))?;
            self.stack.push(canon.clone());
            let expanded = self.expand_source(&imported, resolved.parent(), Some(&resolved))?;
            self.stack.pop();
            self.loaded.insert(canon);
            stmts.extend(expanded.stmts);
        }
        Ok(Program { stmts })
    }
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
}
