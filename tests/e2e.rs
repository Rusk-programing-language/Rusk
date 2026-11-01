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

// ---- functions -------------------------------------------------------

#[test]
fn a_function_gives_back_a_value() {
    let src = "\
to double with n as a number:
    give back n * 2
say double with 21
";
    let (stdout, code) = run("fn_double", src);
    assert_eq!(stdout, "42\n");
    assert_eq!(code, 0);
}

#[test]
fn two_parameters_and_a_local() {
    let src = "\
to add with a as a number and b as a number:
    let sum be a + b
    give back sum
say add with 2 and 40
";
    let (stdout, _) = run("fn_add", src);
    assert_eq!(stdout, "42\n");
}

#[test]
fn a_return_value_can_be_part_of_a_bigger_expression() {
    let src = "\
to double with n as a number:
    give back n * 2
say (double with 3) + (double with 4)
";
    let (stdout, _) = run("fn_expr", src);
    assert_eq!(stdout, "14\n");
}

#[test]
fn recursion_works() {
    let src = "\
to factorial with n as a number:
    if n is at most 1:
        give back 1
    give back n * factorial with n - 1
say factorial with 5
";
    let (stdout, _) = run("fn_fact", src);
    assert_eq!(stdout, "120\n");
}

#[test]
fn text_and_zero_argument_calls() {
    let src = "\
to greet with who as text:
    say \"hello,\"
    say who
to roll:
    say \"rolling\"
greet with \"world\"
roll
";
    let (stdout, _) = run("fn_text", src);
    assert_eq!(stdout, "hello,\nworld\nrolling\n");
}

#[test]
fn a_function_can_be_called_before_it_is_defined() {
    let src = "\
say later with 5
to later with n as a number:
    give back n + 1
";
    let (stdout, _) = run("fn_forward", src);
    assert_eq!(stdout, "6\n");
}

#[test]
fn arity_mismatches_are_explained() {
    let src = "\
to double with n as a number:
    give back n * 2
say double with 1 and 2
";
    let err = rusk::bake(src).unwrap_err();
    assert!(err.contains("takes 1 thing"), "{err}");
}

#[test]
fn give_back_outside_a_function_is_an_error() {
    let err = rusk::bake("give back 1\n").unwrap_err();
    assert!(err.contains("inside a function"), "{err}");
}

