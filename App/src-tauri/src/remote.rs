#[cfg(test)]
#[path = "remote_tests.rs"]
mod tests;

use crate::host::{contracts::{valid_id, HostError}, registry::{new_id, write_atomic}, verification::sha256_bytes};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::BTreeMap, net::{IpAddr, SocketAddr}, path::PathBuf, sync::Arc, time::{Duration, Instant}};
use rustls::{pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime}, client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier}, DigitallySignedStruct, SignatureScheme};

pub fn endpoint(value: &str) -> Result<SocketAddr, HostError> {
    let addr: SocketAddr = value.parse().map_err(|_| HostError::new("RemoteEndpoint", "Use a loopback or Tailscale IP and port"))?;
    let allowed = match addr.ip() {
        IpAddr::V4(ip) => { let b = ip.octets(); ip.is_loopback() || (b[0] == 100 && (64..128).contains(&b[1])) },
        IpAddr::V6(ip) => ip.is_loopback() || ip.segments()[..3] == [0xfd7a, 0x115c, 0xa1e0],
    };
    if !allowed || addr.port() == 0 { return Err(HostError::new("RemoteEndpoint", "Only explicit loopback/Tailscale endpoints are allowed")); }
    Ok(addr)
}

/// Whole-file DPAPI protection, user-bound and atomic. No key/token is serialized to web state.
pub(crate) fn protect(bytes: &[u8], encrypt: bool) -> Result<Vec<u8>, HostError> {
    use windows_sys::Win32::{Security::Cryptography::{CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN}, Foundation::LocalFree};
    if bytes.len() > 256 * 1024 { return Err(HostError::new("RemoteTrust", "Trust file too large")); }
    let input = CRYPT_INTEGER_BLOB { cbData: bytes.len() as u32, pbData: bytes.as_ptr() as *mut u8 };
    let mut output = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
    let ok = unsafe { if encrypt {
        CryptProtectData(&input, std::ptr::null(), std::ptr::null(), std::ptr::null_mut(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut output)
    } else {
        CryptUnprotectData(&input, std::ptr::null_mut(), std::ptr::null(), std::ptr::null_mut(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut output)
    }};
    if ok == 0 { return Err(HostError::new("RemoteTrust", "Windows credential protection failed")); }
    let result = unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe { std::ptr::write_bytes(output.pbData, 0, output.cbData as usize); LocalFree(output.pbData as _); }
    Ok(result)
}
pub(crate) fn load_secret<T: serde::de::DeserializeOwned>(path: &std::path::Path) -> Result<T, HostError> {
    let meta = std::fs::metadata(path).map_err(|_| HostError::new("RemoteTrust", "Trust file unavailable"))?;
    if meta.len() > 256 * 1024 { return Err(HostError::new("RemoteTrust", "Trust file too large")); }
    let encrypted = std::fs::read(path).map_err(|_| HostError::new("RemoteTrust", "Trust file unreadable"))?;
    serde_json::from_slice(&protect(&encrypted, false)?).map_err(|_| HostError::new("RemoteTrust", "Invalid encrypted trust file"))
}
pub(crate) fn save_secret<T: Serialize>(path: &std::path::Path, value: &T) -> Result<(), HostError> {
    let bytes = serde_json::to_vec(value).map_err(|_| HostError::new("RemoteTrust", "Cannot serialize trust"))?;
    write_atomic(path, &protect(&bytes, true)?)
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Peer { name: String, credential: String }
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Trust {
    version: u32, host_id: String, certificate: Vec<u8>, key: Vec<u8>, listener: Option<String>, peers: BTreeMap<String, Peer>,
    #[serde(default)] released_boots: Vec<String>,
}
struct Pairing { code: String, deadline: Instant, failures: u8 }
struct Pending { peer: String, name: String, deadline: Instant, approved: bool }
pub struct RemoteStore { path: PathBuf, trust: Trust, pairing: Option<Pairing>, pending: BTreeMap<String, Pending> }
pub(crate) fn authenticated_admission<T>(remote:&std::sync::Mutex<RemoteStore>,generation:&std::sync::atomic::AtomicU64,expected:u64,peer:&str,credential:&str,insert:impl FnOnce()->Result<T,HostError>)->Result<T,HostError> {
    let trust=remote.lock().unwrap();trust.authenticate(peer,credential)?;
    if generation.load(std::sync::atomic::Ordering::Acquire)!=expected {return Err(HostError::new("PeerRejected","Listener changed"));}
    let result=insert();drop(trust);result
}
impl RemoteStore {
    pub fn open(path: PathBuf, host_id: String) -> Result<Self, HostError> {
        if !valid_id(&host_id) { return Err(HostError::new("RemoteTrust", "Invalid Host identity")); }
        let trust = if path.exists() { load_secret::<Trust>(&path)? } else {
            let rcgen::CertifiedKey { cert, signing_key } = rcgen::generate_simple_self_signed(vec!["yang-lab".into()])
                .map_err(|_| HostError::new("RemoteTls", "Certificate generation failed"))?;
            Trust { version: 1, host_id: host_id.clone(), certificate: cert.der().to_vec(), key: signing_key.serialize_der(), listener: None, peers: BTreeMap::new(), released_boots:vec![] }
        };
        if trust.version != 1 || trust.host_id != host_id || trust.peers.len() > 16 || trust.released_boots.len()>16 || trust.released_boots.iter().any(|b|!valid_id(b)) {
            return Err(HostError::new("RemoteTrust", "Trust belongs to another Host or unsupported version"));
        }
        if let Some(ref e) = trust.listener { endpoint(e)?; }
        let store = Self { path, trust, pairing: None, pending: BTreeMap::new() };
        store.acceptor()?; // corrupt certificate/key fails before any bind
        save_secret(&store.path, &store.trust)?;
        Ok(store)
    }
    pub fn fingerprint(&self) -> Result<String, HostError> { sha256_bytes(&self.trust.certificate) }
    pub fn host_id(&self) -> &str { &self.trust.host_id }
    /// Native-only capability for querying this peer's old session, never for acquiring control.
    pub(crate) fn release_token(&self,peer:&str,boot:&str,session:&str)->Result<String,HostError> {
        if ![peer,boot,session].iter().all(|v|valid_id(v)) {return Err(HostError::new("ReleaseIdentity","Invalid release identity"));}
        let key=ring::hmac::Key::new(ring::hmac::HMAC_SHA256,&self.trust.key);
        let message=format!("yang-lab-release-v1|{}|{peer}|{boot}|{session}",self.host_id());
        Ok(ring::hmac::sign(&key,message.as_bytes()).as_ref().iter().map(|b|format!("{b:02x}")).collect())
    }
    pub(crate) fn verify_release(&self,peer:&str,boot:&str,session:&str,token:&str)->Result<(),HostError> {
        let expected=self.release_token(peer,boot,session)?;
        #[allow(deprecated)]
        ring::constant_time::verify_slices_are_equal(expected.as_bytes(),token.as_bytes()).map_err(|_|HostError::new("ReleaseIdentity","Release proof belongs to another peer, boot or session"))
    }
    pub(crate) fn record_release(&mut self,boot:&str,report:&Value)->Result<(),HostError> {
        if !valid_id(boot)||report["resource_released"]!=true||report["process_exit"]["confirmed"]!=true||report["process_exit"]["success"]!=true {
            return Err(HostError::new("ReleaseUnknown","No verified Host resource-release evidence"));
        }
        let mut next=self.trust.clone();next.released_boots.retain(|b|b!=boot);next.released_boots.push(boot.to_owned());
        if next.released_boots.len()>16 {next.released_boots.remove(0);}
        save_secret(&self.path,&next)?;self.trust=next;Ok(())
    }
    pub(crate) fn released_boot(&self,boot:&str)->Option<Value> {
        self.trust.released_boots.iter().any(|b|b==boot).then(||json!({"resource_released":true,"process_exit":{"confirmed":true,"success":true},"host_stopped":true,"physical_zero_verified":false}))
    }
    pub fn listener(&self) -> Option<String> { self.trust.listener.clone() }
    pub fn set_listener(&mut self, value: Option<String>) -> Result<(), HostError> {
        if let Some(ref e) = value { endpoint(e)?; }
        let mut next = self.trust.clone(); next.listener = value;
        save_secret(&self.path, &next)?; self.trust = next; self.pairing = None; self.pending.clear(); Ok(())
    }
    pub fn status(&self) -> Value {
        json!({"host_id":self.trust.host_id,"fingerprint":self.fingerprint().ok(),"listener":self.trust.listener,
            "peers":self.trust.peers.iter().map(|(id,p)|json!({"id":id,"name":p.name})).collect::<Vec<_>>(),
            "pending":self.pending.iter().filter(|(_,p)|Instant::now()<p.deadline).map(|(id,p)|json!({"id":id,"peer_id":p.peer,"name":p.name})).collect::<Vec<_>>()})
    }
    pub fn begin_pairing(&mut self) -> Result<String, HostError> {
        let random = new_id()?;
        let code = format!("{:06}", u32::from_str_radix(&random[..8],16).unwrap() % 1_000_000);
        self.pairing = Some(Pairing { code: code.clone(), deadline: Instant::now()+Duration::from_secs(120), failures: 0 });
        self.pending.clear(); Ok(code)
    }
    pub fn request_pair(&mut self, peer: &str, name: &str, code: &str) -> Result<String, HostError> {
        let pair = self.pairing.as_mut().filter(|p| Instant::now()<p.deadline && p.failures<5)
            .ok_or_else(|| HostError::new("PairingClosed", "Open pairing on the owning Host first"))?;
        if code != pair.code { pair.failures += 1; return Err(HostError::new("PairingRejected", "Pairing code rejected")); }
        if !valid_id(peer) || peer == self.trust.host_id || name.is_empty() || name.len()>128 || self.pending.len()>=16 || self.trust.peers.len()>=16 {
            return Err(HostError::new("PairingRejected", "Invalid or excess peer"));
        }
        let ticket = new_id()?;
        self.pending.insert(ticket.clone(), Pending { peer: peer.into(), name: name.into(), deadline: pair.deadline, approved: false });
        Ok(ticket)
    }
    pub fn approve(&mut self, ticket: &str) -> Result<(), HostError> {
        self.pending.get_mut(ticket).filter(|p| Instant::now()<p.deadline).ok_or_else(|| HostError::new("PairingExpired", "Pair request expired"))?.approved = true; Ok(())
    }
    pub fn take_approved(&mut self, ticket: &str) -> Result<Option<String>, HostError> {
        let pending = self.pending.get(ticket).filter(|p|Instant::now()<p.deadline)
            .ok_or_else(||HostError::new("PairingExpired", "Pair request expired"))?;
        if !pending.approved { return Ok(None); }
        let credential = format!("{}{}",new_id()?,new_id()?);
        let mut next = self.trust.clone();
        next.peers.insert(pending.peer.clone(),Peer { name: pending.name.clone(), credential: credential.clone() });
        save_secret(&self.path, &next)?; self.trust = next;
        self.pending.remove(ticket); self.pairing = None; self.pending.clear();
        Ok(Some(credential))
    }
    pub fn authenticate(&self, peer: &str, credential: &str) -> Result<(), HostError> {
        let known = self.trust.peers.get(peer).ok_or_else(||HostError::new("PeerRejected", "Unknown or revoked peer"))?;
        #[allow(deprecated)]
        let valid = ring::constant_time::verify_slices_are_equal(known.credential.as_bytes(),credential.as_bytes()).is_ok();
        if !valid { return Err(HostError::new("PeerRejected", "Peer authentication rejected")); }
        Ok(())
    }
    pub fn revoke(&mut self, peer: &str) -> Result<(), HostError> {
        let mut next = self.trust.clone(); next.peers.remove(peer); save_secret(&self.path,&next)?; self.trust=next; Ok(())
    }
    pub fn acceptor(&self) -> Result<tokio_rustls::TlsAcceptor, HostError> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let config = rustls::ServerConfig::builder_with_provider(provider).with_protocol_versions(&[&rustls::version::TLS13])
            .map_err(|_|HostError::new("RemoteTls","TLS version unavailable"))?.with_no_client_auth()
            .with_single_cert(vec![CertificateDer::from(self.trust.certificate.clone())],PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.trust.key.clone())))
            .map_err(|_|HostError::new("RemoteTls","Invalid Host certificate"))?;
        Ok(tokio_rustls::TlsAcceptor::from(Arc::new(config)))
    }
}

