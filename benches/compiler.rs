//! A dependency-free benchmark for the compiler optimizations.
//!
//! Run with `cargo bench`. It prints code-generation time, generated text
//! size, and the run time of the baked programs with the optimization pass off
//! and on. The numbers come from the same source and the same host target.

use std::time::{Duration, Instant};

#[cfg(unix)]
use std::path::PathBuf;
#[cfg(unix)]
use std::process::Command;

const SOURCE: &str = r#"let total be 0
repeat 2000000 times:
    set total to total + ((2 + 3) * (4 + 5))
    if false:
        set total to total + 999
    otherwise:
        set total to total
say total
"#;

fn program() -> rusk::ast::Program {
    let tokens = rusk::lexer::lex(SOURCE).expect("benchmark source should lex");
    rusk::parser::parse(tokens).expect("benchmark source should parse")
}

fn seconds(elapsed: Duration) -> f64 {
    elapsed.as_secs_f64().max(1e-9)
}

fn ratio(before: f64, after: f64) -> f64 {
    before / after.max(1e-9)
}

#[cfg(unix)]
fn bake_to(label: &str, optimize: bool, target: rusk::Target) -> (PathBuf, Vec<u8>) {
    let baked = rusk::codegen::compile_for_with_optimization(&program(), target, optimize)
        .unwrap_or_else(|e| panic!("{label} bake failed: {e}"));
    let bytes = rusk::executable_for(
        target,
        &baked.text,
        &baked.rodata,
        baked.bss,
        &baked.imports,
    )
    .expect("benchmark should wrap an executable");
    let dir = std::env::temp_dir().join(format!("rusk-bench-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp directory should be writable");
    let path = dir.join(format!("{label}-{}", std::process::id()));
    rusk::write_executable(&path, target, &bytes).expect("benchmark executable should be written");
    (path, bytes)
}

#[cfg(unix)]
fn run_once(path: &PathBuf) -> (Duration, String) {
    let start = Instant::now();
    let out = Command::new(path)
        .output()
        .unwrap_or_else(|e| panic!("benchmark program should run: {e}"));
    let elapsed = start.elapsed();
    assert!(out.status.success(), "benchmark program failed");
    let output = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (elapsed, output)
}

#[cfg(unix)]
fn median(mut values: Vec<Duration>) -> Duration {
    values.sort();
    values[values.len() / 2]
}

#[cfg(unix)]
fn run() {
    let target = rusk::Target::host();
    let source = program();
    let (_, stats) = rusk::optimize::optimize(&source);
    assert!(stats.folded > 0, "benchmark source should fold constants");
    assert!(
        stats.removed > 0,
        "benchmark source should remove dead code"
    );

    let rounds = 5000;
    // Warm both paths before timing so allocator and page-cache startup is not
    // charged to the first compile.
    for optimize in [false, true] {
        for _ in 0..20 {
            let _ = std::hint::black_box(
                rusk::codegen::compile_for_with_optimization(&source, target, optimize).unwrap(),
            );
        }
    }

    let start = Instant::now();
    for _ in 0..rounds {
        let _ = std::hint::black_box(
            rusk::codegen::compile_for_with_optimization(&source, target, false).unwrap(),
        );
    }
    let plain_codegen = start.elapsed();

    let start = Instant::now();
    for _ in 0..rounds {
        let _ = std::hint::black_box(
            rusk::codegen::compile_for_with_optimization(&source, target, true).unwrap(),
        );
    }
    let fast_codegen = start.elapsed();

    let plain = rusk::codegen::compile_for_with_optimization(&source, target, false).unwrap();
    let fast = rusk::codegen::compile_for_with_optimization(&source, target, true).unwrap();
    assert!(
        fast.text.len() < plain.text.len(),
        "optimization should shrink code"
    );

    let (plain_path, plain_bytes) = bake_to("plain", false, target);
    let (fast_path, fast_bytes) = bake_to("fast", true, target);
    let (_, plain_output) = run_once(&plain_path);
    let (_, fast_output) = run_once(&fast_path);
    assert_eq!(
        plain_output, fast_output,
        "both programs should print the same value"
    );
    assert_eq!(
        plain_output, "90000000",
        "benchmark program should be correct"
    );

    // The loop is long enough that process startup is a small part of each
    // run. Pair the two paths in alternating order and take the median so a
    // single scheduler pause does not decide the result.
    for _ in 0..2 {
        let _ = run_once(&plain_path);
        let _ = run_once(&fast_path);
    }
    let mut plain_runs = Vec::new();
    let mut fast_runs = Vec::new();
    for round in 0..7 {
        if round % 2 == 0 {
            let (elapsed, _) = run_once(&plain_path);
            plain_runs.push(elapsed);
            let (elapsed, _) = run_once(&fast_path);
            fast_runs.push(elapsed);
        } else {
            let (elapsed, _) = run_once(&fast_path);
            fast_runs.push(elapsed);
            let (elapsed, _) = run_once(&plain_path);
            plain_runs.push(elapsed);
        }
    }
    let plain_run = median(plain_runs);
    let fast_run = median(fast_runs);

    println!("compiler optimization benchmark ({rounds} compiles, 7 paired runs)");
    println!(
        "  codegen:  {:.2?} -> {:.2?}  ({:.2}x)",
        plain_codegen,
        fast_codegen,
        ratio(seconds(plain_codegen), seconds(fast_codegen))
    );
    println!(
        "  text:     {} -> {} bytes (-{} bytes)",
        plain.text.len(),
        fast.text.len(),
        plain.text.len() - fast.text.len()
    );
    println!(
        "  image:    {} -> {} bytes",
        plain_bytes.len(),
        fast_bytes.len()
    );
    println!(
        "  runtime:  {plain_run:?} -> {fast_run:?}  ({}x)",
        ratio(seconds(plain_run), seconds(fast_run))
    );
    println!(
        "  folded {} expressions, removed {} statements, resolved {} constant branches",
        stats.folded, stats.removed, stats.constant_branches
    );
}

#[cfg(not(unix))]
fn run() {
    println!("compiler optimization benchmark: executable timing needs a Unix host");
}

fn main() {
    if cfg!(debug_assertions) {
        println!("compiler benchmark skipped in debug profile; run `cargo bench`");
        return;
    }
    run();
}
