//! Isolated pairing bootstrap: no device dispatcher, lease or client session.
use crate::{
    host::{
        contracts::{valid_id, HostError},
        ipc::{read_frame, write_frame, HostReply, HostRequest},
        registry::new_id,
        verification::sha256_bytes,
    },
    remote::{ClientStream, RemoteStore},
    remote_client::RemotePeer,
    remote_pairing::{comparison, PairTranscript, PAIR_TTL},
};
use rustls::{
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    pki_types::{CertificateDer, ServerName, UnixTime},
    DigitallySignedStruct, SignatureScheme,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    net::IpAddr,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tokio::io::AsyncReadExt;
use tokio::sync::{watch, OwnedSemaphorePermit, Semaphore};
pub(crate) struct PairIdentity {
    pub id: String,
    pub name: String,
}
pub(crate) struct PendingPair {
    stream: ClientStream,
    transcript: PairTranscript,
    owner_name: String,
    nickname: Option<String>,
    endpoint: String,
    comparison: String,
    deadline: tokio::time::Instant,
}
pub(crate) struct PairOwner<'a> {
    pub trust: &'a Mutex<RemoteStore>,
    pub generation: &'a AtomicU64,
    pub stopped: &'a AtomicBool,
    pub name: String,
}
pub(crate) fn bootstrap_permit(slots: &Arc<Semaphore>) -> Result<OwnedSemaphorePermit, HostError> {
    slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| HostError::new("PairingBusy", "Four connection requests are pending"))
}
fn protocol() -> HostError {
    HostError::new("PairingProtocol", "Invalid or mismatched pairing reply")
}
fn expired() -> HostError {
    HostError::new("PairingExpired", "Request expired or listener changed")
}
fn valid_name(name: &str) -> bool {
    !name.trim().is_empty() && name.len() <= 128 && !name.chars().any(char::is_control)
}
#[derive(Debug)]
struct BootstrapCertificate(Option<String>);
impl ServerCertVerifier for BootstrapCertificate {
    fn verify_server_cert(
        &self,
        cert: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if self
            .0
            .as_ref()
            .is_some_and(|pin| sha256_bytes(cert.as_ref()).ok().as_ref() != Some(pin))
        {
            return Err(rustls::Error::General("Saved certificate changed".into()));
        }
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        m: &[u8],
        cert: &CertificateDer<'_>,
        sig: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            m,
            cert,
            sig,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }
    fn verify_tls13_signature(
        &self,
        m: &[u8],
        cert: &CertificateDer<'_>,
        sig: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            m,
            cert,
            sig,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Start {
    version: u32,
    peer_id: String,
    name: String,
    nonce: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Ack {
    version: u32,
    phase: String,
    request_id: String,
    nonce: String,
    ticket: String,
    peer_id: String,
    owner_id: String,
    owner_name: String,
    expires_in_ms: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Completed {
    version: u32,
    phase: String,
    request_id: String,
    nonce: String,
    ticket: String,
    peer_id: String,
    owner_id: String,
    credential: String,
}
async fn reply(stream: &mut ClientStream, id: &str, initial: bool) -> Result<Value, HostError> {
    let unavailable = || {
        if initial {
            HostError::new(
                "PairingUpdateRequired",
                "Owning Host needs the new pairing protocol",
            )
        } else {
            HostError::new(
                "PairingIncomplete",
                "Pairing delivery failed; owner may retain authorization",
            )
        }
    };
    let bytes = read_frame(stream)
        .await
        .map_err(|_| unavailable())?
        .ok_or_else(unavailable)?;
    let value = crate::runtime::strict_json(&bytes).map_err(|_| protocol())?;
    let r: HostReply = serde_json::from_value(value).map_err(|_| protocol())?;
    if r.v != 1 || r.id != id || r.ok == r.error.is_some() {
        return Err(protocol());
    }
    if !r.ok {
        let code = r.error.unwrap().code;
        return Err(match code.as_str() {
            "PairingRejected" => HostError::new("PairingRejected", "Owner rejected the request"),
            "PairingExpired" => expired(),
            "PairingBusy" | "PairingRate" | "PairingKnown" => HostError::new(
                &code,
                "Request unavailable; use an existing pairing or try later",
            ),
            _ => protocol(),
        });
    }
    Ok(r.result)
}
pub(crate) async fn open_pair(
    endpoint: &str,
    identity: PairIdentity,
    nickname: Option<String>,
    known: &[RemotePeer],
) -> Result<PendingPair, HostError> {
    let deadline = tokio::time::Instant::now() + PAIR_TTL;
    let address = crate::remote::endpoint(endpoint)?;
    let endpoint = address.to_string();
    if !valid_id(&identity.id)
        || !valid_name(&identity.name)
        || nickname.as_ref().is_some_and(|s| !valid_name(s))
    {
        return Err(HostError::new(
            "PairingRejected",
            "Invalid identity or nickname",
        ));
    }
    let pin = known
        .iter()
        .find(|p| p.endpoint == endpoint)
        .map(|p| p.fingerprint.clone());
    let mut config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_protocol_versions(&[&rustls::version::TLS13])
    .map_err(|_| protocol())?
    .dangerous()
    .with_custom_certificate_verifier(Arc::new(BootstrapCertificate(pin)))
    .with_no_client_auth();
    config.resumption = rustls::client::Resumption::disabled();
    config.enable_early_data = false;
    let mut stream = tokio::time::timeout(Duration::from_secs(10), async {
        let tcp = tokio::net::TcpStream::connect(address)
            .await
            .map_err(|_| HostError::new("RemoteOffline", "Computer is unreachable"))?;
        tokio_rustls::TlsConnector::from(Arc::new(config))
            .connect(ServerName::try_from("yang-lab").unwrap(), tcp)
            .await
            .map_err(|_| HostError::new("RemoteTls", "TLS handshake or saved pin rejected"))
    })
    .await
    .map_err(|_| HostError::new("RemoteOffline", "Connection timed out"))??;
    let certificate = stream
        .get_ref()
        .1
        .peer_certificates()
        .and_then(|c| c.first())
        .ok_or_else(protocol)?;
    let fingerprint = sha256_bytes(certificate.as_ref())?;
    let exporter = stream
        .get_ref()
        .1
        .export_keying_material([0u8; 32], b"EXPORTER-Channel-Binding", Some(b""))
        .map_err(|_| protocol())?;
    let id = new_id()?;
    let nonce = new_id()?;
    let request = HostRequest {
        v: 1,
        id: id.clone(),
        method: "remote_pair_v2".into(),
        params: json!({"version":2,"peer_id":identity.id,"name":identity.name,"nonce":nonce}),
    };
    let ack = tokio::time::timeout(Duration::from_secs(10), async {
        write_frame(&mut stream, &request).await?;
        reply(&mut stream, &id, true).await
    })
    .await
    .map_err(|_| {
        HostError::new(
            "PairingUpdateRequired",
            "No pairing acknowledgment; update the owning Host",
        )
    })??;
    let ack: Ack = serde_json::from_value(ack).map_err(|_| protocol())?;
    if ack.version != 2
        || ack.phase != "Waiting"
        || ack.request_id != id
        || ack.nonce != nonce
        || ack.peer_id != identity.id
        || !valid_name(&ack.owner_name)
        || ack.expires_in_ms == 0
        || ack.expires_in_ms > 120000
    {
        return Err(protocol());
    }
    let transcript = PairTranscript {
        version: 2,
        request_id: id,
        nonce,
        ticket: ack.ticket,
        peer_id: identity.id,
        owner_id: ack.owner_id,
        fingerprint,
    };
    transcript.validate()?;
    if let Some(saved) = known.iter().find(|p| p.host_id == transcript.owner_id) {
        if saved.fingerprint != transcript.fingerprint {
            return Err(HostError::new(
                "RemotePin",
                "Saved owning certificate changed",
            ));
        }
        return Err(HostError::new(
            "PairingKnown",
            "Already paired; use Connect",
        ));
    }
    if known.iter().any(|p| p.endpoint == endpoint) {
        return Err(HostError::new(
            "RemoteIdentity",
            "Saved endpoint belongs to another Host",
        ));
    }
    let comparison = comparison(&exporter, &transcript)?;
    let deadline =
        deadline.min(tokio::time::Instant::now() + Duration::from_millis(ack.expires_in_ms));
    Ok(PendingPair {
        stream,
        transcript,
        owner_name: ack.owner_name,
        nickname,
        endpoint,
        comparison,
        deadline,
    })
}
impl PendingPair {
    pub fn public(&self) -> Value {
        json!({"request_id":self.transcript.request_id,"phase":"Waiting","comparison":self.comparison,"owner_name":self.owner_name,"endpoint":self.endpoint,"expires_in_ms":self.deadline.saturating_duration_since(tokio::time::Instant::now()).as_millis() as u64})
    }
    pub async fn finish(
        mut self,
        mut cancel: watch::Receiver<bool>,
    ) -> Result<RemotePeer, HostError> {
        let cancelled = || {
            HostError::new(
                "PairingCancelled",
                "Cancelled; owner may retain authorization if approval raced",
            )
        };
        if *cancel.borrow() {
            return Err(cancelled());
        }
        let result = tokio::select! {biased; _=cancel.changed()=>return Err(cancelled()), _=tokio::time::sleep_until(self.deadline)=>return Err(expired()), r=reply(&mut self.stream,&self.transcript.request_id,false)=>r?};
        let r: Completed = serde_json::from_value(result).map_err(|_| protocol())?;
        let t = &self.transcript;
        if r.version != 2
            || r.phase != "Completed"
            || r.request_id != t.request_id
            || r.nonce != t.nonce
            || r.ticket != t.ticket
            || r.peer_id != t.peer_id
            || r.owner_id != t.owner_id
            || r.credential.len() != 64
            || !r
                .credential
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(protocol());
        }
        Ok(RemotePeer {
            host_id: t.owner_id.clone(),
            name: self.nickname.unwrap_or(self.owner_name),
            endpoint: self.endpoint,
            fingerprint: t.fingerprint.clone(),
            peer_id: t.peer_id.clone(),
            credential: r.credential,
        })
    }
}
struct PendingGuard<'a> {
    trust: &'a Mutex<RemoteStore>,
    ticket: String,
}
impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        self.trust.lock().unwrap().discard_v2(&self.ticket);
    }
}
pub(crate) async fn serve_pair_v2(
    mut stream: tokio_rustls::server::TlsStream<tokio::net::TcpStream>,
    first: HostRequest,
    owner: PairOwner<'_>,
    source: IpAddr,
    admitted_generation: u64,
) -> Result<(), HostError> {
    let requested = (|| {
        let start: Start = serde_json::from_value(first.params.clone()).map_err(|_| protocol())?;
        if first.v != 1
            || first.method != "remote_pair_v2"
            || !valid_id(&first.id)
            || start.version != 2
            || !valid_id(&start.peer_id)
            || !valid_id(&start.nonce)
            || !valid_name(&start.name)
        {
            return Err(protocol());
        }
        let exporter = stream
            .get_ref()
            .1
            .export_keying_material([0u8; 32], b"EXPORTER-Channel-Binding", Some(b""))
            .map_err(|_| protocol())?;
        let mut store = owner.trust.lock().unwrap();
        if owner.stopped.load(Ordering::Acquire)
            || owner.generation.load(Ordering::Acquire) != admitted_generation
        {
            return Err(expired());
        }
        let t = PairTranscript {
            version: 2,
            request_id: first.id.clone(),
            nonce: start.nonce,
            ticket: new_id()?,
            peer_id: start.peer_id,
            owner_id: store.host_id().into(),
            fingerprint: store.fingerprint()?,
        };
        let public = store.request_v2(
            t.clone(),
            &start.name,
            source,
            admitted_generation,
            &exporter,
            Instant::now(),
        )?;
        Ok((t, public))
    })();
    let (t, public) = match requested {
        Ok(v) => v,
        Err(e) => {
            let _ = tokio::time::timeout(
                Duration::from_secs(10),
                write_frame(
                    &mut stream,
                    &HostReply::from_result(first.id, Err(e.clone())),
                ),
            )
            .await;
            return Err(e);
        }
    };
    let _guard = PendingGuard {
        trust: owner.trust,
        ticket: t.ticket.clone(),
    };
    let ack = json!({"version":2,"phase":"Waiting","request_id":t.request_id,"nonce":t.nonce,"ticket":t.ticket,"peer_id":t.peer_id,"owner_id":t.owner_id,"owner_name":owner.name,"expires_in_ms":public.expires_in_ms});
    tokio::time::timeout(
        Duration::from_secs(10),
        write_frame(
            &mut stream,
            &HostReply::from_result(first.id.clone(), Ok(ack)),
        ),
    )
    .await
    .map_err(|_| expired())??;
    // No second request is allowed. A cancellation-safe single-byte read detects
    // EOF or any unsolicited input without losing a partial length prefix.
    let mut extra = [0u8; 1];
    let result = loop {
        tokio::select! {biased;
            incoming=stream.read(&mut extra)=>{break Err(if matches!(incoming,Ok(0)){HostError::new("PairingCancelled","Requester disconnected")}else{protocol()});},
            _=tokio::time::sleep(Duration::from_millis(50))=>{let ready={let mut store=owner.trust.lock().unwrap();if owner.stopped.load(Ordering::Acquire)||owner.generation.load(Ordering::Acquire)!=admitted_generation{Err(expired())}else{store.take_approved_v2(&t.ticket,admitted_generation,Instant::now())}};
                match ready{Ok(None)=>continue,Err(e)=>break Err(e),Ok(Some(a))=>{let t=a.transcript;break Ok(json!({"version":2,"phase":"Completed","request_id":t.request_id,"nonce":t.nonce,"ticket":t.ticket,"peer_id":t.peer_id,"owner_id":t.owner_id,"credential":a.credential}));}}}
        }
    };
    tokio::time::timeout(
        Duration::from_secs(10),
        write_frame(&mut stream, &HostReply::from_result(first.id, result)),
    )
    .await
    .map_err(|_| {
        HostError::new(
            "PairingIncomplete",
            "Approval delivery timed out; authorization may remain",
        )
    })??;
    Ok(())
}

#[cfg(test)]
#[path = "pair_transport_tests.rs"]
mod tests;
