use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn wizard_cli_starts_and_exits_with_scripted_stdin() {
    let mut child = Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"))
        .arg("wizard")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn greentic-pack wizard");

    {
        let stdin = child.stdin.as_mut().expect("stdin");
        stdin.write_all(b"0\n").expect("write stdin");
    }

    let output = child.wait_with_output().expect("wait for wizard");
    assert!(output.status.success(), "wizard should exit with code 0");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Main Menu"),
        "wizard should render main menu"
    );
    assert!(
        stdout.contains("0) Exit"),
        "wizard should render exit action"
    );
}
