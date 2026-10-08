//! Connection verification is separate from durable registration.
use super::catalog::{Catalog, DOCUMENT};
use super::contracts::*;
use super::registry::{new_id, Registry, RegistryChange, ReleasePermit};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Consent {
    pub accepted: bool,
    pub mode: String,
    pub config_digest: String,
    pub open_effects: Vec<String>,
    pub supervised: bool,
    pub retain_session: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeEvidence {
    pub draft_id: String,
    pub model_id: String,
    pub profile_id: String,
    pub config_digest: String,
    pub config_rev: u64,
    pub identity: Value,
    pub mode: String,
    pub probe_version: u64,
    pub release_confirmed: bool,
    pub retained_session: bool,
    pub controller: Option<String>,
}
#[derive(Clone, Debug, Serialize)]
pub struct OperationRecord {
    pub operation_id: String,
    pub draft_id: String,
    pub status: String,
    pub proof_id: Option<String>,
}
/// Trusted transport boundary. No public GUI command can supply ProbeEvidence.
pub trait VerificationPort: Send {
    fn published(&mut self, _evidence: &ProbeEvidence) -> Result<(), HostError> {
        Ok(())
    }
    fn probe(&mut self, draft: &DraftRecord, consent: &Consent)
        -> Result<ProbeEvidence, HostError>;
    fn retained_controller_valid(&self, evidence: &ProbeEvidence) -> bool;
    fn cancel(&mut self, draft: &DraftRecord) -> Result<String, HostError>;
}
pub struct UnavailablePort;
impl VerificationPort for UnavailablePort {
    fn probe(&mut self, _: &DraftRecord, _: &Consent) -> Result<ProbeEvidence, HostError> {
        Err(HostError::new(
            "VerificationUnavailable",
            "Host leases and durable dispatch must be installed first",
        ))
    }
    fn retained_controller_valid(&self, _: &ProbeEvidence) -> bool {
        false
    }
    fn cancel(&mut self, _: &DraftRecord) -> Result<String, HostError> {
        Err(HostError::new(
            "ReleaseRequired",
            "Worker release has not been confirmed",
        ))
    }
}
#[derive(Clone)]
pub struct VerifiedProof {
    pub evidence: ProbeEvidence,
    issued: Instant,
    consumed: bool,
}
impl VerifiedProof {
    pub fn valid_at(&self, now: Instant) -> bool {
        !self.consumed
            && now
                .checked_duration_since(self.issued)
                .is_some_and(|age| age < Duration::from_secs(60))
    }
    pub fn matches(&self, mode: &str, revision: u64, digest: &str) -> Result<(), HostError> {
        if mode != "real"
            || self.evidence.mode != "real"
            || self.evidence.config_rev != revision
            || self.evidence.config_digest != digest
        {
            return Err(HostError::new(
                "StaleProof",
                "Verification does not match current draft",
            ));
        }
        Ok(())
    }
}
pub struct Verifier {
    pub registry: Registry,
    port: Box<dyn VerificationPort>,
    proofs: BTreeMap<String, VerifiedProof>,
    clock: Box<dyn Fn() -> Instant + Send>,
}
impl Verifier {
    pub fn new(registry: Registry, port: Box<dyn VerificationPort>) -> Self {
        Self {
            registry,
            port,
            proofs: BTreeMap::new(),
            clock: Box::new(Instant::now),
        }
    }
    pub fn create_draft(
        &mut self,
        model: &str,
        profile: &str,
        params: Value,
        name: &str,
        mode: &str,
        expected_rev: u64,
    ) -> Result<DraftRecord, HostError> {
        if mode != "real" {
            return Err(HostError::new(
                "ModeInvalid",
                "Only real hardware verification is supported",
            ));
        }
        let params = Catalog::load(DOCUMENT)?.validate_connection(model, profile, &params)?;
        let mut draft = DraftRecord {
            device_id: new_id()?,
            name: name.into(),
            model_id: Some(model.into()),
            profile_id: Some(profile.into()),
            params,
            config_digest: String::new(),
            revision: 1,
            mode: mode.into(),
            status: "Draft".into(),
        };
        draft.config_digest = draft_digest(&draft)?;
        self.registry
            .commit(expected_rev, RegistryChange::SaveDraft(draft.clone()))?;
        Ok(draft)
    }
    pub fn update_draft(
        &mut self,
        mut draft: DraftRecord,
        expected_rev: u64,
    ) -> Result<DraftRecord, HostError> {
        let snapshot = self.registry.snapshot()?;
        let current = snapshot
            .drafts
            .iter()
            .find(|value| value.device_id == draft.device_id)
            .ok_or_else(|| HostError::new("DraftUnknown", "Draft not found"))?;
        if current.status == "Cancelled" || draft.revision != current.revision + 1 {
            return Err(HostError::new("ConfigConflict", "Draft revision changed"));
        }
        if self.proofs.values().any(|proof| {
            proof.evidence.draft_id == draft.device_id
                && proof.evidence.retained_session
                && self.port.retained_controller_valid(&proof.evidence)
        }) {
            return Err(HostError::new(
                "ReleaseRequired",
                "End supervised responsibility before editing",
            ));
        }
        let model = draft
            .model_id
            .as_deref()
            .ok_or_else(|| HostError::new("DriverRequired", "Select reviewed model"))?;
        let profile = draft
            .profile_id
            .as_deref()
            .ok_or_else(|| HostError::new("DriverRequired", "Select reviewed profile"))?;
        draft.params =
            Catalog::load(DOCUMENT)?.validate_connection(model, profile, &draft.params)?;
        draft.config_digest = draft_digest(&draft)?;
        self.registry
            .commit(expected_rev, RegistryChange::SaveDraft(draft.clone()))?;
        self.proofs
            .retain(|_, proof| proof.evidence.draft_id != draft.device_id);
        Ok(draft)
    }
    pub fn begin(
        &mut self,
        draft_id: &str,
        expected_rev: u64,
        consent: Consent,
    ) -> Result<OperationRecord, HostError> {
        let snapshot = self.registry.snapshot()?;
        if snapshot.registry_rev != expected_rev {
            return Err(HostError::new(
                "ConfigConflict",
                "Registry revision changed",
            ));
        }
        let draft = snapshot
            .drafts
            .iter()
            .find(|value| value.device_id == draft_id && value.status != "Cancelled")
            .ok_or_else(|| HostError::new("DraftUnknown", "Draft not found"))?
            .clone();
        let catalog = Catalog::load(DOCUMENT)?;
        let profile = catalog.profile(
            draft.model_id.as_deref().unwrap_or(""),
            draft.profile_id.as_deref().unwrap_or(""),
        )?;
        if !consent.accepted
            || consent.mode != draft.mode
            || consent.config_digest != draft.config_digest
            || consent.open_effects != profile.open_effects
        {
            return Err(HostError::new(
                "ConsentRequired",
                "Consent must bind the current profile and disclosed open effects",
            ));
        }
        if (profile.probe_mode == "supervised") != consent.supervised
            || (consent.retain_session && !consent.supervised)
        {
            return Err(HostError::new(
                "ManualVerificationRequired",
                "Use the separate controlled supervised workflow",
            ));
        }
        let start = (self.clock)();
        let evidence = self.port.probe(&draft, &consent)?;
        let model = catalog.model(draft.model_id.as_deref().unwrap_or(""))?;
        let reported = evidence
            .identity
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("");
        let model_matches = if model.driver_kind == "osa" {
            reported.to_ascii_uppercase().starts_with("AQ6370")
        } else {
            reported.eq_ignore_ascii_case(&model.name)
        };
        if !model_matches {
            return Err(HostError::new(
                "IdentityMismatch",
                format!("Expected {}; the instrument reported {}. Check the selected instrument and port.", model.name, reported),
            ));
        }
        if !super::registry::identity_valid(&evidence.identity) {
            return Err(HostError::new("IdentityMismatch", "The instrument model matched, but its identity evidence has an unsupported format."));
        }
        if evidence.draft_id != draft.device_id
            || evidence.model_id != draft.model_id.as_deref().unwrap_or("")
            || evidence.profile_id != draft.profile_id.as_deref().unwrap_or("")
            || evidence.probe_version != profile.probe_version
            || !evidence.identity.is_object()
            || evidence
                .identity
                .as_object()
                .map_or(true, |value| value.is_empty())
        {
            return Err(HostError::new(
                "IdentityMismatch",
                "Probe evidence does not match this model/profile/draft",
            ));
        }
        let proof = VerifiedProof {
            evidence,
            issued: start,
            consumed: false,
        };
        proof.matches(&draft.mode, draft.revision, &draft.config_digest)?;
        if (!proof.evidence.release_confirmed && !proof.evidence.retained_session)
            || (proof.evidence.retained_session
                && (!consent.retain_session
                    || proof.evidence.controller.is_none()
                    || !self.port.retained_controller_valid(&proof.evidence)))
        {
            return Err(HostError::new(
                "ReleaseRequired",
                "Release or a retained lawful controller must be confirmed",
            ));
        }
        if !proof.valid_at((self.clock)()) {
            return Err(HostError::new(
                "StaleProof",
                "Probe took longer than evidence lifetime",
            ));
        }
        let proof_id = new_id()?;
        self.proofs
            .retain(|_, old| old.evidence.draft_id != draft.device_id);
        self.proofs.insert(proof_id.clone(), proof);
        Ok(OperationRecord {
            operation_id: new_id()?,
            draft_id: draft_id.into(),
            status: "Completed".into(),
            proof_id: Some(proof_id),
        })
    }
    pub fn save(
        &mut self,
        draft_id: &str,
        proof_id: &str,
        expected_rev: u64,
    ) -> Result<DeviceRecord, HostError> {
        let snapshot = self.registry.snapshot()?;
        if snapshot.registry_rev != expected_rev {
            return Err(HostError::new(
                "ConfigConflict",
                "Registry revision changed",
            ));
        }
        let draft = snapshot
            .drafts
            .iter()
            .find(|value| value.device_id == draft_id && value.status != "Cancelled")
            .ok_or_else(|| HostError::new("DraftUnknown", "Draft not found"))?;
        let proof = self
            .proofs
            .get(proof_id)
            .ok_or_else(|| HostError::new("StaleProof", "Verification not found"))?;
        if proof.evidence.draft_id != draft_id || !proof.valid_at((self.clock)()) {
            return Err(HostError::new(
                "StaleProof",
                "Verification expired, consumed, or belongs to another draft",
            ));
        }
        proof.matches(&draft.mode, draft.revision, &draft.config_digest)?;
        if proof.evidence.retained_session && !self.port.retained_controller_valid(&proof.evidence)
        {
            return Err(HostError::new(
                "ReleaseRequired",
                "Supervised controller authority ended",
            ));
        }
        let catalog = Catalog::load(DOCUMENT)?;
        let model = catalog.model(draft.model_id.as_deref().unwrap_or(""))?;
        let profile = catalog.profile(&model.id, draft.profile_id.as_deref().unwrap_or(""))?;
        if proof.evidence.model_id != model.id
            || proof.evidence.profile_id != profile.id
            || proof.evidence.probe_version != profile.probe_version
        {
            return Err(HostError::new(
                "StaleProof",
                "Driver/profile version changed",
            ));
        }
        let serial = proof
            .evidence
            .identity
            .get("serial")
            .and_then(Value::as_str);
        let device = DeviceRecord {
            device_id: draft.device_id.clone(),
            name: draft.name.clone(),
            category: model.category.clone(),
            model_id: model.id.clone(),
            driver_id: model.driver_id.clone(),
            profile_id: profile.id.clone(),
            params: draft.params.clone(),
            expected_identity: proof.evidence.identity.clone(),
            identity_strength: if proof.evidence.retained_session {
                "operator_bound"
            } else if serial.is_some() {
                "strong"
            } else {
                "transport_bound"
            }
            .into(),
            config_rev: 1,
            verified_mode: proof.evidence.mode.clone(),
            check_policy: CheckPolicy::default(),
        };
        self.registry
            .commit(expected_rev, RegistryChange::SaveDevice(device.clone()))?;
        self.port.published(&proof.evidence)?;
        self.proofs.get_mut(proof_id).unwrap().consumed = true;
        Ok(device)
    }
    pub fn cancel(&mut self, draft_id: &str, expected_rev: u64) -> Result<(), HostError> {
        let snapshot = self.registry.snapshot()?;
        if snapshot.registry_rev != expected_rev {
            return Err(HostError::new(
                "ConfigConflict",
                "Registry revision changed",
            ));
        }
        let draft = snapshot
            .drafts
            .iter()
            .find(|value| value.device_id == draft_id && value.status != "Cancelled")
            .ok_or_else(|| HostError::new("DraftUnknown", "Draft not found"))?;
        let attempt = self.port.cancel(draft)?;
        let permit = ReleasePermit::confirmed(draft.device_id.clone(), draft.revision, attempt);
        self.registry.commit(
            expected_rev,
            RegistryChange::CancelDraft {
                device_id: draft.device_id.clone(),
                config_rev: draft.revision,
                permit,
            },
        )?;
        self.proofs
            .retain(|_, proof| proof.evidence.draft_id != draft_id);
        Ok(())
    }
}
pub fn draft_digest(draft: &DraftRecord) -> Result<String, HostError> {
    canonical_digest(&json!({"device_id":draft.device_id,"name":draft.name,
        "model_id":draft.model_id,"profile_id":draft.profile_id,"params":draft.params,
        "revision":draft.revision,"mode":draft.mode}))
}
pub fn canonical_digest(value: &Value) -> Result<String, HostError> {
    fn sorted(value: &Value) -> Value {
        match value {
            Value::Object(items) => {
                let mut keys = items.keys().collect::<Vec<_>>();
                keys.sort();
                Value::Object(
                    keys.into_iter()
                        .map(|key| (key.clone(), sorted(&items[key])))
                        .collect(),
                )
            }
            Value::Array(items) => Value::Array(items.iter().map(sorted).collect()),
            other => other.clone(),
        }
    }
    let bytes = serde_json::to_vec(&sorted(value))
        .map_err(|error| HostError::new("DigestInvalid", error.to_string()))?;
    sha256_bytes(&bytes)
}
pub(crate) fn sha256_bytes(bytes: &[u8]) -> Result<String, HostError> {
    let input_len = u32::try_from(bytes.len())
        .map_err(|_| HostError::new("DigestInvalid", "Input too large"))?;
    #[cfg(windows)]
    {
        use windows_sys::Win32::Security::Cryptography::*;
        let mut handle = std::ptr::null_mut();
        // SAFETY: valid output pointer and static nul-terminated algorithm identifier.
        let opened = unsafe {
            BCryptOpenAlgorithmProvider(&mut handle, BCRYPT_SHA256_ALGORITHM, std::ptr::null(), 0)
        };
        if opened < 0 {
            return Err(HostError::new(
                "DigestUnavailable",
                "Windows SHA-256 unavailable",
            ));
        }
        let mut output = [0_u8; 32];
        // SAFETY: bounded initialized input/output slices, no secret key.
        let result = unsafe {
            BCryptHash(
                handle,
                std::ptr::null(),
                0,
                bytes.as_ptr(),
                input_len,
                output.as_mut_ptr(),
                32,
            )
        };
        // SAFETY: handle returned by the successful provider open.
        unsafe { BCryptCloseAlgorithmProvider(handle, 0) };
        if result < 0 {
            return Err(HostError::new(
                "DigestUnavailable",
                "Windows SHA-256 failed",
            ));
        }
        Ok(output.iter().map(|byte| format!("{byte:02x}")).collect())
    }
    #[cfg(not(windows))]
    {
        Err(HostError::new(
            "UnsupportedPlatform",
            "Windows CNG is required",
        ))
    }
}

#[cfg(test)]
mod tests {
    struct TestFolder(std::path::PathBuf);
    impl Drop for TestFolder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    struct FakePort {
        retained: bool,
    }
    impl VerificationPort for FakePort {
        fn probe(&mut self, draft: &DraftRecord, _: &Consent) -> Result<ProbeEvidence, HostError> {
            let catalog = Catalog::load(DOCUMENT)?;
            let model = catalog.model(draft.model_id.as_deref().unwrap())?;
            let profile = catalog.profile(&model.id, draft.profile_id.as_deref().unwrap())?;
            Ok(ProbeEvidence {
                draft_id: draft.device_id.clone(),
                model_id: model.id.clone(),
                profile_id: profile.id.clone(),
                config_digest: draft.config_digest.clone(),
                config_rev: draft.revision,
                identity: json!({"model":model.name,"serial":"SN"}),
                mode: draft.mode.clone(),
                probe_version: profile.probe_version,
                release_confirmed: !self.retained,
                retained_session: self.retained,
                controller: self.retained.then(|| "controller".into()),
            })
        }
        fn retained_controller_valid(&self, evidence: &ProbeEvidence) -> bool {
            self.retained && evidence.controller.is_some()
        }
        fn cancel(&mut self, _: &DraftRecord) -> Result<String, HostError> {
            new_id()
        }
    }
    fn test_verifier(now: Instant, retained: bool) -> (Verifier, TestFolder) {
        let root = std::env::temp_dir().join(format!("yang-proofs-{}", new_id().unwrap()));
        std::fs::create_dir(&root).unwrap();
        let registry = Registry::open(&root.join("devices.json")).unwrap();
        let mut verifier = Verifier::new(registry, Box::new(FakePort { retained }));
        verifier.clock = Box::new(move || now);
        (verifier, TestFolder(root))
    }
    impl Consent {
        fn for_draft(draft: &DraftRecord, supervised: bool) -> Self {
            let catalog = Catalog::load(DOCUMENT).unwrap();
            let profile = catalog
                .profile(
                    draft.model_id.as_deref().unwrap(),
                    draft.profile_id.as_deref().unwrap(),
                )
                .unwrap();
            Self {
                accepted: true,
                mode: draft.mode.clone(),
                config_digest: draft.config_digest.clone(),
                open_effects: profile.open_effects.clone(),
                supervised,
                retain_session: supervised,
            }
        }
    }
    impl VerifiedProof {
        fn test_fixture(start: Instant) -> Self {
            Self {
                evidence: ProbeEvidence {
                    draft_id: "1".repeat(32),
                    model_id: "aq6370".into(),
                    profile_id: "gpib-visa".into(),
                    config_digest: "digest".into(),
                    config_rev: 1,
                    identity: json!({"model":"AQ6370D","serial":"SN"}),
                    mode: "real".into(),
                    probe_version: 1,
                    release_confirmed: true,
                    retained_session: false,
                    controller: None,
                },
                issued: start,
                consumed: false,
            }
        }
    }
    #[test]
    fn registration_is_atomic_and_proof_is_consumed_only_after_commit() {
        let (mut verifier, _folder) = test_verifier(Instant::now(), false);
        let draft = verifier
            .create_draft(
                "aq6370",
                "gpib-visa",
                json!({"resource":"GPIB0::4::INSTR"}),
                "OSA",
                "real",
                0,
            )
            .unwrap();
        let operation = verifier
            .begin(&draft.device_id, 1, Consent::for_draft(&draft, false))
            .unwrap();
        let proof = operation.proof_id.unwrap();
        assert_eq!(verifier.registry.snapshot().unwrap().devices.len(), 0);
        let device = verifier.save(&draft.device_id, &proof, 1).unwrap();
        assert_eq!(device.device_id, draft.device_id);
        assert!(verifier.proofs[&proof].consumed);
        assert_eq!(verifier.registry.snapshot().unwrap().devices.len(), 1);
        assert!(verifier.save(&draft.device_id, &proof, 2).is_err());
    }
    #[test]
    fn draft_edits_invalidate_proofs_and_capacity_is_responsibility_bounded() {
        let (mut verifier, _folder) = test_verifier(Instant::now(), false);
        let draft = verifier
            .create_draft(
                "aq6370",
                "gpib-visa",
                json!({"resource":"GPIB0::4::INSTR"}),
                "OSA",
                "real",
                0,
            )
            .unwrap();
        let operation = verifier
            .begin(&draft.device_id, 1, Consent::for_draft(&draft, false))
            .unwrap();
        let mut changed = draft.clone();
        changed.revision = 2;
        changed.params = json!({"resource":"GPIB0::5::INSTR"});
        verifier.update_draft(changed, 1).unwrap();
        assert!(verifier
            .save(&draft.device_id, &operation.proof_id.unwrap(), 2)
            .is_err());
        for number in 1..64 {
            verifier
                .create_draft(
                    "gain",
                    "cp210x-serial",
                    json!({"port":format!("COM{number}")}),
                    "Gain",
                    "real",
                    number + 1,
                )
                .unwrap();
        }
        assert!(verifier
            .create_draft(
                "gain",
                "cp210x-serial",
                json!({"port":"COM65"}),
                "Gain",
                "real",
                65
            )
            .is_err());
    }
    #[test]
    fn cancel_preserves_retired_identity_without_publishing_device() {
        let (mut verifier, _folder) = test_verifier(Instant::now(), false);
        let draft = verifier
            .create_draft(
                "aq6370",
                "gpib-visa",
                json!({"resource":"GPIB0::4::INSTR"}),
                "OSA",
                "real",
                0,
            )
            .unwrap();
        verifier.cancel(&draft.device_id, 1).unwrap();
        assert_eq!(verifier.registry.snapshot().unwrap().devices.len(), 0);
        assert_eq!(
            verifier.registry.snapshot().unwrap().drafts[0].status,
            "Cancelled"
        );
        let mut reused = draft;
        reused.revision = 2;
        assert!(verifier
            .registry
            .commit(2, RegistryChange::SaveDraft(reused))
            .is_err());
    }
    #[test]
    fn canonical_sha256_is_stable_and_uses_windows_cng() {
        assert_eq!(
            canonical_digest(&json!({})).unwrap(),
            "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a"
        );
        assert_eq!(
            canonical_digest(&json!({"b":{"z":1,"a":2},"a":3})).unwrap(),
            canonical_digest(&json!({"a":3,"b":{"a":2,"z":1}})).unwrap()
        );
    }

    use super::*;
    use serde_json::json;
    use std::time::{Duration, Instant};
    #[test]
    fn proof_expires_at_exactly_sixty_seconds() {
        let start = Instant::now();
        let proof = VerifiedProof::test_fixture(start);
        assert!(proof.valid_at(start + Duration::from_millis(59999)));
        assert!(!proof.valid_at(start + Duration::from_secs(60)));
    }
    struct WrongModelPort(FakePort);
    struct GainProtocolPort(FakePort);
    impl VerificationPort for GainProtocolPort {
        fn probe(&mut self, draft: &DraftRecord, consent: &Consent) -> Result<ProbeEvidence, HostError> {
            let mut evidence = self.0.probe(draft, consent)?;
            evidence.identity = json!({"model":"Gain Chip Driver","resource":"COM4","identity_quality":"weak_protocol","protocol":"READY fields"});
            Ok(evidence)
        }
        fn retained_controller_valid(&self, evidence: &ProbeEvidence) -> bool { self.0.retained_controller_valid(evidence) }
        fn cancel(&mut self, draft: &DraftRecord) -> Result<String, HostError> { self.0.cancel(draft) }
    }
    #[test]
    fn native_gain_protocol_identity_can_be_verified_saved_and_reopened() {
        let (mut verifier, folder) = test_verifier(Instant::now(), true);
        verifier.port = Box::new(GainProtocolPort(FakePort {retained:true}));
        let draft = verifier.create_draft("gain", "cp210x-serial", json!({"port":"COM4"}), "Gain", "real", 0).unwrap();
        let result = verifier.begin(&draft.device_id, 1, Consent::for_draft(&draft, true)).unwrap();
        let device = verifier.save(&draft.device_id, &result.proof_id.unwrap(), 1).unwrap();
        assert_eq!(device.expected_identity["identity_quality"], "weak_protocol");
        assert_eq!(device.expected_identity["resource"], "COM4");
        assert_eq!(Registry::open(&folder.0.join("devices.json")).unwrap().snapshot().unwrap().devices.len(), 1);
    }
    impl VerificationPort for WrongModelPort {
        fn probe(
            &mut self,
            draft: &DraftRecord,
            consent: &Consent,
        ) -> Result<ProbeEvidence, HostError> {
            let mut evidence = self.0.probe(draft, consent)?;
            evidence.identity = json!({"model":"unrelated instrument","serial":"SN"});
            Ok(evidence)
        }
        fn retained_controller_valid(&self, _: &ProbeEvidence) -> bool {
            false
        }
        fn cancel(&mut self, draft: &DraftRecord) -> Result<String, HostError> {
            self.0.cancel(draft)
        }
    }
    #[test]
    fn wrong_identity_never_creates_registration_proof() {
        let (mut verifier, _folder) = test_verifier(Instant::now(), false);
        verifier.port = Box::new(WrongModelPort(FakePort { retained: false }));
        let draft = verifier
            .create_draft(
                "aq6370",
                "gpib-visa",
                json!({"resource":"GPIB0::4::INSTR"}),
                "OSA",
                "real",
                0,
            )
            .unwrap();
        assert_eq!(
            verifier
                .begin(&draft.device_id, 1, Consent::for_draft(&draft, false))
                .unwrap_err()
                .code,
            "IdentityMismatch"
        );
        assert!(verifier.proofs.is_empty());
    }
    #[test]
    fn legacy_proof_cannot_register_any_device() {
        let mut proof = VerifiedProof::test_fixture(Instant::now());
        proof.evidence.mode = "simulate".into();
        assert!(proof.matches("simulate", 1, "digest").is_err());
        assert!(proof.matches("real", 1, "digest").is_err());
        assert!(proof.matches("real", 2, "digest").is_err());
        assert!(proof.matches("real", 1, "changed").is_err());
    }
    #[test]
    fn save_failure_keeps_supervised_session() {
        let start = Instant::now();
        let (mut verifier, folder) = test_verifier(start, true);
        let draft = verifier
            .create_draft(
                "gain",
                "cp210x-serial",
                json!({"port":"COM7"}),
                "Gain",
                "real",
                0,
            )
            .unwrap();
        let operation = verifier
            .begin(&draft.device_id, 1, Consent::for_draft(&draft, true))
            .unwrap();
        let proof_id = operation.proof_id.unwrap();
        use std::os::windows::fs::OpenOptionsExt;
        let held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(folder.0.join("devices.json"))
            .unwrap();
        assert_eq!(
            verifier
                .save(&draft.device_id, &proof_id, 1)
                .unwrap_err()
                .code,
            "ConfigStorage"
        );
        assert!(verifier.proofs[&proof_id].evidence.retained_session);
        assert_eq!(verifier.registry.snapshot().unwrap().devices.len(), 0);
        drop(held);
        assert!(verifier.save(&draft.device_id, &proof_id, 1).is_ok());
    }
}
