//! Command-line glue: argument forwarding and the plain TUI fallback.

use std::process::{Command, Stdio};

fn rusk() -> &'static str {
    env!("CARGO_BIN_EXE_rusk")
}

#[test]
fn run_forwards_arguments_after_the_file() {
    let dir = std::env::temp_dir().join(format!("rusk-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("args.rk");
    std::fs::write(&file, "say number of arguments\nsay argument 1\n").unwrap();

    let out = Command::new(rusk())
        .arg("run")
        .arg(&file)
        .arg("alpha")
        .output()
        .expect("rusk should start");

    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "1\nalpha\n");
}

#[test]
fn tui_demo_falls_back_to_text_when_stdout_is_not_a_terminal() {
    let out = Command::new(rusk())
        .args(["tui", "demo"])
        .stdin(Stdio::null())
        .output()
        .expect("rusk should start");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success());
    assert!(stdout.contains("░█▀▄░█░█░█▀▀░█░█"), "{stdout}");
    assert!(stdout.contains("This is not a terminal"), "{stdout}");
    assert!(!stdout.contains("\x1b["), "{stdout}");
}

#[test]
fn tui_runs_the_line_session_over_a_pipe() {
    use std::io::Write;

    let mut child = Command::new(rusk())
        .arg("tui")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("rusk should start");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"say \"through a pipe\"\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success());
    assert_eq!(stdout, "through a pipe\n");
}

#[test]
fn tui_stop_with_sets_the_exit_status_over_a_pipe() {
    use std::io::Write;

    let mut child = Command::new(rusk())
        .arg("tui")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("rusk should start");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"say \"before\"\nstop with 7\nsay \"after\"\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();

    assert_eq!(out.status.code(), Some(7));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "before\n");
    assert!(!String::from_utf8_lossy(&out.stderr).contains("burnt"));
}
