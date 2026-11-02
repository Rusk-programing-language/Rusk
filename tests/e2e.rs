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
    let (stdout, _, code) = run_full(name, src);
    (stdout, code)
}

fn run_full(name: &str, src: &str) -> (String, String, i32) {
    let path = bake_to_disk(name, src);
    let out = Command::new(&path).output().expect("program should start");
    let _ = std::fs::remove_file(&path);
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
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

// ---- text -------------------------------------------------------------

#[test]
fn text_joins_with_plus() {
    let src = "\
let who be \"world\"
let greeting be \"Hello, \" + who + \"!\"
say greeting
";
    let (stdout, _) = run("text_join", src);
    assert_eq!(stdout, "Hello, world!\n");
}

#[test]
fn text_compares_like_a_dictionary() {
    let src = "\
say \"apple\" is less than \"banana\"
say \"apple\" is equal to \"apple\"
say \"b\" is more than \"abc\"
say \"abc\" is not \"abd\"
say \"\" is \"\"
";
    let (stdout, _) = run("text_cmp", src);
    assert_eq!(stdout, "true\ntrue\ntrue\ntrue\ntrue\n");
}

#[test]
fn a_function_can_build_text() {
    let src = "\
to shout with s as text:
    give back s + \"!\"

to fill with n as a number and word as text:
    let out be \"\"
    for each i from 1 to n:
        set out to out + word
    give back out

say shout with \"hey\"
say fill with 3 and \"ha\"
";
    let (stdout, _) = run("text_fn", src);
    assert_eq!(stdout, "hey!\nhahaha\n");
}

// ---- loops and branches ----------------------------------------------

#[test]
fn while_repeats_until_the_condition_fails() {
    let src = "\
let n be 0
while n is less than 3:
    say n
    set n to n + 1
";
    let (stdout, _) = run("while", src);
    assert_eq!(stdout, "0\n1\n2\n");
}

#[test]
fn for_each_counts_up_inclusively() {
    let src = "\
for each i from 1 to 3:
    say i
";
    let (stdout, _) = run("for_each", src);
    assert_eq!(stdout, "1\n2\n3\n");
}

#[test]
fn for_each_keeps_its_name_inside_the_loop() {
    let src = "\
let i be 10
for each i from 1 to 2:
    say i
say i
for each j from 3 to 1:
    say \"never\"
";
    let (stdout, _) = run("for_each_scope", src);
    assert_eq!(stdout, "1\n2\n10\n");
}

#[test]
fn otherwise_if_chains() {
    let src = "\
to grade with score as a number:
    if score is at least 90:
        give back \"A\"
    otherwise if score is at least 70:
        give back \"B\"
    otherwise:
        give back \"C\"

say grade with 95
say grade with 75
say grade with 10
";
    let (stdout, _) = run("else_if", src);
    assert_eq!(stdout, "A\nB\nC\n");
}

#[test]
fn bare_is_means_equality() {
    let (stdout, _) = run("bare_is", "let x be 5\nsay x is 5\n");
    assert_eq!(stdout, "true\n");
}

// ---- text length, indexing, and slicing ------------------------------

#[test]
fn text_can_be_measured_indexed_and_sliced() {
    let src = "\
let s be \"hello\"
say length of s
say letter 1 of s
say letter 5 of s
say part 2 to 4 of s
say part 2 to 1 of s
say part 6 to 5 of s
say \"Hello, \" + letter 1 of \"world\" + part 2 to 5 of \"world\" + \"!\"

to middle with t as text:
    give back part 2 to (length of t - 1) of t

say middle with \"Rusk\"
";
    let (stdout, code) = run("text_ops", src);
    assert_eq!(stdout, "5\nh\no\nell\n\n\nHello, world!\nus\n");
    assert_eq!(code, 0);
}

#[test]
fn text_index_errors_stop_the_program() {
    let src = "say \"start\"\nsay letter 9 of \"abc\"\n";
    let (stdout, stderr, code) = run_full("letter_bounds", src);
    assert_eq!(stdout, "start\n");
    assert_eq!(code, 1);
    assert!(
        stderr.contains("line 2: a letter outside the text"),
        "{stderr}"
    );
}

#[test]
fn text_part_errors_stop_the_program() {
    let (_, stderr, code) = run_full("part_bounds", "say part 1 to 9 of \"abc\"\n");
    assert_eq!(code, 1);
    assert!(
        stderr.contains("line 1: a part outside the text"),
        "{stderr}"
    );
}

// ---- the interactive session -----------------------------------------

#[test]
fn the_repl_remembers_state_and_echoes_values() {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let mut child = Command::new(env!("CARGO_BIN_EXE_rusk"))
        .arg("repl")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the repl should start");

    let input = "\
let x be 40
x + 2
to double with n as a number:
    give back n * 2

double with 21
:quit
";
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stdout, "42\n42\n");
    assert!(out.status.success());
}



