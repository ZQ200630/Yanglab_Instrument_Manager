use std::io::Write;
use std::process::{Command, Stdio};
#[test]
fn active_backend_tests_and_builds_need_no_python() {
    let result = Command::new(env!("CARGO_BIN_EXE_yang-debug"))
        .env("PATH", "")
        .output()
        .unwrap();
    assert!(result.status.success());
    assert!(String::from_utf8(result.stdout)
        .unwrap()
        .contains("No resources opened"));
    let mut child = Command::new(env!("CARGO_BIN_EXE_worker-fixture"))
        .args(["--real", "--protocol", "2", "--fixture-root"])
        .arg(std::env::temp_dir())
        .env("PATH", "")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"{\"v\":2,\"id\":\"stop\",\"method\":\"shutdown\",\"params\":{}}\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    let reply: serde_json::Value = serde_json::from_str(text.lines().last().unwrap()).unwrap();
    assert_eq!(reply["ok"], true);
    assert_eq!(reply["result"]["unreleased"], serde_json::json!([]));
    assert!(!text.contains("python"));
}
