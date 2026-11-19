//! Command-line glue: argument forwarding and the plain TUI fallback.

use std::process::{Command, Stdio};

fn rusk() -> &'static str {
    env!("CARGO_BIN_EXE_rusk")
}

#[cfg(unix)]
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

#[cfg(unix)]
#[test]
fn no_arguments_runs_the_nearby_ruskfile() {
    let dir = std::env::temp_dir().join(format!("rusk-cli-ruskfile-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("Ruskfile"), "say \"from Ruskfile\"\n").unwrap();

    let out = Command::new(rusk())
        .current_dir(&dir)
        .output()
        .expect("rusk should start");
    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "from Ruskfile\n");
}

#[cfg(unix)]
#[test]
fn no_arguments_finds_a_ruskfile_in_a_parent_directory() {
    let dir = std::env::temp_dir().join(format!("rusk-cli-ruskfile-parent-{}", std::process::id()));
    let nested = dir.join("src").join("deep");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(dir.join("Ruskfile"), "say \"from above\"\n").unwrap();

    let out = Command::new(rusk())
        .current_dir(&nested)
        .output()
        .expect("rusk should start");
    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "from above\n");
}

#[cfg(unix)]
#[test]
fn no_arguments_without_a_ruskfile_starts_the_session() {
    use std::io::Write;

    let dir = std::env::temp_dir().join(format!("rusk-cli-no-ruskfile-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let mut child = Command::new(rusk())
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("rusk should start");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"say \"session\"\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    let _ = std::fs::remove_dir_all(&dir);

    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), "session\n");
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

#[test]
fn build_can_cross_bake_x86_64() {
    let dir = std::env::temp_dir().join(format!("rusk-cli-x86-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("hello.rk");
    let out_path = dir.join("hello");
    std::fs::write(&file, "say \"hello\"\n").unwrap();

    let out = Command::new(rusk())
        .arg("build")
        .arg(&file)
        .arg("-o")
        .arg(&out_path)
        .args(["--target", "x86_64"])
        .output()
        .expect("rusk should start");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let bytes = std::fs::read(&out_path).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&dir);

    assert!(out.status.success(), "{stdout}");
    assert!(stdout.contains("native x86_64"), "{stdout}");
    assert_eq!(&bytes[0..4], b"\x7fELF");
    assert_eq!(u16::from_le_bytes(bytes[18..20].try_into().unwrap()), 62);
    assert_eq!(
        u64::from_le_bytes(bytes[24..32].try_into().unwrap()),
        0x40_0078
    );
}

#[test]
fn check_reports_the_selected_target() {
    let dir = std::env::temp_dir().join(format!("rusk-cli-check-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("hello.rk");
    std::fs::write(&file, "say \"hello\"\n").unwrap();

    let out = Command::new(rusk())
        .arg("check")
        .arg(&file)
        .args(["--target", "x86_64"])
        .output()
        .expect("rusk should start");
    let _ = std::fs::remove_dir_all(&dir);

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    assert!(stdout.contains("native x86_64"), "{stdout}");
}

#[test]
fn toast_defaults_to_both_linux_targets() {
    let dir = std::env::temp_dir().join(format!("rusk-cli-toast-default-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("hello.rk");
    let toast = dir.join("hello.tst");
    std::fs::write(&file, "say \"hello\"\n").unwrap();

    let out = Command::new(rusk())
        .arg("toast")
        .arg(&file)
        .arg("-o")
        .arg(&toast)
        .output()
        .expect("rusk should start");
    let stdout = String::from_utf8_lossy(&out.stdout);

    let check = Command::new(rusk())
        .arg("check")
        .arg(&toast)
        .output()
        .expect("rusk should start");
    let checked = String::from_utf8_lossy(&check.stdout);
    let bytes = std::fs::read(&toast).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&dir);

    assert!(out.status.success(), "{stdout}");
    assert!(stdout.contains("aarch64, x86_64"), "{stdout}");
    assert!(check.status.success(), "{checked}");
    assert!(checked.contains("aarch64, x86_64"), "{checked}");
    assert_eq!(&bytes[..8], b"RTOAST1\n");
}

#[cfg(unix)]
#[test]
fn toast_round_trips_and_runs_on_the_host() {
    let dir = std::env::temp_dir().join(format!("rusk-cli-toast-run-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("hello.rk");
    let toast = dir.join("hello.tst");
    let built = dir.join("hello-from-toast");
    std::fs::write(&file, "say \"toasted\"\nsay argument 1\n").unwrap();

    let toast_out = Command::new(rusk())
        .arg("toast")
        .arg(&file)
        .arg("-o")
        .arg(&toast)
        .output()
        .expect("rusk should start");
    let run_out = Command::new(rusk())
        .arg("run")
        .arg(&toast)
        .arg("alpha")
        .output()
        .expect("rusk should start");
    let build_out = Command::new(rusk())
        .arg("build")
        .arg(&toast)
        .arg("-o")
        .arg(&built)
        .output()
        .expect("rusk should start");
    let built_out = Command::new(&built)
        .output()
        .expect("the wrapped executable should start");
    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        toast_out.status.success(),
        "{}",
        String::from_utf8_lossy(&toast_out.stderr)
    );
    assert!(
        run_out.status.success(),
        "{}",
        String::from_utf8_lossy(&run_out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run_out.stdout), "toasted\nalpha\n");
    assert!(
        build_out.status.success(),
        "{}",
        String::from_utf8_lossy(&build_out.stderr)
    );
    assert!(built_out.status.success());
    assert_eq!(String::from_utf8_lossy(&built_out.stdout), "toasted\n\n");
}

#[test]
fn a_toast_can_hold_an_experimental_windows_image() {
    let dir = std::env::temp_dir().join(format!("rusk-cli-toast-pe-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("hello.rk");
    let toast = dir.join("hello-win.tst");
    let exe = dir.join("hello.exe");
    std::fs::write(&file, "say \"hello from windows\"\n").unwrap();

    let toast_out = Command::new(rusk())
        .arg("toast")
        .arg(&file)
        .arg("-o")
        .arg(&toast)
        .args(["--target", "windows"])
        .output()
        .expect("rusk should start");
    let toast_stdout = String::from_utf8_lossy(&toast_out.stdout);
    assert!(toast_out.status.success(), "{toast_stdout}");

    let build_out = Command::new(rusk())
        .arg("build")
        .arg(&toast)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("rusk should start");
    let build_stdout = String::from_utf8_lossy(&build_out.stdout);
    let bytes = std::fs::read(&exe).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&dir);

    assert!(build_out.status.success(), "{build_stdout}");
    assert_eq!(&bytes[0..2], b"MZ");
    let pe = u32::from_le_bytes(bytes[0x3C..0x40].try_into().unwrap()) as usize;
    assert_eq!(&bytes[pe..pe + 4], b"PE\0\0");
    assert_eq!(
        u16::from_le_bytes(bytes[pe + 4..pe + 6].try_into().unwrap()),
        0x8664
    );
    let optional = pe + 24;
    assert_eq!(
        u16::from_le_bytes(bytes[optional..optional + 2].try_into().unwrap()),
        0x20B
    );
    assert_eq!(
        u16::from_le_bytes(bytes[optional + 68..optional + 70].try_into().unwrap()),
        3
    );
    for needle in [
        b"kernel32.dll".as_slice(),
        b"GetStdHandle",
        b"GetCommandLineA",
        b"GetEnvironmentStringsA",
        b"WriteFile",
        b"ExitProcess",
    ] {
        assert!(
            bytes.windows(needle.len()).any(|window| window == needle),
            "missing {}",
            String::from_utf8_lossy(needle)
        );
    }
}

#[test]
fn run_refuses_a_windows_target_before_baking() {
    let dir = std::env::temp_dir().join(format!("rusk-cli-run-win-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("hello.rk");
    std::fs::write(&file, "say \"hello\"\n").unwrap();

    let out = Command::new(rusk())
        .arg("run")
        .args(["--target", "windows"])
        .arg(&file)
        .output()
        .expect("rusk should start");
    let _ = std::fs::remove_dir_all(&dir);

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(stderr.contains("can't be run directly"), "{stderr}");
}

#[test]
fn an_unknown_target_is_rejected() {
    let out = Command::new(rusk())
        .args(["check", "missing.rk", "--target", "sparc"])
        .output()
        .expect("rusk should start");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(
        stderr.contains("I don't know the target 'sparc'"),
        "{stderr}"
    );
}

#[test]
fn rdb_steps_and_prints_names_over_a_pipe() {
    use std::io::Write;

    let dir = std::env::temp_dir().join(format!("rusk-cli-rdb-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("debug.rk");
    std::fs::write(&file, "let x be 1\nsay x\n").unwrap();

    let mut child = Command::new(rusk())
        .arg("rdb")
        .arg(&file)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("rusk should start");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"step\nprint x\nquit\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    let _ = std::fs::remove_dir_all(&dir);

    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("2 top-level statements"), "{stdout}");
    assert!(stdout.contains("stopped at line 2"), "{stdout}");
    assert!(stdout.contains("x = 1"), "{stdout}");
}

#[test]
fn errors_get_fixed_category_codes_over_a_pipe_without_a_question() {
    use std::io::Write;

    let mut child = Command::new(rusk())
        .arg("repl")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("rusk should start");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"say missing_name\nsay \"oops\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();

    assert!(out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("E02"),
        "unknown name should be a type error: {stderr}"
    );
    assert!(
        stderr.contains("E01"),
        "an unclosed quote should be a syntax error: {stderr}"
    );
    assert!(stderr.contains("line 1"), "{stderr}");
    assert!(stderr.contains("hint:"), "{stderr}");
    assert!(!stderr.contains("Do you want to edit"), "{stderr}");
}

#[test]
fn logo_uses_the_platform_asset() {
    let out = Command::new(rusk())
        .arg("logo")
        .output()
        .expect("rusk should start");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    if cfg!(windows) {
        assert!(stdout.starts_with("<svg"), "{stdout}");
    } else {
        assert!(stdout.contains("█"), "{stdout}");
    }
}

#[test]
fn the_repl_can_start_rdb_with_a_colon_command() {
    use std::io::Write;

    let mut child = Command::new(rusk())
        .arg("repl")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("rusk should start");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"let x be 1\n:rdb\nstep\nprint x\nquit\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();

    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("rdb — Rusk debugger"), "{stdout}");
    assert!(stdout.contains("x = 1"), "{stdout}");
}

#[test]
fn toastset_documents_the_optional_windows_startup_shortcut() {
    let installer = include_str!("../toastset");
    assert!(installer.contains("--no-startup"));
    assert!(installer.contains("TOASTSET_NO_STARTUP"));
    assert!(
        installer.contains("Add Rusk to the Windows Startup folder so the prompt opens at login?")
    );
    assert!(installer.contains("Rusk.lnk"));
}

#[test]
fn toastset_has_a_native_windows_installer() {
    let shell = include_str!("../toastset");
    let native = include_str!("../toastset.ps1");
    assert!(shell.contains("delegate_to_windows_installer"));
    assert!(shell.contains("toastset.ps1"));
    assert!(shell.contains("windows_path \"$ps1\""));
    assert!(!native.contains("[CmdletBinding()]"));
    assert!(native.contains("$RawArgs = $args"));
    assert!(native.contains("[Net.SecurityProtocolType]::Tls12"));
    assert!(native.contains("releases/latest"));
    assert!(native.contains("rusk-$Tag-$Target.tar.gz"));
    assert!(native.contains("Join-Path $env:LOCALAPPDATA 'Rusk'"));
    assert!(native.contains("curl.exe -fsSL"));
    assert!(native.contains("tar.exe -xzf"));
    assert!(native.contains("--no-startup"));
    assert!(native.contains("Add Rusk to the Windows Startup folder so the prompt opens at login?"));
    assert!(native.contains("Rusk.lnk"));
    assert!(native.contains("& $Exe version"));
    let release = include_str!("../.github/workflows/release.yml");
    assert!(release.contains("cp toastset \"dist/${name}/toastset\""));
    assert!(release.contains("cp toastset.ps1 \"dist/${name}/toastset.ps1\""));
    assert!(release.contains("Copy-Item \"toastset.ps1\" \"dist/$name/toastset.ps1\""));
    assert!(release.contains("cp -R rusklib \"dist/${name}/rusklib\""));
    assert!(release.contains("Copy-Item -Recurse \"rusklib\" \"dist/$name/rusklib\""));
    let wheat = include_str!("../rusklib/wheat/README.md");
    assert!(wheat.contains("wheat — a small terminal UI library in Rusk"));
    let pointer = include_str!("../wheat/README.md");
    assert!(pointer.contains("rusklib/wheat"));
    assert!(pointer.contains("wheat_box_around"));
}

fn gui_source() -> &'static str {
    concat!(
        "let w be make simple window titled \"Hello\" wide 320 high 200\n",
        "let b be add simple button to w labelled \"Press\" at 10 and 10 sized 100 and 30\n",
        "show simple window w\n",
        "pump simple events\n",
        "let e be next simple event\n",
        "say e\n",
        "say simple event control\n",
        "set simple text of b to \"Thanks\"\n",
        "say simple text of b\n",
        "show simple message \"Done\"\n",
        "say simple confirm with \"OK?\"\n",
        "close simple window\n",
    )
}

#[test]
fn simple_gui_syntax_checks_on_the_host_target() {
    let dir = std::env::temp_dir().join(format!("rusk-cli-gui-check-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("gui.rk");
    std::fs::write(&file, gui_source()).unwrap();

    let out = Command::new(rusk())
        .arg("check")
        .arg(&file)
        .output()
        .expect("rusk should start");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let _ = std::fs::remove_dir_all(&dir);

    assert!(out.status.success(), "{stdout}{stderr}");
    assert!(stdout.contains("looks good"), "{stdout}");
}

#[test]
fn a_windows_bake_of_simple_gui_links_user32_at_runtime() {
    let dir = std::env::temp_dir().join(format!("rusk-cli-gui-pe-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("gui.rk");
    let exe = dir.join("gui.exe");
    std::fs::write(&file, gui_source()).unwrap();

    let out = Command::new(rusk())
        .arg("build")
        .arg(&file)
        .args(["--target", "windows"])
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("rusk should start");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let bytes = std::fs::read(&exe).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&dir);

    assert!(out.status.success(), "{stdout}{stderr}");
    assert!(stdout.contains("windows-x86_64"), "{stdout}");
    assert_eq!(&bytes[0..2], b"MZ");
    let pe = u32::from_le_bytes(bytes[0x3C..0x40].try_into().unwrap()) as usize;
    assert_eq!(&bytes[pe..pe + 4], b"PE\0\0");
    for needle in [
        b"LoadLibraryA".as_slice(),
        b"GetProcAddress",
        b"GetModuleHandleA",
        b"user32.dll",
        b"CreateWindowExA",
        b"SimpleWindowClass",
    ] {
        assert!(
            bytes.windows(needle.len()).any(|window| window == needle),
            "missing {}",
            String::from_utf8_lossy(needle)
        );
    }
}

#[test]
fn fastlib_imports_resolve_from_a_nested_program() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir = root
        .join("target")
        .join(format!("rusk-cli-fastlib-import-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("gui.rk");
    std::fs::write(
        &file,
        "import \"simple/event\"\nlet e be simple_event_wait\nsay e\n",
    )
    .unwrap();

    let out = Command::new(rusk())
        .arg("check")
        .arg(&file)
        .output()
        .expect("rusk should start");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let _ = std::fs::remove_dir_all(&dir);

    assert!(out.status.success(), "{stdout}{stderr}");
    assert!(stdout.contains("looks good"), "{stdout}");
}

#[test]
fn every_fastlib_simple_module_checks() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let folder = root.join("fastlib").join("simple");
    let mut files: Vec<_> = std::fs::read_dir(&folder)
        .unwrap()
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "rk"))
        .collect();
    files.sort();
    assert!(
        files.len() >= 30,
        "expected the simple bundle, got {}",
        files.len()
    );

    for file in files {
        let out = Command::new(rusk())
            .arg("check")
            .arg(&file)
            .output()
            .expect("rusk should start");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            out.status.success(),
            "{} failed: {stdout}{stderr}",
            file.display()
        );
    }
}
