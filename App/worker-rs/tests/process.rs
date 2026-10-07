#[path = "support/native.rs"]
mod support;
use serde_json::json;
use std::{
    sync::{atomic::Ordering, Arc, Condvar, Mutex},
    time::Duration,
};
use support::*;
#[test]
fn startup_is_native_disarmed_and_empty() {
    let f = Fixture::new();
    let (b, factory) = backend();
    let w = worker(b, &f);
    let out = Output::default();
    let r = w
        .run_io(
            frames(&[request("ping", "ping", json!({}), None)]),
            out.clone(),
        )
        .unwrap();
    let replies = replies(&out);
    let result = &replies[0]["result"];
    assert_eq!(result["worker_kind"], "rust");
    assert_eq!(result["startup_revision"], 1);
    assert_eq!(result["activated"], false);
    assert_eq!(result["connected"], false);
    assert_eq!(result["domains"], json!([]));
    assert_eq!(factory.creates.load(Ordering::SeqCst), 0);
    assert!(r.all_resources_released);
}
#[test]
fn activation_nonce_is_one_use() {
    let f = Fixture::new();
    let (b, factory) = backend();
    let context = b.global_context();
    let w = worker(b, &f);
    let out = Output::default();
    w.run_io(
        frames(&[
            request(
                "bad",
                "activate",
                json!({"ownership_nonce":"f".repeat(32)}),
                Some(context.clone()),
            ),
            request(
                "first",
                "activate",
                json!({"ownership_nonce":f.nonce}),
                Some(context.clone()),
            ),
            request(
                "second",
                "activate",
                json!({"ownership_nonce":f.nonce}),
                Some(context),
            ),
        ]),
        out.clone(),
    )
    .unwrap();
    let replies = replies(&out);
    assert_eq!(replies.len(), 3);
    assert_eq!(replies[0]["phase"], "rejected_before_call");
    assert_eq!(replies[1]["phase"], "completed");
    assert_eq!(replies[2]["phase"], "rejected_before_call");
    assert_eq!(factory.creates.load(Ordering::SeqCst), 0);
}
#[test]
fn stdout_is_protocol_only() {
    let f = Fixture::new();
    let (b, _) = backend();
    let context = b.global_context();
    let w = worker(b, &f);
    let out = Output::default();
    w.run_io(
        frames(&[
            request("ping", "ping", json!({}), None),
            request("status", "status", json!({}), Some(context)),
        ]),
        out.clone(),
    )
    .unwrap();
    let replies = replies(&out);
    assert_eq!(replies.len(), 2);
    assert!(replies.iter().all(|r| r["v"] == 3 && r["id"].is_string()));
}
#[test]
fn eof_reports_unfinished_cleanup() {
    let f = Fixture::new();
    let (b, factory) = backend();
    connected(&b);
    factory.failed_closes.store(2, Ordering::SeqCst);
    let w = worker(b.clone(), &f);
    let receipt = w.run_io(frames(&[]), Output::default()).unwrap();
    assert!(!receipt.all_resources_released);
    assert!(!receipt.reports.is_empty());
    assert!(factory.open.load(Ordering::SeqCst) > 0);
}
struct HeldWriter {
    gate: Arc<(Mutex<(bool, bool)>, Condvar)>,
}
impl std::io::Write for HeldWriter {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        let mut state = self.gate.0.lock().unwrap();
        state.0 = true;
        self.gate.1.notify_all();
        state = self.gate.1.wait_while(state, |s| !s.1).unwrap();
        drop(state);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
#[test]
fn blocked_stdout_keeps_accepted_responsibility() {
    let f = Fixture::new();
    let (b, _) = backend();
    let w = worker(b, &f);
    let gate = Arc::new((Mutex::new((false, false)), Condvar::new()));
    let receipt = w
        .run_io(
            frames(&[request("ping", "ping", json!({}), None)]),
            HeldWriter { gate: gate.clone() },
        )
        .unwrap();
    let state = gate.0.lock().unwrap();
    let (mut state, _) = gate
        .1
        .wait_timeout_while(state, Duration::from_secs(2), |s| !s.0)
        .unwrap();
    let entered = state.0;
    state.1 = true;
    gate.1.notify_all();
    drop(state);
    assert!(entered);
    assert!(!receipt.all_resources_released);
    assert_eq!(receipt.pending_replies, 1);
}

#[test]
fn shutdown_reply_requires_cleanup_completion() {
    let f = Fixture::new();
    let (b, factory) = backend();
    connected(&b);
    factory.failed_closes.store(1, Ordering::SeqCst);
    let context = b.global_context();
    let w = worker(b, &f);
    let out = Output::default();
    let receipt = w
        .run_io(
            frames(&[
                request(
                    "activate",
                    "activate",
                    json!({"ownership_nonce":f.nonce}),
                    Some(context.clone()),
                ),
                request("shutdown", "shutdown", json!({}), Some(context)),
            ]),
            out.clone(),
        )
        .unwrap();
    let shutdown = replies(&out)
        .into_iter()
        .find(|v| v["id"] == "shutdown")
        .unwrap();
    assert_eq!(shutdown["phase"], "failed_after_call_started");
    assert!(!receipt.all_resources_released);
}

#[test]
fn wrong_session_shutdown_does_not_stop_admission() {
    let f = Fixture::new();
    let (b, _) = backend();
    let context = b.global_context();
    let mut wrong = context.clone();
    wrong.session_id = "f".repeat(32);
    let w = worker(b, &f);
    let out = Output::default();
    w.run_io(
        frames(&[
            request(
                "activate",
                "activate",
                json!({"ownership_nonce":f.nonce}),
                Some(context),
            ),
            request("wrong", "shutdown", json!({}), Some(wrong)),
            request("ping", "ping", json!({}), None),
        ]),
        out.clone(),
    )
    .unwrap();
    let responses = replies(&out);
    assert_eq!(responses.len(), 3);
    assert_eq!(responses[1]["phase"], "rejected_before_call");
    assert_eq!(responses[2]["phase"], "completed");
}

#[test]
fn native_idle_eof_exits_and_rejects_legacy_or_fixture_arguments() {
    use std::process::{Command, Stdio};
    let f = Fixture::new();
    let output = Command::new(env!("CARGO_BIN_EXE_yang-worker"))
        .args([
            "--real",
            "--protocol",
            "3",
            "--ownership-nonce",
            &f.nonce,
            "--capture-spool",
        ])
        .arg(&f.root)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    assert_eq!(std::fs::read_dir(&f.root).unwrap().count(), 0);
    for option in [
        "--simulate",
        "--python",
        "--fixture",
        "--raw-scpi",
        "--settings",
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_yang-worker"))
            .arg(option)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
    }
}
