#[path = "support/native.rs"]
mod support;
use serde_json::json;
use std::{
    fs::File,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use yang_protocol::Phase;
use yang_worker::{
    captures::{CaptureSpool, Durability},
    scheduler::Backend,
};
struct Disk(AtomicBool);
impl Durability for Disk {
    fn sync(&self, f: &File) -> std::io::Result<()> {
        if self.0.load(Ordering::SeqCst) {
            Err(std::io::Error::other("disk full"))
        } else {
            f.sync_all()
        }
    }
}
#[test]
fn staging_recovery_preserves_exact_completed_read_and_disconnect_cannot_discard_it() {
    let fixture = support::Fixture::new();
    let disk = Arc::new(Disk(AtomicBool::new(true)));
    let spool = Arc::new(Mutex::new(
        CaptureSpool::with_durability(fixture.root.clone(), fixture.nonce.clone(), disk.clone())
            .unwrap(),
    ));
    let (b, f) = support::backend();
    b.install_spool(spool.clone());
    let ctx = support::connected(&b);
    let read = b.execute(&support::request(
        "original-read",
        "action",
        json!({"name":"read_trace","args":{}}),
        Some(ctx.clone()),
    ));
    assert_eq!(read.error.unwrap().kind, "CaptureStagingFailed");
    assert_eq!(f.reads.load(Ordering::SeqCst), 1);
    let blocked = b.execute(&support::request(
        "duplicate-read",
        "action",
        json!({"name":"read_trace","args":{}}),
        Some(ctx.clone()),
    ));
    assert_eq!(blocked.error.unwrap().kind, "CaptureUnstaged");
    let status = b.observe(&ctx).status;
    let ticket = status["unstaged_capture"]["capture_id"].clone();
    assert!(
        ticket.as_str().is_some_and(|s| s.len() == 32),
        "no recovery ticket: {status}"
    );
    let close = b.execute(&support::request(
        "close",
        "disconnect",
        json!({}),
        Some(ctx.clone()),
    ));
    assert_eq!(
        close.result.as_ref().unwrap()["data_retained"],
        true,
        "disconnect discarded raw spectrum"
    );
    assert_eq!(
        f.open.load(Ordering::SeqCst),
        0,
        "hardware resources must still close"
    );
    for n in 0..40 {
        let failed = b.execute(&support::request(
            &format!("storage-failed-{n}"),
            "action",
            json!({"name":"retry_staging","args":{"capture_id":ticket}}),
            Some(ctx.clone()),
        ));
        assert_eq!(failed.error.unwrap().kind, "CaptureStagingFailed");
    }
    assert_eq!(
        std::fs::read_dir(&fixture.root).unwrap().count(),
        1,
        "retries must not consume new capture slots"
    );
    let wrong = b.execute(&support::request(
        "wrong",
        "action",
        json!({"name":"retry_staging","args":{"capture_id":"0".repeat(32)}}),
        Some(ctx.clone()),
    ));
    assert_ne!(wrong.phase, Phase::Completed);
    disk.0.store(false, Ordering::SeqCst);
    let restored = b.execute(&support::request(
        "storage-only",
        "action",
        json!({"name":"retry_staging","args":{"capture_id":ticket}}),
        Some(ctx.clone()),
    ));
    assert_eq!(restored.phase, Phase::Completed, "{:?}", restored.error);
    let value = restored.result.unwrap();
    assert_eq!(value["original_request_id"], "original-read");
    assert_eq!(value["source_context"], serde_json::to_value(&ctx).unwrap());
    assert_eq!(value["timing"]["io_s"], 0.9);
    assert_eq!(f.reads.load(Ordering::SeqCst), 1);
    let d: yang_protocol::CaptureDescriptor =
        serde_json::from_value(value["capture"].clone()).unwrap();
    let bytes = spool
        .lock()
        .unwrap()
        .read_chunk(&fixture.nonce, &d.capture_id, 0, 32)
        .unwrap();
    let exact: Vec<u8> = [1549f64, -40f64, 1550f64, -30f64]
        .into_iter()
        .flat_map(f64::to_le_bytes)
        .collect();
    assert_eq!(bytes, exact);
    let close = b.execute(&support::request(
        "close-after-storage",
        "disconnect",
        json!({}),
        Some(ctx),
    ));
    assert_eq!(close.result.unwrap()["connected"], false);
}
#[test]
fn eof_retains_raw_until_storage_recovers_without_repeating_read() {
    let fixture = support::Fixture::new();
    let disk = Arc::new(Disk(AtomicBool::new(true)));
    let (b, f) = support::backend();
    let scheduler = yang_worker::scheduler::Scheduler::new(
        b.clone(),
        Arc::new(yang_drivers::clock::SystemClock::default()),
        yang_protocol::Limits::default(),
    )
    .unwrap();
    let spool =
        CaptureSpool::with_durability(fixture.root.clone(), fixture.nonce.clone(), disk.clone())
            .unwrap();
    let w = yang_worker::dispatch::Worker::new(b.clone(), scheduler, spool);
    let ctx = support::connected(&b);
    b.execute(&support::request(
        "read",
        "action",
        json!({"name":"read_trace","args":{}}),
        Some(ctx),
    ));
    let first = w
        .run_io(
            std::io::Cursor::new(Vec::<u8>::new()),
            support::Output::default(),
        )
        .unwrap();
    assert!(!first.all_resources_released);
    assert_eq!(f.open.load(Ordering::SeqCst), 0);
    disk.0.store(false, Ordering::SeqCst);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while !w.retry_shutdown().all_resources_released {
        assert!(
            std::time::Instant::now() < deadline,
            "raw capture prevented recoverable shutdown"
        );
        std::thread::yield_now();
    }
    assert_eq!(f.reads.load(Ordering::SeqCst), 1);
    assert_eq!(
        std::fs::read_dir(&fixture.root).unwrap().count(),
        2,
        "native bytes and manifest must remain on disk, not be implicitly ACKed/deleted"
    );
}
