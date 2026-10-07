#[cfg(test)]
#[path = "diagnostics_tests.rs"]
mod tests;

use crate::{
    host::{
        archive::{ArchiveRef, SelectedDirectory},
        contracts::HostError,
        ipc::HostRequest,
        registry::new_id,
    },
    remote_client::{RemoteHostClient, RemotePeer},
};
use serde::Serialize;
use serde_json::{json, Value};
use std::{path::Path, sync::Arc};
#[derive(Serialize)]
pub struct RemoteDiagnosticReport {
    pub observation: Value,
    pub release_receipt: Value,
    pub release_confirmed: bool,
    pub error: Option<String>,
}
/// Only consumes existing user-protected trust. No pairing, trust edits, leases
/// or instrument operations are exposed by this diagnostic interface.
pub async fn diagnose_saved_peer(
    peers: &Path,
    host_id: &str,
    archive: Option<ArchiveRef>,
    output: &Path,
) -> Result<RemoteDiagnosticReport, HostError> {
    if !peers.is_absolute() || !crate::host::contracts::valid_id(host_id) {
        return Err(HostError::new("Diagnostic", "Invalid saved peer selection"));
    }
    let mut saved: Vec<RemotePeer> = crate::remote::load_secret(peers)?;
    if saved.len() > 64 || saved.iter().filter(|p| p.host_id == host_id).count() != 1 {
        return Err(HostError::new(
            "Diagnostic",
            "Select one existing trusted Host",
        ));
    }
    let index = saved.iter().position(|p| p.host_id == host_id).unwrap();
    diagnose_peer(saved.swap_remove(index), archive, output).await
}
async fn call(client: &RemoteHostClient, method: &str, params: Value) -> Result<Value, HostError> {
    let reply = client
        .call(HostRequest {
            v: 1,
            id: new_id()?,
            method: method.into(),
            params,
        })
        .await?;
    if reply.ok {
        Ok(reply.result)
    } else {
        Err(reply
            .error
            .unwrap_or_else(|| HostError::new("Diagnostic", "Missing Host error")))
    }
}
async fn diagnose_peer(
    peer: RemotePeer,
    archive: Option<ArchiveRef>,
    output: &Path,
) -> Result<RemoteDiagnosticReport, HostError> {
    // Pin the user-selected ordinary directory before connecting. A missing
    // destination never starts an authenticated session.
    let selected = SelectedDirectory::open(output)?;
    if let Some(r) = &archive {
        r.validate()?;
        if r.host_id != peer.host_id {
            return Err(HostError::new(
                "Diagnostic",
                "Archive belongs to another Host",
            ));
        }
    }
    let client = Arc::new(RemoteHostClient::connect(peer.clone()).await?);
    let mut observation =
        json!({"host_id":peer.host_id,"source_kind":"real","instrument_actions_started":false});
    let work = async {
        observation["worker_status"] = call(&client, "worker_status", json!({})).await?;
        if let Some(reference) = archive {
            let export = crate::native_files::receive_archive(
                &reference,
                &peer.host_id,
                |method, params| {
                    let client = client.clone();
                    let method = method.to_owned();
                    async move { call(&client, &method, params).await }
                },
            )
            .await?;
            export.write_raw(&selected)?;
            observation["archive"] = json!(reference);
        }
        Ok::<(), HostError>(())
    }
    .await;
    // Read/download failures do not omit release. A failed stream may only be
    // reconciled through the existing fresh authenticated observer protocol.
    let release = match call(&client, "close_client", json!({})).await {
        Ok(r) => Ok(r),
        Err(_) => client.reconcile(peer).await,
    };
    let confirmed = release.as_ref().is_ok_and(|r| r["released"] == true);
    let receipt = match release {
        Ok(r) => {
            json!({"released":confirmed,"host_id":r["host_id"],"boot_id":r["boot_id"],"client_session_id":r["client_session_id"],"cleanup":r["cleanup"]})
        }
        Err(e) => json!({"released":false,"error":e.to_string(),"outcome":"unknown"}),
    };
    Ok(RemoteDiagnosticReport {
        observation,
        release_receipt: receipt,
        release_confirmed: confirmed,
        error: work.err().map(|e| e.to_string()),
    })
}