#[derive(Debug)]
struct PinnedCertificate(String);
impl ServerCertVerifier for PinnedCertificate {
    fn verify_server_cert(&self, cert: &CertificateDer<'_>, _: &[CertificateDer<'_>], _: &ServerName<'_>, _: &[u8], _: UnixTime) -> Result<ServerCertVerified,rustls::Error> {
        if sha256_bytes(cert.as_ref()).ok().as_deref() != Some(&self.0) { return Err(rustls::Error::General("Host certificate fingerprint changed".into())); }
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid,rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &rustls::crypto::ring::default_provider().signature_verification_algorithms)
    }
    fn verify_tls13_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid,rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &rustls::crypto::ring::default_provider().signature_verification_algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> { rustls::crypto::ring::default_provider().signature_verification_algorithms.supported_schemes() }
}
pub type ClientStream = tokio_rustls::client::TlsStream<tokio::net::TcpStream>;
/// Remote peers may operate devices, but only the owner's local GUI may change configuration.
pub fn remote_method(method: &str) -> bool {
    matches!(method, "ping" | "catalog" | "snapshot" | "subscribe" | "worker_status" | "request_snapshot" |
        "acquire_control" | "renew_control" | "release_control" | "safe_stop" | "close_client" | "reconcile_client" |
        "prepare" | "execute" | "operation" | "read_result" | "list_archives" | "read_archive" |
        "archive_manifest" | "archive_manifest_bytes")
}
pub async fn connect_tls(address: &str, fingerprint: &str) -> Result<ClientStream,HostError> {
    let address = endpoint(address)?;
    if fingerprint.len()!=64 || !fingerprint.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) {
        return Err(HostError::new("RemotePin", "Copy the full SHA256 fingerprint from the owning Host"));
    }
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_protocol_versions(&[&rustls::version::TLS13]).map_err(|_|HostError::new("RemoteTls","TLS version unavailable"))?
        .dangerous().with_custom_certificate_verifier(Arc::new(PinnedCertificate(fingerprint.into()))).with_no_client_auth();
    tokio::time::timeout(Duration::from_secs(10), async {
        let tcp = tokio::net::TcpStream::connect(address).await.map_err(|_|HostError::new("RemoteOffline","Remote Host is unreachable"))?;
        tokio_rustls::TlsConnector::from(Arc::new(config)).connect(ServerName::try_from("yang-lab").unwrap(),tcp)
            .await.map_err(|_|HostError::new("RemoteTls", "TLS handshake or certificate pin verification failed"))
    }).await.map_err(|_|HostError::new("RemoteOffline","Remote connection timed out"))?
}
