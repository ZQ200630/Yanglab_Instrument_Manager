use std::{
    fs::{self, File},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, SystemTime},
};
use yang_drivers::osa::*;
use yang_worker::captures::{CaptureSpool, Durability};

struct Fixture {
    parent: PathBuf,
    root: PathBuf,
    nonce: String,
}
impl Fixture {
    fn new() -> Self {
        let nonce = yang_worker::new_id().unwrap();
        let parent =
            std::env::temp_dir().join(format!("native-spool-{}", yang_worker::new_id().unwrap()));
        let root = parent.join(&nonce);
        fs::create_dir_all(&root).unwrap();
        Self {
            parent,
            root,
            nonce,
        }
    }
    fn open(&self) -> CaptureSpool {
        CaptureSpool::open(self.root.clone(), self.nonce.clone()).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.parent);
    }
}
fn capture(count: usize, identity: &str) -> TraceCapture {
    let context = TraceContext::new(TraceContextParams {
        transfer_format: TransferFormat::Ascii,
        sample_count: count,
        spacing: 0,
        level_unit: 0,
        x_unit: 0,
        trace_attribute: 0,
        active_trace: TraceId::A,
        center_m: 1.55e-6,
        span_m: 2e-9,
        resolution_m: 2e-11,
        sweep_mode: 1,
    })
    .unwrap();
    TraceCapture::new(
        (0..count).map(|i| 1549. + i as f64).collect(),
        (0..count)
            .map(|i| if i == 0 { -210. } else { -69.3149081 })
            .collect(),
        NativeUnit::Dbm,
        TraceId::A,
        identity.into(),
        ReadTiming {
            started_utc: SystemTime::UNIX_EPOCH + Duration::from_secs(1791280800),
            finished_utc: SystemTime::UNIX_EPOCH + Duration::from_secs(1791280801),
            elapsed: Duration::from_secs(1),
            decode: Duration::from_millis(1),
            io: Duration::from_millis(900),
        },
        context.clone(),
        context,
    )
    .unwrap()
}
fn sample() -> TraceCapture {
    capture(2, "YOKOGAWA,AQ6370E,TEST-BYTES,FW")
}
#[test]
fn payload_and_descriptor_match_host_fixture() {
    let fixture = Fixture::new();
    let mut spool = fixture.open();
    let desc = spool.stage(&sample()).unwrap();
    let reference: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/capture/literal.json")).unwrap();
    let payload: Vec<u8> = reference["payload_hex"]
        .as_str()
        .unwrap()
        .as_bytes()
        .chunks_exact(2)
        .map(|c| u8::from_str_radix(std::str::from_utf8(c).unwrap(), 16).unwrap())
        .collect();
    let mut actual = serde_json::to_value(&desc).unwrap();
    actual["capture_id"] = reference["descriptor"]["capture_id"].clone();
    assert_eq!(actual, reference["descriptor"]);
    assert_eq!(
        spool
            .read_chunk(&fixture.nonce, &desc.capture_id, 0, 32)
            .unwrap(),
        payload
    );
    let disk: serde_json::Value = serde_json::from_slice(
        &fs::read(fixture.root.join(format!("{}.json", desc.capture_id))).unwrap(),
    )
    .unwrap();
    assert_eq!(disk, serde_json::to_value(&desc).unwrap());
}
#[test]
fn bounds_and_nonce_are_enforced() {
    let fixture = Fixture::new();
    let mut spool = fixture.open();
    let desc = spool.stage(&sample()).unwrap();
    for (offset, length) in [(0, 0), (0, 16385), (32, 1), (u64::MAX, 2)] {
        assert!(spool
            .read_chunk(&fixture.nonce, &desc.capture_id, offset, length)
            .is_err());
    }
    assert!(spool
        .read_chunk(&"f".repeat(32), &desc.capture_id, 0, 1)
        .is_err());
    assert!(spool
        .ack(&"f".repeat(32), &desc.capture_id, &desc.sha256)
        .is_err());
    assert!(spool
        .read_chunk(&fixture.nonce, "../outside", 0, 1)
        .is_err());
    assert!(CaptureSpool::open(fixture.root.clone(), "f".repeat(32)).is_err());
    for _ in 1..32 {
        spool.stage(&sample()).unwrap();
    }
    assert!(spool.stage(&sample()).is_err());
    assert_eq!(fs::read_dir(&fixture.root).unwrap().count(), 64);
    let large = Fixture::new();
    File::create(large.root.join("orphan"))
        .unwrap()
        .set_len(128 * 1024 * 1024 + 1)
        .unwrap();
    assert!(CaptureSpool::open(large.root.clone(), large.nonce.clone()).is_err());
    let escaped = Fixture::new();
    let d = escaped
        .open()
        .stage(&capture(2, &"\u{0001}".repeat(1024)))
        .unwrap();
    assert!(serde_json::to_vec(&d.metadata).unwrap().len() <= 8192);
}
#[cfg(windows)]
#[test]
fn reparse_swap_cannot_redirect_io() {
    let fixture = Fixture::new();
    let outside = Fixture::new();
    let mut spool = fixture.open();
    let desc = spool.stage(&sample()).unwrap();
    let saved = fixture.parent.join("saved");
    fs::rename(&fixture.root, &saved).unwrap();
    let status = std::process::Command::new("cmd.exe")
        .args(["/c", "mklink", "/J"])
        .arg(&fixture.root)
        .arg(&outside.root)
        .output()
        .unwrap();
    assert!(status.status.success());
    let read = spool.read_chunk(&fixture.nonce, &desc.capture_id, 0, 16);
    let ack = spool.ack(&fixture.nonce, &desc.capture_id, &desc.sha256);
    let stage = spool.stage(&sample());
    fs::remove_dir(&fixture.root).unwrap();
    fs::rename(&saved, &fixture.root).unwrap();
    assert!(read.is_err() && ack.is_err() && stage.is_err());
    assert_eq!(fs::read_dir(&outside.root).unwrap().count(), 0);
    assert!(spool
        .read_chunk(&fixture.nonce, &desc.capture_id, 0, 32)
        .is_ok());
}
struct Disk(AtomicBool);
impl Durability for Disk {
    fn sync(&self, file: &File) -> std::io::Result<()> {
        if self.0.load(Ordering::SeqCst) {
            Err(std::io::Error::other("injected disk commit failure"))
        } else {
            file.sync_all()
        }
    }
}
#[test]
fn disk_failure_keeps_capture_unacked() {
    let fixture = Fixture::new();
    let disk = Arc::new(Disk(AtomicBool::new(true)));
    let mut spool =
        CaptureSpool::with_durability(fixture.root.clone(), fixture.nonce.clone(), disk.clone())
            .unwrap();
    assert!(spool.stage(&sample()).is_err());
    let paths: Vec<_> = fs::read_dir(&fixture.root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(paths.len(), 1);
    let orphan = paths[0].file_stem().unwrap().to_str().unwrap();
    assert!(spool.read_chunk(&fixture.nonce, orphan, 0, 16).is_err());
    assert!(spool.ack(&fixture.nonce, orphan, &"a".repeat(64)).is_err());
    disk.0.store(false, Ordering::SeqCst);
    let d = spool.stage(&sample()).unwrap();
    spool.ack(&fixture.nonce, &d.capture_id, &d.sha256).unwrap();
    assert_eq!(fs::read_dir(&fixture.root).unwrap().count(), 1);
    assert!(fixture
        .open()
        .read_chunk(&fixture.nonce, orphan, 0, 1)
        .is_err());
}
#[test]
fn ack_requires_exact_hash() {
    let fixture = Fixture::new();
    let mut spool = fixture.open();
    let d = spool.stage(&sample()).unwrap();
    assert!(spool
        .ack(&fixture.nonce, &d.capture_id, &"0".repeat(64))
        .is_err());
    assert_eq!(
        spool
            .read_chunk(&fixture.nonce, &d.capture_id, 0, 32)
            .unwrap()
            .len(),
        32
    );
    assert_eq!(fs::read_dir(&fixture.root).unwrap().count(), 2);
    spool.ack(&fixture.nonce, &d.capture_id, &d.sha256).unwrap();
    assert_eq!(fs::read_dir(&fixture.root).unwrap().count(), 0);
    assert!(spool
        .read_chunk(&fixture.nonce, &d.capture_id, 0, 1)
        .is_err());
}
#[test]
fn duplicate_ack_is_idempotent() {
    let fixture = Fixture::new();
    let mut spool = fixture.open();
    let d = spool.stage(&sample()).unwrap();
    spool.ack(&fixture.nonce, &d.capture_id, &d.sha256).unwrap();
    spool.ack(&fixture.nonce, &d.capture_id, &d.sha256).unwrap();
    assert!(spool
        .ack(&fixture.nonce, &d.capture_id, &"0".repeat(64))
        .is_err());
    assert!(spool
        .ack(&fixture.nonce, &"0".repeat(32), &d.sha256)
        .is_err());
    assert_eq!(fs::read_dir(&fixture.root).unwrap().count(), 0);
}

#[test]
fn missing_files_do_not_erase_live_capacity_obligations() {
    let fixture = Fixture::new();
    let mut spool = fixture.open();
    let mut descriptors = Vec::new();
    for _ in 0..32 {
        descriptors.push(spool.stage(&sample()).unwrap());
    }
    let d = &descriptors[0];
    fs::remove_file(fixture.root.join(format!("{}.bin", d.capture_id))).unwrap();
    fs::remove_file(fixture.root.join(format!("{}.json", d.capture_id))).unwrap();
    assert!(spool.stage(&sample()).is_err());
}

#[cfg(windows)]
#[test]
fn partial_ack_keeps_obligation_and_retry_deletes_only_owned_files() {
    use std::os::windows::fs::OpenOptionsExt;
    let fixture = Fixture::new();
    let mut spool = fixture.open();
    let d = spool.stage(&sample()).unwrap();
    let pin = fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(fixture.root.join(format!("{}.json", d.capture_id)))
        .unwrap();
    assert!(spool.ack(&fixture.nonce, &d.capture_id, &d.sha256).is_err());
    assert_eq!(fs::read_dir(&fixture.root).unwrap().count(), 1);
    drop(pin);
    spool.ack(&fixture.nonce, &d.capture_id, &d.sha256).unwrap();
    assert_eq!(fs::read_dir(&fixture.root).unwrap().count(), 0);
}

#[test]
fn staged_file_replacement_or_hardlink_is_not_accepted_or_deleted() {
    let fixture = Fixture::new();
    let mut spool = fixture.open();
    let d = spool.stage(&sample()).unwrap();
    let path = fixture.root.join(format!("{}.bin", d.capture_id));
    let other = fixture.parent.join("other.bin");
    fs::rename(&path, &other).unwrap();
    fs::copy(&other, &path).unwrap();
    assert!(spool
        .read_chunk(&fixture.nonce, &d.capture_id, 0, 16)
        .is_err());
    assert!(spool.ack(&fixture.nonce, &d.capture_id, &d.sha256).is_err());
    assert!(path.exists());
    fs::remove_file(&path).unwrap();
    fs::hard_link(&other, &path).unwrap();
    assert!(spool
        .read_chunk(&fixture.nonce, &d.capture_id, 0, 16)
        .is_err());
    assert!(spool.ack(&fixture.nonce, &d.capture_id, &d.sha256).is_err());
    assert!(other.exists());
}
