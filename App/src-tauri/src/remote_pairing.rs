use crate::host::contracts::{valid_id, HostError};
use serde::Serialize;
use std::{
    collections::{BTreeMap, VecDeque},
    net::IpAddr,
    time::{Duration, Instant},
};

pub const PAIR_TTL: Duration = Duration::from_secs(120);
#[derive(Clone, Debug, PartialEq)]
pub struct PairTranscript {
    pub version: u32,
    pub request_id: String,
    pub nonce: String,
    pub ticket: String,
    pub peer_id: String,
    pub owner_id: String,
    pub fingerprint: String,
}
impl PairTranscript {
    pub fn validate(&self) -> Result<(), HostError> {
        if self.version != 2
            || ![
                &self.request_id,
                &self.nonce,
                &self.ticket,
                &self.peer_id,
                &self.owner_id,
            ]
            .iter()
            .all(|s| valid_id(s))
            || self.peer_id == self.owner_id
            || self.fingerprint.len() != 64
            || !self
                .fingerprint
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(HostError::new(
                "PairingProtocol",
                "Invalid pairing transcript",
            ));
        }
        Ok(())
    }
}
pub fn comparison(exporter: &[u8; 32], t: &PairTranscript) -> Result<String, HostError> {
    t.validate()?;
    let version = t.version.to_be_bytes();
    let mut bytes = Vec::new();
    for part in [
        b"yang-lab-pair-v2".as_slice(),
        exporter.as_slice(),
        &version,
        t.request_id.as_bytes(),
        t.nonce.as_bytes(),
        t.ticket.as_bytes(),
        t.peer_id.as_bytes(),
        t.owner_id.as_bytes(),
        t.fingerprint.as_bytes(),
    ] {
        bytes.extend_from_slice(&(part.len() as u32).to_be_bytes());
        bytes.extend_from_slice(part);
    }
    let digest = ring::digest::digest(&ring::digest::SHA256, &bytes);
    let number = u32::from_be_bytes(digest.as_ref()[..4].try_into().unwrap()) % 1_000_000;
    Ok(format!("{number:06}"))
}
#[derive(Clone, Debug, Serialize)]
pub struct PendingPublic {
    pub id: String,
    pub request_id: String,
    pub peer_id: String,
    pub name: String,
    pub source_ip: String,
    pub comparison: String,
    pub expires_in_ms: u64,
}
#[derive(Clone, Debug)]
pub(crate) struct PairEntry {
    pub transcript: PairTranscript,
    pub name: String,
    source: IpAddr,
    comparison: String,
    generation: u64,
    deadline: Instant,
    state: PairState,
}
#[derive(Clone, Copy, Debug, PartialEq)]
enum PairState {
    Waiting,
    Approved,
    Rejected,
}
#[derive(Default)]
pub(crate) struct PairQueue {
    entries: BTreeMap<String, PairEntry>,
    sources: BTreeMap<IpAddr, VecDeque<Instant>>,
    global: VecDeque<Instant>,
}
impl PairQueue {
    pub fn clear(&mut self) {
        self.entries.clear();
    } // Rate windows survive listener changes.
    pub fn contains(&self, ticket: &str) -> bool {
        self.entries.contains_key(ticket)
    }
    fn prune(&mut self, now: Instant) {
        self.entries.retain(|_, e| now < e.deadline);
        self.global
            .retain(|t| now.saturating_duration_since(*t) < PAIR_TTL);
        self.sources.retain(|_, times| {
            times.retain(|t| now.saturating_duration_since(*t) < PAIR_TTL);
            !times.is_empty()
        });
    }
    pub fn insert(
        &mut self,
        t: PairTranscript,
        name: &str,
        source: IpAddr,
        generation: u64,
        exporter: &[u8; 32],
        now: Instant,
    ) -> Result<PendingPublic, HostError> {
        let cmp = comparison(exporter, &t)?;
        if name.trim().is_empty() || name.len() > 128 || name.chars().any(char::is_control) {
            return Err(HostError::new("PairingRejected", "Invalid computer name"));
        }
        self.prune(now);
        if self.entries.len() >= 4
            || self.entries.contains_key(&t.ticket)
            || self.entries.values().any(|e| {
                e.transcript.peer_id == t.peer_id
                    || e.transcript.request_id == t.request_id
                    || e.transcript.nonce == t.nonce
            })
        {
            return Err(HostError::new("PairingBusy", "Another request is pending"));
        }
        let source = match source {
            IpAddr::V6(ip) => ip.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(source),
            _ => source,
        };
        if self.global.len() >= 20
            || self.sources.get(&source).is_some_and(|v| v.len() >= 5)
            || (!self.sources.contains_key(&source) && self.sources.len() >= 32)
        {
            return Err(HostError::new(
                "PairingRate",
                "Too many connection requests; try later",
            ));
        }
        self.global.push_back(now);
        self.sources.entry(source).or_default().push_back(now);
        let e = PairEntry {
            transcript: t.clone(),
            name: name.into(),
            source,
            comparison: cmp,
            generation,
            deadline: now + PAIR_TTL,
            state: PairState::Waiting,
        };
        let public = e.public(now);
        self.entries.insert(t.ticket, e);
        Ok(public)
    }
    fn live(
        &mut self,
        ticket: &str,
        generation: u64,
        now: Instant,
    ) -> Result<&mut PairEntry, HostError> {
        self.prune(now);
        let e = self
            .entries
            .get_mut(ticket)
            .ok_or_else(|| HostError::new("PairingExpired", "Connection request ended"))?;
        if e.generation != generation {
            return Err(HostError::new("PairingExpired", "Listener changed"));
        }
        if e.state == PairState::Rejected {
            return Err(HostError::new(
                "PairingRejected",
                "Connection request rejected",
            ));
        }
        Ok(e)
    }
    pub fn approve(
        &mut self,
        ticket: &str,
        generation: u64,
        now: Instant,
    ) -> Result<(), HostError> {
        let e = self.live(ticket, generation, now)?;
        e.state = PairState::Approved;
        Ok(())
    }
    pub fn reject(&mut self, ticket: &str, generation: u64, now: Instant) -> Result<(), HostError> {
        let e = self.live(ticket, generation, now)?;
        e.state = PairState::Rejected;
        Ok(())
    }
    pub fn cancel(&mut self, ticket: &str, generation: u64, now: Instant) -> Result<(), HostError> {
        self.live(ticket, generation, now)?;
        self.entries.remove(ticket);
        Ok(())
    }
    pub fn remove(&mut self, ticket: &str) {
        self.entries.remove(ticket);
    }
    pub fn approved(
        &mut self,
        ticket: &str,
        generation: u64,
        now: Instant,
    ) -> Result<Option<PairEntry>, HostError> {
        let e = self.live(ticket, generation, now)?;
        Ok((e.state == PairState::Approved).then(|| e.clone()))
    }
    pub fn public(&self, now: Instant) -> Vec<PendingPublic> {
        self.entries
            .values()
            .filter(|e| now < e.deadline && e.state != PairState::Rejected)
            .map(|e| e.public(now))
            .collect()
    }
}
impl PairEntry {
    fn public(&self, now: Instant) -> PendingPublic {
        PendingPublic {
            id: self.transcript.ticket.clone(),
            request_id: self.transcript.request_id.clone(),
            peer_id: self.transcript.peer_id.clone(),
            name: self.name.clone(),
            source_ip: self.source.to_string(),
            comparison: self.comparison.clone(),
            expires_in_ms: self.deadline.saturating_duration_since(now).as_millis() as u64,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn transcript(n: u128) -> PairTranscript {
        PairTranscript {
            version: 2,
            request_id: format!("{n:032x}"),
            nonce: format!("{:032x}", n + 3000),
            ticket: format!("{:032x}", n + 1000),
            peer_id: format!("{:032x}", n + 2000),
            owner_id: "e".repeat(32),
            fingerprint: "f".repeat(64),
        }
    }
    #[test]
    fn pairing_comparison_vectors() {
        let t = PairTranscript {
            version: 2,
            request_id: "a".repeat(32),
            nonce: "b".repeat(32),
            ticket: "c".repeat(32),
            peer_id: "d".repeat(32),
            owner_id: "e".repeat(32),
            fingerprint: "f".repeat(64),
        };
        assert_eq!(comparison(&[1; 32], &t).unwrap(), "919680");
        assert_eq!(comparison(&[2; 32], &t).unwrap(), "096479");
        let mut bad = t.clone();
        bad.nonce = "b".repeat(31);
        assert!(comparison(&[1; 32], &bad).is_err());
        bad = t.clone();
        bad.version = 1;
        assert!(comparison(&[1; 32], &bad).is_err());
        bad = t;
        bad.fingerprint = "F".repeat(64);
        assert!(comparison(&[1; 32], &bad).is_err());
    }
    #[test]
    fn pairing_bounds_and_terminal_states() {
        let now = Instant::now();
        let ip = "127.0.0.1".parse().unwrap();
        let mut q = PairQueue::default();
        for n in 0..4 {
            q.insert(transcript(n), "PC", ip, 7, &[1; 32], now).unwrap();
        }
        assert!(q.insert(transcript(5), "PC", ip, 7, &[1; 32], now).is_err());
        assert!(q.insert(transcript(0), "PC", ip, 7, &[1; 32], now).is_err());
        q.cancel(&transcript(0).ticket, 7, now).unwrap();
        q.insert(transcript(5), "PC", ip, 7, &[1; 32], now).unwrap();
        q.cancel(&transcript(5).ticket, 7, now).unwrap();
        assert_eq!(
            q.insert(transcript(6), "PC", ip, 7, &[1; 32], now)
                .unwrap_err()
                .code,
            "PairingRate"
        );
        assert!(q.approve(&transcript(0).ticket, 7, now).is_err());
        q.reject(&transcript(1).ticket, 7, now).unwrap();
        assert_eq!(
            q.approved(&transcript(1).ticket, 7, now).unwrap_err().code,
            "PairingRejected"
        );
        assert!(q.approve(&transcript(2).ticket, 8, now).is_err());
        assert!(q
            .approved(&transcript(2).ticket, 7, now + Duration::from_secs(120))
            .is_err());
        assert!(q
            .insert(
                transcript(6),
                &"中".repeat(43),
                ip,
                7,
                &[1; 32],
                now + Duration::from_secs(120)
            )
            .is_err());
        q.insert(
            transcript(6),
            "PC",
            ip,
            7,
            &[1; 32],
            now + Duration::from_secs(120),
        )
        .unwrap();
        let mut global = PairQueue::default();
        for n in 0..20 {
            let source = format!("100.65.0.{}", n + 1).parse().unwrap();
            let t = transcript(n);
            global
                .insert(t.clone(), "PC", source, 7, &[1; 32], now)
                .unwrap();
            global.cancel(&t.ticket, 7, now).unwrap();
        }
        assert_eq!(
            global
                .insert(
                    transcript(21),
                    "PC",
                    "100.65.2.2".parse().unwrap(),
                    7,
                    &[1; 32],
                    now
                )
                .unwrap_err()
                .code,
            "PairingRate"
        );
        global
            .insert(
                transcript(21),
                "PC",
                ip,
                7,
                &[1; 32],
                now + Duration::from_secs(120),
            )
            .unwrap();
        // The production global cap makes 32 current sources unreachable organically;
        // directly seed private bounded bookkeeping to prove fail-closed capacity.
        let mut full = PairQueue::default();
        for n in 1..=32 {
            full.sources.insert(
                format!("100.65.1.{n}").parse().unwrap(),
                VecDeque::from([now]),
            );
        }
        assert!(full
            .insert(transcript(30), "PC", ip, 7, &[1; 32], now)
            .is_err());
        assert_eq!(full.sources.len(), 32);
        full.insert(
            transcript(30),
            "PC",
            ip,
            7,
            &[1; 32],
            now + Duration::from_secs(120),
        )
        .unwrap();
        let mut mapped = PairQueue::default();
        for n in 0..5 {
            let t = transcript(n);
            mapped
                .insert(t.clone(), "PC", ip, 7, &[1; 32], now)
                .unwrap();
            mapped.cancel(&t.ticket, 7, now).unwrap();
        }
        assert!(mapped
            .insert(
                transcript(8),
                "PC",
                "::ffff:127.0.0.1".parse().unwrap(),
                7,
                &[1; 32],
                now
            )
            .is_err());
    }
}
