//! End-to-end tests of the built binary.

use std::io::Write;
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_nitop");

#[test]
fn version_prints_the_package_version() {
    let out = Command::new(BIN).arg("--version").output().unwrap();
    assert!(out.status.success());
    let want = format!("nitop {}\n", env!("CARGO_PKG_VERSION"));
    assert_eq!(String::from_utf8_lossy(&out.stdout), want);
}

#[test]
fn help_documents_every_key() {
    let out = Command::new(BIN).arg("--help").output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    for want in [
        "Usage: nitop",
        "Quit",
        "bytes or bits",
        "sampling",
        "Scroll",
    ] {
        assert!(text.contains(want), "missing {want:?} in:\n{text}");
    }
}

#[test]
fn unknown_argument_fails_with_usage_status() {
    let out = Command::new(BIN).arg("--nope").output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("--nope"));
}

#[test]
fn refuses_to_start_without_a_terminal() {
    let out = Command::new(BIN).stdin(Stdio::null()).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("must be run in a terminal"));
}

/// Runs the real program in a pseudo-terminal, using util-linux `script`, and quits it
/// with a key press. Checks it draws, exits cleanly and restores the terminal.
#[test]
fn runs_draws_and_quits_in_a_terminal() {
    if Command::new("script").arg("--version").output().is_err() {
        eprintln!("skipped: `script` is not installed");
        return;
    }
    let inner = format!("stty cols 120 rows 40; exec '{BIN}'");
    let mut child = Command::new("script")
        .args(["-qec", &inner, "/dev/null"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    sleep(Duration::from_millis(1500));
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(b"q").unwrap();
    stdin.flush().unwrap();

    let deadline = Instant::now() + Duration::from_secs(10);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() > deadline {
            child.kill().unwrap();
            panic!("nitop did not quit on 'q'");
        }
        sleep(Duration::from_millis(50));
    }
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "exit status {:?}", out.status);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("all traffic"), "no UI drawn: {text:?}");
    assert!(
        text.contains("\x1b[?1049h"),
        "never entered the alternate screen"
    );
    assert!(
        text.contains("\x1b[?1049l"),
        "did not restore the screen on exit"
    );
}