// ---- lists and constants ---------------------------------------------

#[test]
fn lists_hold_items_and_share_their_storage() {
    let src = "\
let xs be [10, 20, 30]
say length of xs
say item 2 of xs
set item 2 of xs to 99
say xs
let ys be xs
set item 1 of ys to 7
say xs
say an empty list of numbers
let flags be [true, false]
say flags
let words be [\"a\", \"b\"]
say words
";
    let (stdout, code) = run("lists", src);
    assert_eq!(
        stdout,
        "3\n20\n[10, 99, 30]\n[7, 99, 30]\n[]\n[true, false]\n[a, b]\n"
    );
    assert_eq!(code, 0);
}

#[test]
fn a_list_can_be_a_parameter_and_be_walked_with_for_each() {
    let src = "\
to sum with xs as a list of numbers:
    let total be 0
    for each x in xs:
        set total to total + x
    give back total

say sum with [1, 2, 3, 4]
say sum with an empty list of numbers
";
    let (stdout, code) = run("list_sum", src);
    assert_eq!(stdout, "10\n0\n");
    assert_eq!(code, 0);
}

#[test]
fn a_list_index_outside_the_list_stops_the_program() {
    let src = "say \"start\"\nsay item 5 of [1, 2]\n";
    let (stdout, stderr, code) = run_full("list_bounds", src);
    assert_eq!(stdout, "start\n");
    assert_eq!(code, 1);
    assert!(
        stderr.contains("line 2: an item outside the list"),
        "{stderr}"
    );
}

#[test]
fn set_item_checks_its_bounds_too() {
    let src = "let xs be [1]\nset item 2 of xs to 9\n";
    let (_, stderr, code) = run_full("set_bounds", src);
    assert_eq!(code, 1);
    assert!(
        stderr.contains("line 2: an item outside the list"),
        "{stderr}"
    );
}

#[test]
fn a_constant_cannot_be_set() {
    let err = rusk::bake("constant pi be 3\nset pi to 4\n").unwrap_err();
    assert!(err.contains("constant"), "{err}");
}

#[test]
fn lists_are_homogeneous_and_empty_lists_need_a_kind() {
    let err = rusk::bake("let xs be [1, \"two\"]\n").unwrap_err();
    assert!(err.contains("mixes"), "{err}");

    let err = rusk::bake("let xs be []\n").unwrap_err();
    assert!(err.contains("empty list needs a kind"), "{err}");
}

// ---- imports ---------------------------------------------------------

#[test]
fn a_program_can_import_several_modules() {
    let dir = std::env::temp_dir().join(format!("rusk-e2e-imports-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("lib")).unwrap();
    std::fs::write(
        dir.join("lib/base.rk"),
        "to base_add with a as a number and b as a number:\n    give back a + b\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("lib/extra.rk"),
        "import \"base\"\nto extra_twice with n as a number:\n    give back base_add with n and n\n",
    )
    .unwrap();
    let main = dir.join("main.rk");
    std::fs::write(
        &main,
        "import \"lib/base\"\nimport \"lib/extra\"\nsay base_add with 2 and 3\nsay extra_twice with 4\n",
    )
    .unwrap();

    let (bytes, _) = rusk::bake_path(&main).unwrap();
    let out = dir.join("main");
    rusk::elf::write(&out, &bytes).unwrap();
    let result = Command::new(&out).output().expect("imported program should start");
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(String::from_utf8_lossy(&result.stdout), "5\n8\n");
    assert_eq!(result.status.code(), Some(0));
}

#[test]
fn a_function_can_give_back_a_list() {
    let src = "\
to pair with n as a number:
    give back [n, n + 1]

say pair with 5
say item 2 of pair with 5
let xs be pair with 7
say length of xs
";
    let (stdout, code) = run("list_return", src);
    assert_eq!(stdout, "[5, 6]\n6\n2\n");
    assert_eq!(code, 0);
}
