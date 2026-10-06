//! Native-user-authorized folders and data-only archive export.

use crate::host::{
    archive::{ArchiveRef, NativeExport, SelectedDirectory},
    contracts::HostError,
    verification::sha256_bytes,
};
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
fn error(message: impl std::fmt::Display) -> HostError {
    HostError::new("ArchiveExport", message.to_string())
}
pub(crate) struct SelectionGuard(Arc<AtomicBool>);
impl SelectionGuard {
    pub(crate) fn begin(gate: &Arc<AtomicBool>) -> Result<Self, HostError> {
        if gate
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(error("A folder selection or export is already in progress"));
        }
        Ok(Self(gate.clone()))
    }
}
impl Drop for SelectionGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}
fn resolve_choice(choice: Option<PathBuf>) -> Result<Option<SelectedDirectory>, HostError> {
    choice
        .map(|path| SelectedDirectory::open(&path))
        .transpose()
}
fn decode_hex(reply: &Value, length: usize) -> Result<Vec<u8>, HostError> {
    let text = reply["data_hex"]
        .as_str()
        .ok_or_else(|| error("Missing archive bytes"))?;
    if text.len() != length * 2
        || !text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(error("Invalid archive byte encoding or length"));
    }
    text.as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).map_err(error))
        .collect()
}
pub(crate) async fn receive_archive<F, Fut>(
    reference: &ArchiveRef,
    connected_host: &str,
    mut call: F,
) -> Result<NativeExport, HostError>
where
    F: FnMut(&str, Value) -> Fut,
    Fut: std::future::Future<Output = Result<Value, HostError>>,
{
    reference.validate()?;
    if connected_host != reference.host_id {
        return Err(error("Archive belongs to another connected Host"));
    }
    let access = json!({"domain":reference.domain,"name":reference.name,"id":reference.id});
    let reply = call("archive_manifest_bytes", access.clone()).await?;
    let length = reply["byte_count"]
        .as_u64()
        .filter(|n| *n > 0 && *n <= 16384)
        .ok_or_else(|| error("Manifest size out of range"))?;
    if reply["id"] != reference.id || reply["name"] != reference.name {
        return Err(error("Manifest reply scope mismatch"));
    }
    let manifest = decode_hex(&reply, length as usize)?;
    if reply["sha256"] != sha256_bytes(&manifest)? {
        return Err(error("Manifest reply hash mismatch"));
    }
    let mut native = Vec::with_capacity(reference.byte_count as usize);
    while (native.len() as u64) < reference.byte_count {
        let offset = native.len() as u64;
        let length = (reference.byte_count - offset).min(16384);
        let mut params = access.clone();
        params["offset"] = json!(offset);
        params["length"] = json!(length);
        let reply = call("read_archive", params).await?;
        if reply["id"] != reference.id
            || reply["name"] != reference.name
            || reply["offset"] != offset
            || reply["length"] != length
            || reply["sha256"] != reference.sha256
        {
            return Err(error("Archive chunk reply binding mismatch"));
        }
        native.extend(decode_hex(&reply, length as usize)?);
    }
    NativeExport::verify(reference, manifest, native)
}
pub(crate) async fn choose_folder(
    owner: isize,
    title: &'static str,
    guard: SelectionGuard,
) -> Result<(Option<SelectedDirectory>, SelectionGuard), HostError> {
    if owner == 0 {
        return Err(error("A native parent window is required"));
    }
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::Builder::new()
        .name("yang-folder-dialog".into())
        .spawn(move || {
            // If the caller disappears, the guard stays with the live native dialog
            // until it exits; no second dialog can start while that thread is alive.
            let result = folder_dialog(owner, title)
                .and_then(resolve_choice)
                .map(|choice| (choice, guard));
            let _ = tx.send(result);
        })
        .map_err(error)?;
    rx.await.map_err(error)?
}
#[cfg(windows)]
fn folder_dialog(owner: isize, title: &str) -> Result<Option<PathBuf>, HostError> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::{
        System::Com::{CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_APARTMENTTHREADED},
        UI::Shell::{
            SHBrowseForFolderW, SHGetPathFromIDListEx, BIF_NEWDIALOGSTYLE, BIF_NONEWFOLDERBUTTON,
            BIF_NOTRANSLATETARGETS, BIF_RETURNONLYFSDIRS, BROWSEINFOW, GPFIDL_DEFAULT,
        },
    };
    // Shell folder UI requires an STA. Never initialize COM on a Tokio worker.
    let initialized = unsafe { CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32) };
    if initialized < 0 {
        return Err(error(format!(
            "Native dialog COM initialization failed: {initialized:#x}"
        )));
    }
    struct Com;
    impl Drop for Com {
        fn drop(&mut self) {
            unsafe { CoUninitialize() }
        }
    }
    let _com = Com;
    let title: Vec<u16> = title.encode_utf16().chain([0]).collect();
    let mut display = [0_u16; 260];
    let info = BROWSEINFOW {
        hwndOwner: owner as _,
        pszDisplayName: display.as_mut_ptr(),
        lpszTitle: title.as_ptr(),
        ulFlags: BIF_NEWDIALOGSTYLE
            | BIF_RETURNONLYFSDIRS
            | BIF_NOTRANSLATETARGETS
            | BIF_NONEWFOLDERBUTTON,
        ..Default::default()
    };
    let pidl = unsafe { SHBrowseForFolderW(&info) };
    if pidl.is_null() {
        return Ok(None);
    }
    struct Pidl(*mut windows_sys::Win32::UI::Shell::Common::ITEMIDLIST);
    impl Drop for Pidl {
        fn drop(&mut self) {
            unsafe { CoTaskMemFree(self.0.cast()) }
        }
    }
    let _pidl = Pidl(pidl);
    let mut buffer = vec![0_u16; 32768];
    if unsafe {
        SHGetPathFromIDListEx(
            pidl,
            buffer.as_mut_ptr(),
            buffer.len() as u32,
            GPFIDL_DEFAULT,
        )
    } == 0
    {
        return Err(error("Choose an ordinary filesystem folder"));
    }
    let length = buffer
        .iter()
        .position(|c| *c == 0)
        .filter(|n| *n > 0)
        .ok_or_else(|| error("Invalid folder path"))?;
    Ok(Some(PathBuf::from(std::ffi::OsString::from_wide(
        &buffer[..length],
    ))))
}
#[cfg(not(windows))]
fn folder_dialog(_: isize, _: &str) -> Result<Option<PathBuf>, HostError> {
    Err(error("Native folder selection requires Windows"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::{archive::ArchiveRef, leases::DomainRef, verification::sha256_bytes};
    use serde_json::{json, Value};
    fn capture() -> (ArchiveRef, Vec<u8>, Vec<u8>) {
        let bytes: Vec<_> = [1549_f64, -210., 1550., -69.3149081]
            .into_iter()
            .flat_map(f64::to_le_bytes)
            .collect();
        let context = json!({"transfer_format":"ASCII","sample_count":2,"spacing":0,"level_unit":0,
            "x_unit":0,"trace_attribute":0,"active_trace":"TRA","center_m":1.55e-6,
            "span_m":2e-9,"resolution_m":2e-11,"sweep_mode":1});
        let reference = ArchiveRef {
            id: "b".repeat(32),
            name: "osa".into(),
            host_id: "a".repeat(32),
            domain: DomainRef {
                kind: "device".into(),
                id: "c".repeat(32),
            },
            byte_count: 32,
            sample_count: 2,
            sha256: sha256_bytes(&bytes).unwrap(),
            metadata: json!({"native_unit":"dBm","trace":"A","identity":"YOKOGAWA,AQ6370E,TEST-BYTES,FW",
                "read_started_at":"2026-10-06T10:00:00+00:00","read_finished_at":"2026-10-06T10:00:01+00:00",
                "elapsed_s":1.,"consistency":"unproven","context_before":context,"context_after":context}),
        };
        let manifest=serde_json::to_vec_pretty(&json!({"schema":1,"source_kind":"real","name":"osa","archived_at_unix_ms":1,
            "origin":{"host_id":reference.host_id,"domain":reference.domain,"device_identity":{"model":"AQ6370E"},"config_rev":1,"operation_id":reference.id},
            "descriptor":{"schema":1,"kind":"osa_trace","capture_id":"d".repeat(32),"point_count":2,"byte_count":32,"sha256":reference.sha256,"metadata":reference.metadata}})).unwrap();
        (reference, manifest, bytes)
    }
    fn response(
        reference: &ArchiveRef,
        manifest: &[u8],
        bytes: &[u8],
        method: &str,
        params: &Value,
    ) -> Value {
        if method == "archive_manifest_bytes" {
            json!({"id":reference.id,"name":reference.name,"byte_count":manifest.len(),"sha256":sha256_bytes(manifest).unwrap(),"data_hex":hex(manifest)})
        } else {
            assert_eq!(method, "read_archive");
            let offset = params["offset"].as_u64().unwrap() as usize;
            let length = params["length"].as_u64().unwrap() as usize;
            assert!((1..=16384).contains(&length));
            json!({"id":reference.id,"name":reference.name,"offset":offset,"length":length,"sha256":reference.sha256,"data_hex":hex(&bytes[offset..offset+length])})
        }
    }
    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }
    #[test]
    fn native_export_fetches_only_scoped_history_and_preserves_original_bytes() {
        let (reference, manifest, bytes) = capture();
        let mut calls = Vec::new();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(receive_archive(
            &reference,
            &reference.host_id,
            |method, params| {
                calls.push((method.to_owned(), params.clone()));
                std::future::ready(Ok(response(&reference, &manifest, &bytes, method, &params)))
            },
        ));
        assert!(result.is_ok());
        assert_eq!(calls.len(), 2);
        for (_, params) in &calls {
            assert_eq!(
                params["domain"],
                serde_json::to_value(&reference.domain).unwrap()
            );
        }
        assert!(!serde_json::to_string(&calls)
            .unwrap()
            .contains("lease_token"));
        let dest = std::env::temp_dir().join(format!(
            "native-export-{}",
            crate::host::registry::new_id().unwrap()
        ));
        std::fs::create_dir(&dest).unwrap();
        let path = result
            .unwrap()
            .write(&resolve_choice(Some(dest.clone())).unwrap().unwrap())
            .unwrap();
        assert_eq!(std::fs::read(path.join("manifest.json")).unwrap(), manifest);
        std::fs::remove_dir_all(dest).unwrap();
    }
    #[test]
    fn native_export_rejects_host_mismatch_and_invalid_reference_before_any_history_call() {
        let (reference, _, _) = capture();
        let rt = tokio::runtime::Runtime::new().unwrap();
        assert!(rt
            .block_on(receive_archive(&reference, &"e".repeat(32), |_, _| async {
                panic!("wrong host fetch")
            }))
            .is_err());
        for reference in [
            {
                let mut r = reference.clone();
                r.name = "../escape".into();
                r
            },
            {
                let mut r = reference.clone();
                r.byte_count = 3200032;
                r
            },
            {
                let mut r = reference.clone();
                r.metadata["huge"] = json!("a".repeat(8193));
                r
            },
        ] {
            assert!(rt
                .block_on(receive_archive(
                    &reference,
                    &reference.host_id,
                    |_, _| async { panic!("invalid fetch") }
                ))
                .is_err());
        }
    }
    #[test]
    fn native_export_rejects_truncated_changed_or_unbound_history_replies() {
        let (reference, manifest, bytes) = capture();
        let rt = tokio::runtime::Runtime::new().unwrap();
        for fault in 0..7 {
            let result = rt.block_on(receive_archive(
                &reference,
                &reference.host_id,
                |method, params| {
                    let mut reply = response(&reference, &manifest, &bytes, method, &params);
                    match (fault, method) {
                        (0, "archive_manifest_bytes") => reply["id"] = json!("e".repeat(32)),
                        (1, "archive_manifest_bytes") => reply["byte_count"] = json!(16385),
                        (2, "archive_manifest_bytes") => reply["sha256"] = json!("0".repeat(64)),
                        (3, "read_archive") => reply["offset"] = json!(16),
                        (4, "read_archive") => reply["data_hex"] = json!("00"),
                        (5, "read_archive") => reply["sha256"] = json!("0".repeat(64)),
                        (6, "read_archive") => reply["data_hex"] = json!("ff".repeat(32)),
                        _ => {}
                    }
                    std::future::ready(Ok(reply))
                },
            ));
            assert!(result.is_err(), "accepted fault {fault}");
        }
    }
    #[test]
    fn native_selection_cancel_and_concurrent_dialog_leave_resources_unchanged() {
        assert!(resolve_choice(None).unwrap().is_none());
        let gate = Arc::new(AtomicBool::new(false));
        let first = SelectionGuard::begin(&gate).unwrap();
        assert!(SelectionGuard::begin(&gate).is_err());
        drop(first);
        assert!(SelectionGuard::begin(&gate).is_ok());
    }
}
