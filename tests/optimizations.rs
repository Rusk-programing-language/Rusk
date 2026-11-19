//! Regression tests for the compiler optimizations.
//!
//! These tests avoid timing assertions: they check that the pass has work to
//! do, that it produces smaller code, and that both paths print the same thing.
//! `benches/compiler.rs` is where the timing numbers live.

#![cfg(unix)]

use std::path::PathBuf;
use std::process::Command;

const SOURCE: &str = r#"let total be 0
repeat 100 times:
    set total to total + ((6 * 7) - 2)
    if false:
        set total to total + 100
    otherwise:
        set total to total
say total
"#;

fn program() -> rusk::ast::Program {
    let tokens = rusk::lexer::lex(SOURCE).expect("test source should lex");
    rusk::parser::parse(tokens).expect("test source should parse")
}

#[test]
fn optimization_folds_and_removes_code() {
    let (optimized, stats) = rusk::optimize::optimize(&program());
    assert!(stats.folded > 0, "expected literal folds: {stats:?}");
    assert!(stats.removed > 0, "expected dead code removal: {stats:?}");
    assert!(
        stats.constant_branches > 0,
        "expected a constant branch: {stats:?}"
    );
    assert!(!optimized.stmts.is_empty());
}

#[test]
fn optimization_shrinks_generated_code() {
    let source = program();
    let plain =
        rusk::codegen::compile_for_with_optimization(&source, rusk::Target::X86_64, false).unwrap();
    let fast =
        rusk::codegen::compile_for_with_optimization(&source, rusk::Target::X86_64, true).unwrap();
    assert!(
        fast.text.len() < plain.text.len(),
        "optimized text should be smaller: {} -> {}",
        plain.text.len(),
        fast.text.len()
    );
}

fn bake(label: &str, optimize: bool) -> PathBuf {
    let target = rusk::Target::host();
    let baked = rusk::codegen::compile_for_with_optimization(&program(), target, optimize).unwrap();
    let bytes = rusk::executable_for(
        target,
        &baked.text,
        &baked.rodata,
        baked.bss,
        &baked.imports,
    )
    .unwrap();
    let dir = std::env::temp_dir().join(format!("rusk-opt-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(label);
    rusk::write_executable(&path, target, &bytes).unwrap();
    path
}

fn run(path: &PathBuf) -> String {
    let out = Command::new(path).output().unwrap_or_else(|error| {
        if error.raw_os_error() == Some(26) {
            std::thread::sleep(std::time::Duration::from_millis(10));
            Command::new(path).output().unwrap()
        } else {
            panic!("optimized program should run: {error}");
        }
    });
    assert!(out.status.success(), "optimized program failed");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn both_paths_print_the_same_answer() {
    let plain = bake("plain", false);
    let fast = bake("fast", true);
    assert_eq!(run(&plain), "4000");
    assert_eq!(run(&fast), "4000");
}
