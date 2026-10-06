use crate::host::{
    contracts::HostError,
    ipc::{write_frame, HostReply, HostRequest, PipeSecurity},
    registry::new_id,
};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::{net::windows::named_pipe::NamedPipeClient, sync::Mutex};
pub struct LocalHostClient {
    pipe: Mutex<NamedPipeClient>,
    endpoint: String,
    attach_token: String,
    failed: std::sync::atomic::AtomicBool,
}
pub struct HostEvents {
    pub snapshot: Value,
    client: LocalHostClient,
}
impl LocalHostClient {
    pub async fn connect(endpoint: &str) -> Result<Self, HostError> {
        Self::open(endpoint, json!({})).await
    }
    pub async fn attached(&self, channel: &str) -> Result<Self, HostError> {
        Self::open(
            &self.endpoint,
            json!({"attach_token":self.attach_token,"channel":channel}),
        )
        .await
    }
    async fn open(endpoint: &str, params: Value) -> Result<Self, HostError> {
        let security = PipeSecurity::current()?;
        let pipe = security.open_client(endpoint)?;
        let mut client = Self {
            pipe: Mutex::new(pipe),
            endpoint: endpoint.into(),
            attach_token: String::new(),
            failed: std::sync::atomic::AtomicBool::new(false),
        };
        let reply = client
            .call(HostRequest {
                v: 1,
                id: new_id()?,
                method: "ping".into(),
                params,
            })
            .await?;
        if !reply.ok || reply.result["protocol_version"] != 1 {
            return Err(reply
                .error
                .unwrap_or_else(|| HostError::new("HostProtocol", "Invalid Host handshake")));
        }
        if reply.result["mode"] != "real" || reply.result["worker_protocol"] != 3 {
            return Err(HostError::new(
                "HostIncompatible",
                "Upgrade and safely restart the local Host before connecting instruments",
            ));
        }
        client.attach_token = reply.result["attach_token"]
            .as_str()
            .filter(|s| crate::host::contracts::valid_id(s))
            .ok_or_else(|| {
                HostError::new("HostProtocol", "Private channel attachment token missing")
            })?
            .into();
        Ok(client)
    }
    pub async fn call(&self, request: HostRequest) -> Result<HostReply, HostError> {
        if self.failed.load(std::sync::atomic::Ordering::Acquire) {
            return Err(HostError::new(
                "HostOffline",
                "Transport is invalid; do not retry operations",
            ));
        }
        let mut pipe = self.pipe.lock().await;
        let outcome = tokio::time::timeout(Duration::from_secs(95), async {
            write_frame(&mut *pipe, &request).await?;
            let bytes = crate::host::ipc::read_frame_limit(
                &mut *pipe,
                if request.method == "read_result" {
                    600 * 1024
                } else {
                    crate::host::ipc::MAX_FRAME
                },
            )
            .await?
            .ok_or_else(|| HostError::new("HostOffline", "Host disconnected"))?;
            let value = crate::runtime::strict_json(&bytes)
                .map_err(|error| HostError::new("HostProtocol", error))?;
            let reply: HostReply = serde_json::from_value(value)
                .map_err(|error| HostError::new("HostProtocol", error.to_string()))?;
            if reply.v != 1
                || (!reply.id.is_empty() && reply.id != request.id)
                || reply.ok == reply.error.is_some()
            {
                return Err(HostError::new(
                    "HostProtocol",
                    "Reply does not match request",
                ));
            }
            Ok(reply)
        })
        .await;
        finish_call(&self.failed, outcome)
    }
    pub async fn subscribe(self) -> Result<HostEvents, HostError> {
        let reply = self
            .call(HostRequest {
                v: 1,
                id: new_id()?,
                method: "subscribe".into(),
                params: json!({}),
            })
            .await?;
        if !reply.ok {
            return Err(reply.error.unwrap());
        }
        let snapshot = crate::host::events::read_event(&mut *self.pipe.lock().await)
            .await?
            .ok_or_else(|| HostError::new("HostOffline", "Snapshot stream closed"))?;
        Ok(HostEvents {
            snapshot,
            client: self,
        })
    }
}
fn finish_call(
    failed: &std::sync::atomic::AtomicBool,
    outcome: Result<Result<HostReply, HostError>, tokio::time::error::Elapsed>,
) -> Result<HostReply, HostError> {
    let outcome = outcome.unwrap_or_else(|_| {
        Err(HostError::new(
            "OutcomeUnknown",
            "Host reply deadline expired; no automatic retry",
        ))
    });
    if outcome.is_err() {
        failed.store(true, std::sync::atomic::Ordering::Release);
    }
    outcome
}
impl HostEvents {
    pub async fn next(&self) -> Result<Option<Value>, HostError> {
        let mut pipe = self.client.pipe.lock().await;
        crate::host::events::read_event(&mut *pipe).await
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn real_only_client_rejects_missing_or_obsolete_worker_provenance() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                for metadata in [
                    json!({}),
                    json!({"mode":"simulate","worker_protocol":3}),
                    json!({"mode":"real","worker_protocol":2}),
                ] {
                    let security = PipeSecurity::current().unwrap();
                    let endpoint = format!("{}-test-{}", security.endpoint(), new_id().unwrap());
                    let mut pipe = security.create_server(&endpoint, true).unwrap();
                    let server = tokio::spawn(async move {
                        pipe.connect().await.unwrap();
                        let request: HostRequest = serde_json::from_slice(
                            &crate::host::ipc::read_frame(&mut pipe)
                                .await
                                .unwrap()
                                .unwrap(),
                        )
                        .unwrap();
                        let mut reply = json!({"protocol_version":1,"attach_token":"a".repeat(32)});
                        for (key, value) in metadata.as_object().unwrap() {
                            reply[key] = value.clone();
                        }
                        write_frame(&mut pipe, &HostReply::from_result(request.id, Ok(reply)))
                            .await
                            .unwrap();
                    });
                    let rejected = LocalHostClient::connect(&endpoint).await;
                    assert!(matches!(rejected, Err(ref error) if error.code == "HostIncompatible"));
                    server.await.unwrap();
                }
            });
    }
    #[test]
    fn deadline_poison_prevents_reusing_partial_frame() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let failed = std::sync::atomic::AtomicBool::new(false);
                let timeout = tokio::time::timeout(
                    Duration::from_millis(1),
                    std::future::pending::<Result<HostReply, HostError>>(),
                )
                .await;
                assert!(finish_call(&failed, timeout).is_err());
                assert!(failed.load(std::sync::atomic::Ordering::Acquire));
            });
    }
}
