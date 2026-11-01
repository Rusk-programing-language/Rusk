//! End-to-end: bake a `.rk` program to a real executable and run it.

use std::process::Command;

fn bake_to_disk(name: &str, src: &str) -> std::path::PathBuf {
    let (bytes, _) = rusk::bake(src).unwrap_or_else(|e| panic!("{name} failed to bake: {e}"));
    let dir = std::env::temp_dir().join(format!("rusk-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    rusk::elf::write(&path, &bytes).unwrap();
    path
}

fn run(name: &str, src: &str) -> (String, i32) {
    let path = bake_to_disk(name, src);
    let out = Command::new(&path).output().expect("program should start");
    let _ = std::fs::remove_file(&path);
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn hello_world() {
    let (stdout, code) = run("hello", "say \"Hello, world.\"\n");
    assert_eq!(stdout, "Hello, world.\n");
    assert_eq!(code, 0);
}

#[test]
fn arithmetic_and_precedence() {
    let (stdout, _) = run(
        "arith",
        "say 1 + 2 * 3\nsay (1 + 2) * 3\nsay 100 / 3\nsay 100 / -7\nsay -15 + 3\n",
    );
    assert_eq!(stdout, "7\n9\n33\n-14\n-12\n");
}

#[test]
fn variables_keep_their_kind() {
    let (stdout, _) = run("vars", "let x be 41\nset x to x + 1\nsay x\n");
    assert_eq!(stdout, "42\n");
}

#[test]
fn english_comparisons_and_blocks() {
    let src = "\
let t be 180
if t is more than 150:
    say \"hot\"
otherwise:
    say \"mild\"
if t is at most 100:
    say \"cold\"
";
    let (stdout, _) = run("decide", src);
    assert_eq!(stdout, "hot\n");
}

#[test]
fn repeat_runs_the_body() {
    let (stdout, _) = run("loop", "repeat 3 times:\n    say \"loaf\"\n");
    assert_eq!(stdout, "loaf\nloaf\nloaf\n");
}

#[test]
fn strings_are_values_too() {
    let (stdout, _) = run("strs", "let who be \"world\"\nsay who\nsay \"hello\"\n");
    assert_eq!(stdout, "world\nhello\n");
}

#[test]
fn booleans_print_as_words() {
    let (stdout, _) = run("bools", "say 1 is less than 2\nsay not true\n");
    assert_eq!(stdout, "true\nfalse\n");
}

#[test]
fn big_and_small_numbers() {
    let (stdout, _) = run("big", "say 0\nsay -1\nsay 9223372036854775807\n");
    assert_eq!(stdout, "0\n-1\n9223372036854775807\n");
}

#[test]
fn syntax_errors_point_at_a_line() {
    let err = rusk::bake("let x 5\n").unwrap_err();
    assert!(err.contains("line 1"), "{err}");
    assert!(err.contains("expected 'be'"), "{err}");
}

#[test]
fn unknown_names_are_explained() {
    let err = rusk::bake("say nope\n").unwrap_err();
    assert!(err.contains("let nope be"), "{err}");
}
