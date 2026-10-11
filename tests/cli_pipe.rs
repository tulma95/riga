use std::process::{Command, Stdio};
#[test]
fn complete_exits_quietly_when_stdout_closes() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_riga"))
        .args(["complete", "--cwd", "/tmp", "git "])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
