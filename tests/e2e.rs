//! End-to-end: bake a `.rk` program to a real executable and run it.
//!
//! By default the tests bake for the host architecture. Set
//! `RUSK_E2E_TARGET=x86_64` (or `aarch64`) to exercise another backend, and
//! point `RUSK_E2E_RUNNER` at a user-mode emulator such as `qemu-x86_64` when
//! that target is not the host. The runner is also found on `PATH`.

use std::process::Command;

fn e2e_target() -> rusk::Target {
    match std::env::var("RUSK_E2E_TARGET").ok().as_deref() {
        Some("x86_64") | Some("x86-64") | Some("amd64") => rusk::Target::X86_64,
        Some("aarch64") | Some("arm64") => rusk::Target::AArch64,
        _ => rusk::Target::host(),
    }
}

fn bake_src(src: &str) -> Result<(Vec<u8>, u64), String> {
    rusk::bake_with_target(src, e2e_target())
}

fn find_in_path(name: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// A command that runs a baked program, through an emulator when the bake
/// target differs from the host.
fn program_command(path: &std::path::Path) -> Command {
    let target = e2e_target();
    if target == rusk::Target::host() {
        return Command::new(path);
    }
    let runner = std::env::var_os("RUSK_E2E_RUNNER")
        .map(std::path::PathBuf::from)
        .or_else(|| find_in_path(&format!("qemu-{}", target.name())))
        .or_else(|| find_in_path(&format!("qemu-{}-static", target.name())))
        .unwrap_or_else(|| {
            panic!(
                "set RUSK_E2E_RUNNER to a {} user-mode emulator (for example qemu-{})",
                target.name(),
                target.name()
            )
        });
    let mut command = Command::new(runner);
    command.arg(path);
    command
}

fn bake_to_disk(name: &str, src: &str) -> std::path::PathBuf {
    let (bytes, _) = bake_src(src).unwrap_or_else(|e| panic!("{name} failed to bake: {e}"));
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
    let out = program_command(&path)
        .output()
        .expect("program should start");
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
fn and_or_short_circuit() {
    let src = "\
to noisy with value as a truth value:
    say \"checked\"
    give back value

say false and noisy with true
say true or noisy with false
say true and noisy with true
say false or noisy with true
";
    let (stdout, _) = run("and_or", src);
    assert_eq!(stdout, "false\ntrue\nchecked\ntrue\nchecked\ntrue\n");
}

#[test]
fn and_binds_tighter_than_or() {
    let src = "\
say true and false or true
say false or false and true
say not false and not false
";
    let (stdout, _) = run("and_or_precedence", src);
    assert_eq!(stdout, "true\nfalse\ntrue\n");
}

#[test]
fn and_still_separates_call_arguments() {
    let src = "\
to both with a as a truth value and b as a truth value:
    give back a and b

say both with true and true
say both with true and false
";
    let (stdout, _) = run("and_args", src);
    assert_eq!(stdout, "true\nfalse\n");
}

#[test]
fn and_needs_truth_values() {
    let err = bake_src("say 1 and 2\n").unwrap_err();
    assert!(err.contains("joins truth values"), "{err}");
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
    let err = bake_src("let x 5\n").unwrap_err();
    assert!(err.contains("line 1"), "{err}");
    assert!(err.contains("expected 'be'"), "{err}");
}

#[test]
fn unknown_names_are_explained() {
    let err = bake_src("say nope\n").unwrap_err();
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
    let err = bake_src(src).unwrap_err();
    assert!(err.contains("takes 1 thing"), "{err}");
}

#[test]
fn give_back_outside_a_function_is_an_error() {
    let err = bake_src("give back 1\n").unwrap_err();
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
fn break_leaves_a_repeat_loop_early() {
    let src = "\
repeat 5 times:
    say \"turn\"
    break
say \"done\"
";
    let (stdout, _) = run("break_repeat", src);
    assert_eq!(stdout, "turn\ndone\n");
}

#[test]
fn continue_skips_the_rest_of_a_loop_body() {
    let src = "\
for each i from 1 to 5:
    if i is 3:
        continue
    say i
";
    let (stdout, _) = run("continue_each", src);
    assert_eq!(stdout, "1\n2\n4\n5\n");
}

#[test]
fn break_works_in_a_while_loop() {
    let src = "\
let n be 0
while true:
    if n is 2:
        break
    say n
    set n to n + 1
say \"stopped\"
";
    let (stdout, _) = run("break_while", src);
    assert_eq!(stdout, "0\n1\nstopped\n");
}

#[test]
fn break_leaves_a_for_each_in_loop_early() {
    let src = "\
let xs be [\"a\", \"b\", \"c\"]
for each word in xs:
    if word is \"b\":
        break
    say word
";
    let (stdout, _) = run("break_each_in", src);
    assert_eq!(stdout, "a\n");
}

#[test]
fn break_only_leaves_the_innermost_loop() {
    let src = "\
for each i from 1 to 2:
    for each j from 1 to 3:
        if j is 2:
            break
        say j
    say i
";
    let (stdout, _) = run("break_nested", src);
    assert_eq!(stdout, "1\n1\n1\n2\n");
}

#[test]
fn break_outside_a_loop_is_rejected() {
    let err = bake_src("break\n").unwrap_err();
    assert!(err.contains("only works inside a loop"), "{err}");
}

#[test]
fn continue_outside_a_loop_is_rejected() {
    let err = bake_src("continue\n").unwrap_err();
    assert!(err.contains("only works inside a loop"), "{err}");
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
fn lists_hold_items_and_copy_of_is_independent() {
    let src = "\
let xs be [10, 20, 30]
say length of xs
say item 2 of xs
set item 2 of xs to 99
say xs
let ys be copy of xs
set item 1 of ys to 7
say xs
say ys
say an empty list of numbers
let flags be [true, false]
say flags
let words be [\"a\", \"b\"]
say words
";
    let (stdout, code) = run("lists", src);
    assert_eq!(
        stdout,
        "3\n20\n[10, 99, 30]\n[10, 99, 30]\n[7, 99, 30]\n[]\n[true, false]\n[a, b]\n"
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
fn add_grows_a_list() {
    let src = "\
let xs be [1, 2]
add 3 to xs
say xs
say length of xs
let ys be an empty list of numbers
add 7 to ys
say ys
let words be [\"rye\"]
add \"wheat\" to words
say words
let flags be [true]
add false to flags
say flags
";
    let (stdout, code) = run("grow_list", src);
    assert_eq!(stdout, "[1, 2, 3]\n3\n[7]\n[rye, wheat]\n[true, false]\n");
    assert_eq!(code, 0);
}

#[test]
fn add_can_grow_a_list_inside_a_function() {
    let src = "\
to doubled with xs as a list of numbers:
    let ys be an empty list of numbers
    for each x in xs:
        add x * 2 to ys
    give back ys

say doubled with [1, 2, 3]
";
    let (stdout, code) = run("grow_local", src);
    assert_eq!(stdout, "[2, 4, 6]\n");
    assert_eq!(code, 0);
}

#[test]
fn adding_text_moves_it_into_the_list() {
    let err = bake_src("let a be \"hi\"\nlet xs be [\"x\"]\nadd a to xs\nsay a\n").unwrap_err();
    assert!(err.contains("moved already"), "{err}");
}

#[test]
fn add_checks_the_item_kind() {
    let err = bake_src("let xs be [1]\nadd \"two\" to xs\n").unwrap_err();
    assert!(err.contains("this list holds a number"), "{err}");
}

#[test]
fn lists_grow_past_their_starting_capacity() {
    let src = "\
let xs be an empty list of numbers
let i be 1
while i is at most 20:
    add i to xs
    set i to i + 1
say xs
say length of xs
remove item 10 from xs
say xs
say length of xs
let words be an empty list of text
add \"a\" to words
add \"bb\" to words
add \"ccc\" to words
add \"dddd\" to words
add \"eeeee\" to words
say words
remove last from words
say words
";
    let (stdout, code) = run("grow_capacity", src);
    assert_eq!(
        stdout,
        "[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20]\n\
         20\n\
         [1, 2, 3, 4, 5, 6, 7, 8, 9, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20]\n\
         19\n\
         [a, bb, ccc, dddd, eeeee]\n\
         [a, bb, ccc, dddd]\n"
    );
    assert_eq!(code, 0);
}

#[test]
fn remove_last_and_remove_item_shift_later_items() {
    let src = "\
let xs be [1, 2, 3, 4]
remove last from xs
say xs
remove item 1 from xs
say xs
remove item 2 from xs
say xs
add 9 to xs
say xs
let flags be [true, false, true]
remove item 2 from flags
say flags
";
    let (stdout, code) = run("remove_items", src);
    assert_eq!(stdout, "[1, 2, 3]\n[2, 3]\n[2]\n[2, 9]\n[true, true]\n");
    assert_eq!(code, 0);
}

#[test]
fn removal_keeps_a_copy_of_the_list_independent() {
    let src = "\
let xs be [1, 2, 3]
let ys be copy of xs
remove item 2 from ys
say xs
say ys
remove last from xs
say xs
say ys
";
    let (stdout, code) = run("remove_copy", src);
    assert_eq!(stdout, "[1, 2, 3]\n[1, 3]\n[1, 2]\n[1, 3]\n");
    assert_eq!(code, 0);
}

#[test]
fn removing_from_an_empty_list_stops_the_program() {
    let src = "say \"start\"\nlet xs be an empty list of numbers\nremove last from xs\n";
    let (stdout, stderr, code) = run_full("remove_empty", src);
    assert_eq!(stdout, "start\n");
    assert_eq!(code, 1);
    assert!(
        stderr.contains("line 3: there's nothing to remove"),
        "{stderr}"
    );
}

#[test]
fn removing_an_item_outside_the_list_stops_the_program() {
    let src = "say \"start\"\nlet xs be [1, 2]\nremove item 3 from xs\n";
    let (stdout, stderr, code) = run_full("remove_bounds", src);
    assert_eq!(stdout, "start\n");
    assert_eq!(code, 1);
    assert!(
        stderr.contains("line 3: an item outside the list"),
        "{stderr}"
    );
}

#[test]
fn remove_checks_the_target_and_index_kinds() {
    let err = bake_src("let n be 1\nremove last from n\n").unwrap_err();
    assert!(err.contains("'remove' needs a list"), "{err}");

    let err = bake_src("let xs be [1]\nremove item \"one\" from xs\n").unwrap_err();
    assert!(err.contains("item number must be a number"), "{err}");

    let err = bake_src("constant xs be [1]\nremove last from xs\n").unwrap_err();
    assert!(err.contains("constant"), "{err}");
}

#[test]
fn a_constant_list_cannot_grow() {
    let err = bake_src("constant xs be [1]\nadd 2 to xs\n").unwrap_err();
    assert!(err.contains("constant"), "{err}");
}

#[test]
fn a_constant_cannot_be_set() {
    let err = bake_src("constant pi be 3\nset pi to 4\n").unwrap_err();
    assert!(err.contains("constant"), "{err}");
}

#[test]
fn lists_are_homogeneous_and_empty_lists_need_a_kind() {
    let err = bake_src("let xs be [1, \"two\"]\n").unwrap_err();
    assert!(err.contains("mixes"), "{err}");

    let err = bake_src("let xs be []\n").unwrap_err();
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

    let (bytes, _) = rusk::bake_path_with_target(&main, e2e_target()).unwrap();
    let out = dir.join("main");
    rusk::elf::write(&out, &bytes).unwrap();
    let result = program_command(&out)
        .output()
        .expect("imported program should start");
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(String::from_utf8_lossy(&result.stdout), "5\n8\n");
    assert_eq!(result.status.code(), Some(0));
}

#[test]
fn modules_can_be_named_and_selectively_imported() {
    let dir = std::env::temp_dir().join(format!("rusk-e2e-modules-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("math.rk"),
        "let answer be 42\nto double with n as a number:\n    give back n * 2\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("text.rk"),
        "to bang with s as text:\n    give back s + \"!\"\n",
    )
    .unwrap();
    let main = dir.join("main.rk");
    std::fs::write(
        &main,
        "import \"math\" as math\nfrom \"text\" import bang\nfrom \"math\" import double as twice\nsay math.double with 21\nsay math.answer\nsay twice with 5\nsay bang with \"hi\"\n",
    )
    .unwrap();

    let (bytes, _) = rusk::bake_path_with_target(&main, e2e_target()).unwrap();
    let out = dir.join("main");
    rusk::elf::write(&out, &bytes).unwrap();
    let result = program_command(&out)
        .output()
        .expect("module program should start");
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(String::from_utf8_lossy(&result.stdout), "42\n42\n10\nhi!\n");
    assert_eq!(result.status.code(), Some(0));
}

#[test]
fn a_namespaced_modules_own_imports_stay_private() {
    let dir = std::env::temp_dir().join(format!("rusk-e2e-private-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("helper.rk"), "to hidden:\n    give back 7\n").unwrap();
    std::fs::write(
        dir.join("thing.rk"),
        "import \"helper\"\nto use_hidden:\n    give back hidden\n",
    )
    .unwrap();
    let main = dir.join("main.rk");
    std::fs::write(&main, "import \"thing\" as thing\nsay thing.use_hidden\n").unwrap();

    let (bytes, _) = rusk::bake_path_with_target(&main, e2e_target()).unwrap();
    let out = dir.join("main");
    rusk::elf::write(&out, &bytes).unwrap();
    let result = program_command(&out)
        .output()
        .expect("private import program should start");
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(String::from_utf8_lossy(&result.stdout), "7\n");
    assert_eq!(result.status.code(), Some(0));
}

#[test]
fn nested_lists_nest_and_deep_copy() {
    let src = "\
let rows be [[1, 2], [3, 4], an empty list of numbers]
say rows
say item 2 of item 1 of rows
set item 3 of rows to [5]
add [6, 7] to rows
say rows
let echo be copy of rows
remove last from rows
say rows
say echo
let deep be [[[1, 2], [3]], [[4]]]
say deep
let deep_echo be copy of deep
remove item 1 from deep
say deep
say deep_echo
let empties be an empty list of lists of text
add [\"a\"] to empties
say empties
for each row in echo:
    say row
";
    let (stdout, code) = run("nested_lists", src);
    assert_eq!(
        stdout,
        "\
[[1, 2], [3, 4], []]
2
[[1, 2], [3, 4], [5], [6, 7]]
[[1, 2], [3, 4], [5]]
[[1, 2], [3, 4], [5], [6, 7]]
[[[1, 2], [3]], [[4]]]
[[[4]]]
[[[1, 2], [3]], [[4]]]
[[a]]
[1, 2]
[3, 4]
[5]
[6, 7]
"
    );
    assert_eq!(code, 0);
}

#[test]
fn nested_list_kinds_are_still_homogeneous() {
    let err = bake_src("let xs be [[1], [\"two\"]]\n").unwrap_err();
    assert!(err.contains("mixes"), "{err}");

    let err = bake_src("let xs be an empty list of lists of numbers\nadd 1 to xs\n").unwrap_err();
    assert!(err.contains("this list holds"), "{err}");
    assert!(err.contains("list of numbers"), "{err}");
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

// ---- ownership and runtime safety ------------------------------------

#[test]
fn using_a_name_after_it_moves_is_a_compile_error() {
    let src = "\
let xs be [1, 2]
let ys be xs
say xs
";
    let err = bake_src(src).unwrap_err();
    assert!(err.contains("moved already"), "{err}");
    assert!(err.contains("line 3"), "{err}");
}

#[test]
fn copy_of_keeps_text_and_lists_usable() {
    let src = "\
let greeting be \"hello\"
let echo be copy of greeting
say greeting
say echo
let xs be [\"a\", \"b\"]
let ys be copy of xs
set item 1 of ys to \"z\"
say xs
say ys
";
    let (stdout, code) = run("ownership_copy", src);
    assert_eq!(stdout, "hello\nhello\n[a, b]\n[z, b]\n");
    assert_eq!(code, 0);
}

#[test]
fn a_function_argument_moves_and_copy_of_brings_it_back() {
    let src = "\
to shout with s as text:
    say s + \"!\"

let greeting be \"hi\"
shout with copy of greeting
say greeting
";
    let (stdout, code) = run("ownership_call", src);
    assert_eq!(stdout, "hi!\nhi\n");
    assert_eq!(code, 0);
}

#[test]
fn a_constant_cannot_be_moved() {
    let err = bake_src("constant greeting be \"hi\"\nlet a be greeting\n").unwrap_err();
    assert!(err.contains("constant"), "{err}");
    assert!(err.contains("copy of"), "{err}");
}

#[test]
fn dividing_by_zero_stops_with_a_sentence() {
    let src = "say \"before\"\nsay 1 / 0\nsay \"after\"\n";
    let (stdout, stderr, code) = run_full("div_zero", src);
    assert_eq!(stdout, "before\n");
    assert_eq!(code, 1);
    assert!(
        stderr.contains("line 2: I can't divide by zero"),
        "{stderr}"
    );
}

#[test]
fn the_smallest_number_prints_and_divides() {
    let src = "\
let smallest be 0 - 9223372036854775807 - 1
say smallest
say smallest / -1
say smallest + 1
";
    let (stdout, code) = run("min_int", src);
    assert_eq!(
        stdout,
        "-9223372036854775808\n-9223372036854775808\n-9223372036854775807\n"
    );
    assert_eq!(code, 0);
}

#[test]
fn runaway_recursion_stops_with_a_sentence() {
    let src = "\
to down with n as a number and acc as a number:
    if n is 0:
        give back acc
    give back down with n + 1 and acc

say down with 1 and 0
";
    let (stdout, stderr, code) = run_full("deep_recursion", src);
    assert_eq!(stdout, "");
    assert_eq!(code, 1);
    assert!(stderr.contains("recursed too deeply"), "{stderr}");
}

#[test]
fn running_out_of_heap_stops_with_a_sentence() {
    let src = "\
let s be \"0123456789abcdef\"
repeat 30 times:
    set s to s + s
say \"unreachable\"
";
    let (stdout, stderr, code) = run_full("heap_exhaustion", src);
    assert_eq!(stdout, "");
    assert_eq!(code, 1);
    assert!(stderr.contains("ran out of memory"), "{stderr}");
}

fn run_with(
    name: &str,
    src: &str,
    args: &[&str],
    input: Option<&str>,
    envs: &[(&str, &str)],
) -> (String, String, i32) {
    use std::io::Write;
    use std::process::Stdio;

    let path = bake_to_disk(name, src);
    let mut command = program_command(&path);
    command.args(args);
    for (key, value) in envs {
        command.env(key, value);
    }

    let out = match input {
        Some(input) => {
            command.stdin(Stdio::piped());
            let mut child = command
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("program should start");
            child
                .stdin
                .take()
                .expect("piped stdin should exist")
                .write_all(input.as_bytes())
                .expect("input should be written");
            child.wait_with_output().expect("program should finish")
        }
        None => command.output().expect("program should start"),
    };

    let _ = std::fs::remove_file(&path);
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn command_line_arguments_are_readable() {
    let src = "\
say number of arguments
say argument 1
say argument 2
say \"[\" + argument 3 + \"]\"
";
    let (stdout, stderr, code) = run_with("args", src, &["alpha", "beta"], None, &[]);
    assert_eq!(stdout, "2\nalpha\nbeta\n[]\n");
    assert_eq!(stderr, "");
    assert_eq!(code, 0);
}

#[test]
fn environment_variables_are_readable_and_missing_ones_are_empty() {
    let src = "\
say environment \"RUSK_TEST_VALUE\"
say \"[\" + environment \"RUSK_SURELY_MISSING\" + \"]\"
";
    let (stdout, _, code) = run_with(
        "environment",
        src,
        &[],
        None,
        &[("RUSK_TEST_VALUE", "hello")],
    );
    assert_eq!(stdout, "hello\n[]\n");
    assert_eq!(code, 0);
}

#[test]
fn standard_input_can_be_read_a_line_at_a_time_or_all_at_once() {
    let src = "\
let first be read line
say \"first: \" + first
let rest be read all input
say \"rest: \" + rest
";
    let (stdout, _, code) = run_with("stdin", src, &[], Some("one\ntwo\nthree\n"), &[]);
    assert_eq!(stdout, "first: one\nrest: two\nthree\n\n");
    assert_eq!(code, 0);
}

#[test]
fn files_can_be_written_and_read_back() {
    let path = std::env::temp_dir().join(format!("rusk-io-{}.txt", std::process::id()));
    let path_text = format!("\"{}\"", path.display());
    let src = format!(
        "write \"saved text\" to file {path_text}\nsay read file {path_text}\nwarn read file {path_text}\n"
    );
    let (stdout, stderr, code) = run_with("files", &src, &[], None, &[]);
    let _ = std::fs::remove_file(&path);
    assert_eq!(stdout, "saved text\n");
    assert_eq!(stderr, "saved text\n");
    assert_eq!(code, 0);
}

#[test]
fn warn_goes_to_standard_error() {
    let src = "say \"normal\"\nwarn \"careful\"\nwarn 42\nwarn true\n";
    let (stdout, stderr, code) = run_full("warn", src);
    assert_eq!(stdout, "normal\n");
    assert_eq!(stderr, "careful\n42\ntrue\n");
    assert_eq!(code, 0);
}

#[test]
fn stop_with_sets_the_exit_code_and_stops_the_program() {
    let src = "say \"before\"\nstop with 7\nsay \"after\"\n";
    let (stdout, stderr, code) = run_full("stop_with", src);
    assert_eq!(stdout, "before\n");
    assert_eq!(stderr, "");
    assert_eq!(code, 7);
}

#[test]
fn an_unreadable_file_stops_with_a_sentence() {
    let src = "say read file \"/definitely/not/here\"\n";
    let (stdout, stderr, code) = run_full("missing_file", src);
    assert_eq!(stdout, "");
    assert_eq!(code, 1);
    assert!(stderr.contains("I can't open that file"), "{stderr}");
}

#[test]
fn function_values_and_closures_run() {
    let src = "\
to double with n as a number:
    give back n * 2

let f be the function double
say call f with 21
say f with 4

let base be 10
let add_base be a function with n as a number:
    give back n + base

say call add_base with 5

to make_adder with n as a number:
    let add be a function with x as a number:
        give back x + n
    give back call add with 5

say make_adder with 100
";
    let (stdout, code) = run("closures", src);
    assert_eq!(stdout, "42\n8\n15\n105\n");
    assert_eq!(code, 0);
}

#[test]
fn borrowed_views_share_a_list_without_moving_it() {
    let src = "\
to total with xs as a view of a list of numbers:
    let sum be 0
    for each x in xs:
        set sum to sum + x
    give back sum

let loaves be [3, 6, 9]
say total with loaves
say length of loaves

let shared be a view of loaves
say item 2 of shared
";
    let (stdout, code) = run("views", src);
    assert_eq!(stdout, "18\n3\n6\n");
    assert_eq!(code, 0);
}

#[test]
fn a_view_cannot_be_changed() {
    let src = "\
let xs be [1, 2]
let shared be a view of xs
add 3 to shared
";
    let err = bake_src(src).unwrap_err();
    assert!(err.contains("view"), "{err}");
}

#[test]
fn closures_capture_lists_and_nest() {
    let src = "\
to make with xs as a view of a list of numbers:
    let outer be a function with a as a number:
        let inner be a function with b as a number:
            give back a + b + item 1 of xs
        give back call inner with 2
    give back call outer with 1

let xs be [3]
say make with xs
";
    let (stdout, code) = run("nested_closures", src);
    assert_eq!(stdout, "6\n");
    assert_eq!(code, 0);
}
