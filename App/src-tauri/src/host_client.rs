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
// Kept inside the exchange future, which borrows the locked transport. On
// cancellation it fences that transport before the mutex can admit another call.
pub(crate) struct PendingExchange<'a> {
    failed: &'a std::sync::atomic::AtomicBool,
    complete: bool,
}
impl<'a> PendingExchange<'a> {
    pub(crate) fn new(failed: &'a std::sync::atomic::AtomicBool) -> Self {
        Self { failed, complete: false }
    }
    pub(crate) fn complete(&mut self) {
        self.complete = true;
    }
}
impl Drop for PendingExchange<'_> {
    fn drop(&mut self) {
        if !self.complete {
            self.failed.store(true, std::sync::atomic::Ordering::Release);
        }
    }
}
impl LocalHostClient {
    pub async fn connect(endpoint: &str) -> Result<Self, HostError> {
        Self::open(endpoint, json!({})).await
    }
    pub async fn attached(&self, channel: &str) -> Result<Self, HostError> {
        let result = Self::open(
            &self.endpoint,
            json!({"attach_token":self.attach_token,"channel":channel}),
        )
        .await;
        result.map_err(|error| {
            if matches!(channel, "background" | "status") && error.code == "SessionAttach" && error.message == "Invalid channel" {
                HostError::new("HostIncompatible", "Upgrade and safely restart the local Host to enable asynchronous request channels")
            } else { error }
        })
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
        if reply.result["mode"] != "real" || reply.result["worker_protocol"] != 3
            || !crate::host::contracts::native_host_compatible(&reply.result)
        {
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
        let deadline = tokio::time::Instant::now() + Duration::from_secs(95);
        let admission_timeout = || HostError::new("AdmissionTimeout", "Host request deadline expired before transmission; request was not sent");
        if self.failed.load(std::sync::atomic::Ordering::Acquire) {
            return Err(HostError::new(
                "HostOffline",
                "Transport is invalid; do not retry operations",
            ));
        }
        let mut pipe = tokio::time::timeout_at(deadline, self.pipe.lock())
            .await.map_err(|_| admission_timeout())?;
        // A preceding exchange can fail while this request waits for the pipe.
        if self.failed.load(std::sync::atomic::Ordering::Acquire) {
            return Err(HostError::new("HostOffline", "Transport is invalid; do not retry operations"));
        }
        let mut exchange_started = false;
        let outcome = tokio::time::timeout_at(deadline, async {
            // timeout_at polls a ready inner future first, even after expiry.
            if tokio::time::Instant::now() >= deadline {
                return Err(admission_timeout());
            }
            exchange_started = true;
            let mut exchange = PendingExchange::new(&self.failed);
            write_frame(&mut *pipe, &request).await?;
            let bytes = crate::host::ipc::read_frame_limit(
                &mut *pipe,
                crate::host::ipc::reply_limit(&request.method),
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
            exchange.complete();
            Ok(reply)
        })
        .await;
        if !exchange_started {
            return Err(admission_timeout());
        }
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
mod exchange_boundary_tests {
    use super::*;
    use std::{future::Future, task::Poll};
    use tokio::{net::windows::named_pipe::NamedPipeServer, time::{advance, pause, resume}};

    fn run(test: impl Future<Output = ()>) {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(test);
    }
    fn request(id: &str) -> HostRequest {
        HostRequest { v: 1, id: id.into(), method: "snapshot".into(), params: json!({}) }
    }
    async fn fixture() -> (LocalHostClient, NamedPipeServer) {
        let security = PipeSecurity::current().unwrap();
        let endpoint = format!("{}-test-{}", security.endpoint(), new_id().unwrap());
        let server = security.create_server(&endpoint, true).unwrap();
        let pipe = security.open_client(&endpoint).unwrap();
        server.connect().await.unwrap();
        (LocalHostClient { pipe: Mutex::new(pipe), endpoint, attach_token: String::new(), failed: false.into() }, server)
    }
    async fn poll_once<F: Future>(future: std::pin::Pin<&mut F>) -> Poll<F::Output> {
        let mut future = future;
        std::future::poll_fn(|cx| Poll::Ready(future.as_mut().poll(cx))).await
    }
    async fn observe_request<F: Future>(mut call: std::pin::Pin<&mut F>, server: &mut NamedPipeServer) {
        tokio::select! {
            _ = &mut call => panic!("call completed before the fixture replied"),
            bytes = crate::host::ipc::read_frame(server) => {
                let request: HostRequest = serde_json::from_slice(&bytes.unwrap().unwrap()).unwrap();
                assert_eq!(request.id, "pending");
            }
        }
    }
    async fn confirm_usable(client: &LocalHostClient, server: &mut NamedPipeServer) {
        let reply = async {
            let bytes = crate::host::ipc::read_frame(server).await.unwrap().unwrap();
            let request: HostRequest = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(request.id, "fresh", "an expired or cancelled queued call was transmitted");
            write_frame(server, &HostReply::from_result(request.id, Ok(json!({"fresh":true})))).await.unwrap();
        };
        let (result, ()) = tokio::join!(client.call(request("fresh")), reply);
        assert_eq!(result.unwrap().result["fresh"], true);
        assert!(!client.failed.load(std::sync::atomic::Ordering::Acquire));
    }

    // Missing admission bounds would leave this pending beyond the whole-call deadline.
    #[test]
    fn held_pipe_admission_expires_without_transmitting_or_poisoning() {
        run(async {
            let (client, mut server) = fixture().await;
            let guard = client.pipe.lock().await;
            pause();
            let mut call = Box::pin(client.call(request("expired")));
            assert!(poll_once(call.as_mut()).await.is_pending());
            advance(Duration::from_secs(96)).await;
            assert!(matches!(poll_once(call.as_mut()).await, Poll::Ready(Err(error)) if error.code == "AdmissionTimeout"));
            drop(call);
            assert!(!client.failed.load(std::sync::atomic::Ordering::Acquire));
            drop(guard);
            resume();
            confirm_usable(&client, &mut server).await;
        });
    }

    // A ready mutex must not win timeout_at's first poll after the deadline.
    #[test]
    fn expired_pipe_admission_does_not_send_when_the_mutex_becomes_ready() {
        run(async {
            let (client, mut server) = fixture().await;
            let guard = client.pipe.lock().await;
            pause();
            let mut call = Box::pin(client.call(request("expired")));
            assert!(poll_once(call.as_mut()).await.is_pending());
            advance(Duration::from_secs(96)).await;
            drop(guard);
            assert!(matches!(poll_once(call.as_mut()).await, Poll::Ready(Err(error)) if error.code == "AdmissionTimeout"));
            drop(call);
            assert!(!client.failed.load(std::sync::atomic::Ordering::Acquire));
            resume();
            confirm_usable(&client, &mut server).await;
        });
    }

    // Resetting the exchange timer after admission would incorrectly grant another 95 seconds.
    #[test]
    fn pipe_queue_and_exchange_share_one_deadline() {
        run(async {
            let (client, mut server) = fixture().await;
            let guard = client.pipe.lock().await;
            pause();
            let mut call = Box::pin(client.call(request("pending")));
            assert!(poll_once(call.as_mut()).await.is_pending());
            advance(Duration::from_secs(60)).await;
            drop(guard);
            resume();
            observe_request(call.as_mut(), &mut server).await;
            pause();
            advance(Duration::from_secs(36)).await;
            assert!(matches!(poll_once(call.as_mut()).await, Poll::Ready(Err(error)) if error.code == "OutcomeUnknown"));
            assert!(client.failed.load(std::sync::atomic::Ordering::Acquire));
        });
    }

    // An expired partial reply must fence the stream before any later request can write.
    #[test]
    fn post_send_pipe_timeout_poison_prevents_later_transmission() {
        run(async {
            let (client, mut server) = fixture().await;
            let mut call = Box::pin(client.call(request("pending")));
            observe_request(call.as_mut(), &mut server).await;
            tokio::io::AsyncWriteExt::write_all(&mut server, &[20, 0]).await.unwrap();
            pause();
            advance(Duration::from_secs(96)).await;
            assert!(matches!(poll_once(call.as_mut()).await, Poll::Ready(Err(error)) if error.code == "OutcomeUnknown"));
            drop(call);
            assert!(client.failed.load(std::sync::atomic::Ordering::Acquire));
            assert!(matches!(client.call(request("later")).await, Err(error) if error.code == "HostOffline"));
            drop(client);
            resume();
            assert!(crate::host::ipc::read_frame(&mut server).await.unwrap().is_none());
        });
    }

    // Dropping a transmitted call must not bypass the failure latch.
    #[test]
    fn dropping_transmitted_pipe_call_poison_prevents_later_transmission() {
        run(async {
            let (client, mut server) = fixture().await;
            let mut call = Box::pin(client.call(request("pending")));
            observe_request(call.as_mut(), &mut server).await;
            let mut queued = Box::pin(client.call(request("later")));
            assert!(poll_once(queued.as_mut()).await.is_pending());
            drop(call);
            assert!(client.failed.load(std::sync::atomic::Ordering::Acquire));
            assert!(matches!(poll_once(queued.as_mut()).await, Poll::Ready(Err(error)) if error.code == "HostOffline"));
            drop(queued);
            drop(client);
            assert!(crate::host::ipc::read_frame(&mut server).await.unwrap().is_none());
        });
    }

    // Cancelling admission has no uncertain exchange and must leave the channel usable.
    #[test]
    fn dropping_queued_pipe_call_does_not_poison_or_transmit() {
        run(async {
            let (client, mut server) = fixture().await;
            let guard = client.pipe.lock().await;
            let mut call = Box::pin(client.call(request("cancelled")));
            assert!(poll_once(call.as_mut()).await.is_pending());
            drop(call);
            assert!(!client.failed.load(std::sync::atomic::Ordering::Acquire));
            drop(guard);
            confirm_usable(&client, &mut server).await;
        });
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
                    json!({"mode":"real","worker_protocol":3}),
                    json!({"mode":"real","worker_protocol":3,"worker_kind":"python","worker_startup_revision":1}),
                    json!({"mode":"real","worker_protocol":3,"worker_kind":"rust"}),
                    json!({"mode":"real","worker_protocol":3,"worker_kind":"rust","worker_startup_revision":2}),
                    json!({"mode":"real","worker_protocol":3,"worker_startup_revision":1}),
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
    #[test]
    fn a_queued_call_cannot_send_after_the_previous_exchange_poisoned_the_pipe() {
        use std::future::Future;
        use std::sync::atomic::Ordering;
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
            let security = PipeSecurity::current().unwrap();
            let endpoint = format!("{}-test-{}", security.endpoint(), new_id().unwrap());
            let mut pipe = security.create_server(&endpoint, true).unwrap();
            let server = tokio::spawn(async move {
                pipe.connect().await.unwrap();
                let hello: HostRequest = serde_json::from_slice(&crate::host::ipc::read_frame(&mut pipe).await.unwrap().unwrap()).unwrap();
                write_frame(&mut pipe, &HostReply::from_result(hello.id, Ok(json!({"protocol_version":1,"mode":"real","worker_protocol":3,"worker_kind":"rust","worker_startup_revision":1,"attach_token":"a".repeat(32)})))).await.unwrap();
                if let Some(bytes) = crate::host::ipc::read_frame(&mut pipe).await.unwrap() {
                    let request: HostRequest = serde_json::from_slice(&bytes).unwrap();
                    write_frame(&mut pipe, &HostReply::from_result(request.id, Ok(json!({})))).await.unwrap();
                    true
                } else { false }
            });
            let client = LocalHostClient::connect(&endpoint).await.unwrap();
            let guard = client.pipe.lock().await;
            let mut waiting = Box::pin(client.call(HostRequest {v:1,id:"queued".into(),method:"prepare".into(),params:json!({})}));
            std::future::poll_fn(|cx| {
                assert!(waiting.as_mut().poll(cx).is_pending());
                std::task::Poll::Ready(())
            }).await;
            client.failed.store(true, Ordering::Release);drop(guard);
            let result = waiting.await;drop(client);
            let sent = server.await.unwrap();
            assert!(!sent, "a queued request reached a poisoned transport");
            assert!(matches!(result, Err(error) if error.code == "HostOffline"));
        });
    }
    #[test]
    fn old_host_channel_rejection_requires_upgrade_without_request_replay() {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
            let security = PipeSecurity::current().unwrap();
            let endpoint = format!("{}-test-{}", security.endpoint(), new_id().unwrap());
            let mut primary = security.create_server(&endpoint, true).unwrap();
            let server = tokio::spawn(async move {
                primary.connect().await.unwrap();
                let hello: HostRequest = serde_json::from_slice(&crate::host::ipc::read_frame(&mut primary).await.unwrap().unwrap()).unwrap();
                write_frame(&mut primary, &HostReply::from_result(hello.id, Ok(json!({"protocol_version":1,"mode":"real","worker_protocol":3,"worker_kind":"rust","worker_startup_revision":1,"attach_token":"a".repeat(32)})))).await.unwrap();
                assert!(crate::host::ipc::read_frame(&mut primary).await.unwrap().is_none());
            });
            let client = LocalHostClient::connect(&endpoint).await.unwrap();
            for name in ["background", "status"] {
                let mut channel = security.create_server(&endpoint, false).unwrap();
                let reject = tokio::spawn(async move {
                    channel.connect().await.unwrap();
                    let request: HostRequest = serde_json::from_slice(&crate::host::ipc::read_frame(&mut channel).await.unwrap().unwrap()).unwrap();
                    assert_eq!(request.method,"ping");assert_eq!(request.params["channel"],name);
                    write_frame(&mut channel, &HostReply::from_result(request.id, Err(HostError::new("SessionAttach", "Invalid channel")))).await.unwrap();
                    assert!(crate::host::ipc::read_frame(&mut channel).await.unwrap().is_none(), "attachment was replayed");
                });
                let result = client.attached(name).await;
                assert!(matches!(result, Err(error) if error.code == "HostIncompatible"));
                reject.await.unwrap();
            }
            drop(client);server.await.unwrap();
        });
    }
}

#[cfg(test)]
mod native_management_tests {
    use super::*;
    #[test]
    fn failed_native_startup_allows_management_attachment() {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
            tokio::time::timeout(Duration::from_secs(5), async {
                let security = PipeSecurity::current().unwrap();
                let endpoint = format!("{}-test-{}", security.endpoint(), new_id().unwrap());
                let mut pipe = security.create_server(&endpoint, true).unwrap();
                let service = async {
                    pipe.connect().await.unwrap();
                    let hello: HostRequest = serde_json::from_slice(&crate::host::ipc::read_frame(&mut pipe).await.unwrap().unwrap()).unwrap();
                    write_frame(&mut pipe, &HostReply::from_result(hello.id, Ok(json!({"protocol_version":1,"mode":"real","worker_protocol":3,"worker_kind":"rust","worker_startup_revision":1,"worker_startup_verified":false,"worker_activation_confirmed":false,"startup_error":"retained fixture","attach_token":"a".repeat(32)})))).await.unwrap();
                    let status: HostRequest = serde_json::from_slice(&crate::host::ipc::read_frame(&mut pipe).await.unwrap().unwrap()).unwrap();
                    assert_eq!(status.method,"worker_status");
                    write_frame(&mut pipe, &HostReply::from_result(status.id, Ok(json!({"startup_error":"retained fixture","host_status":"RETAINED","worker_startup_verified":false,"worker_activation_confirmed":false})))).await.unwrap();
                };
                let client = async {
                    let client = LocalHostClient::connect(&endpoint).await.unwrap();
                    let status = client.call(HostRequest {v:1,id:"status".into(),method:"worker_status".into(),params:json!({})}).await.unwrap();
                    assert_eq!(status.result["host_status"],"RETAINED");
                    assert_eq!(status.result["worker_startup_verified"],false);
                };
                tokio::join!(service, client);
            }).await.unwrap();
        });
    }
}
