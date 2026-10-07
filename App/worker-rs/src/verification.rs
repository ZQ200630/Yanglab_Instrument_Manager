use crate::{
    catalog::{admit, Catalog, ClaimToken, Claims, DOCUMENT},
    new_id, DomainRegistry, WorkerError,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use yang_drivers::{
    clock::Clock,
    lifecycle::{CleanupReport, CleanupStep, DriverState, ProbeReport},
};
use yang_protocol::{valid_hash, ContextV3, DomainConfig, DomainRef};
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeAuthorization {
    pub stage: String,
    pub accepted: bool,
    pub supervised: bool,
    pub retain_session: bool,
    pub binding: Value,
}
pub trait ProbePort: Send + Sync {
    fn registry(&self) -> DomainRegistry;
    fn probe_readonly(
        &self,
        config: &DomainConfig,
        authorization: &ProbeAuthorization,
    ) -> Result<ProbeReport, WorkerError>;
    /// Trusted cache of an independently authorized existing controlled session.
    /// This method must not connect, open, or change any physical output.
    fn supervised_snapshot(&self, _: &DomainConfig) -> Option<SupervisedSnapshot> {
        None
    }
}
pub struct SupervisedSnapshot {
    pub context: ContextV3,
    pub controller: String,
    pub report: ProbeReport,
}
#[derive(Clone, Debug, Serialize)]
pub struct VerificationProof {
    pub proof_id: String,
    domain: DomainRef,
    model_id: String,
    profile_id: String,
    config_digest: String,
    config_rev: u64,
    identity: Value,
    mode: String,
    probe_version: u64,
    issued_monotonic: f64,
    release_confirmed: bool,
    retained_session: bool,
    controller: Option<String>,
    observations: Value,
}
struct ProofEntry {
    proof: VerificationProof,
    config: DomainConfig,
    context: ContextV3,
    issued: Duration,
}
struct CheckPolicy {
    config: DomainConfig,
    authorization: ProbeAuthorization,
    next: Duration,
    interval: Duration,
}
#[derive(Default)]
struct State {
    proofs: HashMap<String, ProofEntry>,
    registered: HashMap<DomainRef, DomainConfig>,
    probes: HashMap<DomainRef, Option<ClaimToken>>,
    checks: HashMap<DomainRef, CheckPolicy>,
}
pub struct Verifier {
    port: Arc<dyn ProbePort>,
    clock: Arc<dyn Clock>,
    registry: DomainRegistry,
    claims: Claims,
    state: Mutex<State>,
}
fn error(code: &str, message: &str) -> WorkerError {
    WorkerError::new(code, message)
}
fn same(left: &DomainConfig, right: &DomainConfig) -> bool {
    serde_json::to_value(left).ok() == serde_json::to_value(right).ok()
}
impl Verifier {
    pub fn new(port: Arc<dyn ProbePort>, clock: Arc<dyn Clock>) -> Self {
        Self {
            registry: port.registry(),
            port,
            clock,
            claims: Claims::default(),
            state: Mutex::new(State::default()),
        }
    }
    pub fn claims(&self) -> Claims {
        self.claims.clone()
    }
    pub fn authorize_connection(&self, config: &DomainConfig) -> Result<DomainConfig, WorkerError> {
        let approved = admit(config)?;
        if config.domain.kind == "device" {
            if !same(&self.registry.config(&config.domain)?, config)
                || !self.registered(&config.domain)
            {
                return Err(error(
                    "VerificationRequired",
                    "Saved metadata is not native instrument identity evidence",
                ));
            }
        } else {
            for member in &config.members {
                self.authorize_connection(member)?;
            }
        }
        Ok(approved)
    }
    fn binding(
        &self,
        config: &DomainConfig,
        auth: &ProbeAuthorization,
    ) -> Result<String, WorkerError> {
        admit(config)?;
        if config.domain.kind != "device" || !same(&self.registry.config(&config.domain)?, config) {
            return Err(error("StaleProof", "Current physical draft required"));
        }
        let binding = auth
            .binding
            .as_object()
            .ok_or_else(|| error("Authorization", "Draft binding required"))?;
        let digest = binding
            .get("config_digest")
            .and_then(Value::as_str)
            .filter(|s| valid_hash(s))
            .ok_or_else(|| error("Authorization", "Valid configuration digest required"))?;
        if !auth.accepted
            || binding.get("mode") != Some(&Value::from("real"))
            || binding.get("domain") != Some(&serde_json::to_value(&config.domain).unwrap())
            || binding.get("config_rev") != Some(&Value::from(config.config_rev))
            || binding.get("model_id") != Some(&Value::from(config.model_id.clone()))
            || binding.get("profile_id") != Some(&Value::from(config.profile_id.clone().unwrap()))
        {
            return Err(error(
                "Authorization",
                "Consent does not bind the current real draft",
            ));
        }
        Ok(digest.into())
    }
    pub fn probe(
        &self,
        config: &DomainConfig,
        auth: &ProbeAuthorization,
    ) -> Result<VerificationProof, WorkerError> {
        let digest = self.binding(config, auth)?;
        let catalog = Catalog::load(DOCUMENT)?;
        let profile = catalog.profile(&config.model_id, config.profile_id.as_deref().unwrap())?;
        let context = self.registry.context(&config.domain)?;
        let retained = profile.probe_mode == "supervised";
        let supervised = if retained {
            if auth.stage != "supervised" || !auth.supervised || !auth.retain_session {
                return Err(error("ManualVerificationRequired","An independently authorized controlled session is required; never normal-connect for an identity check"));
            }
            let snapshot = self.port.supervised_snapshot(config).ok_or_else(|| {
                error(
                    "ManualVerificationRequired",
                    "No independently authorized session",
                )
            })?;
            if snapshot.context != context
                || context.connection_id.is_none()
                || auth.binding["controller"].as_str() != Some(snapshot.controller.as_str())
                || !self.registry.snapshot().iter().any(|s| {
                    s.context == context
                        && matches!(s.state, DriverState::Ready | DriverState::Active)
                })
            {
                return Err(error(
                    "ManualVerificationRequired",
                    "Controlled session authority does not match",
                ));
            }
            Some(snapshot)
        } else {
            if profile.probe_mode != "readonly"
                || auth.stage != "readonly"
                || auth.supervised
                || auth.retain_session
            {
                return Err(error(
                    "Authorization",
                    "Explicit read-only probe consent required",
                ));
            }
            if context.connection_id.is_some() {
                return Err(error(
                    "ResourceBusy",
                    "Owned observation lane is the sole reader",
                ));
            }
            None
        };
        {
            let mut state = self.state.lock().unwrap();
            if state.probes.contains_key(&config.domain) {
                return Err(error(
                    "ResourceBusy",
                    "Previous probe is active or retained",
                ));
            }
            let reservation = if retained {
                None
            } else {
                Some(self.claims.reserve(config)?)
            };
            state.probes.insert(config.domain.clone(), reservation);
            state
                .proofs
                .retain(|_, entry| entry.proof.domain != config.domain);
        }
        let (report, controller) = if let Some(snapshot) = supervised {
            (snapshot.report, Some(snapshot.controller))
        } else {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                self.port.probe_readonly(config, auth)
            })) {
                Ok(Ok(report)) => (report, None),
                Ok(Err(error)) => return Err(error),
                Err(_) => {
                    return Err(error(
                        "ProbePanic",
                        "Probe call panicked; physical responsibility retained",
                    ))
                }
            }
        };
        if !retained && !report.release_confirmed() {
            return Err(error(
                "ReleaseUnconfirmed",
                "Identity probe did not confirm transport release",
            ));
        }
        {
            let mut state = self.state.lock().unwrap();
            if let Some(Some(token)) = state.probes.get(&config.domain) {
                // Successful read-only port reports are immutable release evidence,
                // not voltage-zero or physical-output measurements.
                let cleanup = CleanupReport::new(
                    new_id()?,
                    vec![CleanupStep {
                        role: "probe".into(),
                        action: "close".into(),
                        error: None,
                    }],
                    None,
                    vec![],
                )?;
                self.claims.confirm_release(token, &cleanup)?;
            }
            state.probes.remove(&config.domain);
        }
        let model = report.identity()["model"]
            .as_str()
            .ok_or_else(|| error("IdentityMismatch", "Probe model missing"))?;
        let trusted = catalog.model(&config.model_id)?;
        if !(if config.driver_kind == "osa" {
            model.to_ascii_uppercase().starts_with("AQ6370")
        } else {
            model.eq_ignore_ascii_case(&trusted.name)
        }) {
            return Err(error(
                "IdentityMismatch",
                "Actual instrument model does not match catalog",
            ));
        }
        let expected = config
            .expected_identity
            .get("serial")
            .or_else(|| config.expected_identity.get("transport_serial"));
        let actual = report
            .identity()
            .get("serial")
            .or_else(|| report.identity().get("transport_serial"));
        if expected.is_some() && expected != actual {
            return Err(error(
                "IdentityMismatch",
                "Actual serial does not match expected identity",
            ));
        }
        if !report.identity().is_object()
            || !report.observations().is_object()
            || serde_json::to_vec(report.identity()).map_or(true, |b| b.len() > 8192)
            || serde_json::to_vec(report.observations()).map_or(true, |b| b.len() > 8192)
        {
            return Err(error("InvalidEvidence", "Unbounded probe evidence"));
        }
        let issued = self.clock.now();
        let proof = VerificationProof {
            proof_id: new_id()?,
            domain: config.domain.clone(),
            model_id: config.model_id.clone(),
            profile_id: profile.id.clone(),
            config_digest: digest,
            config_rev: config.config_rev,
            identity: report.identity().clone(),
            mode: "real".into(),
            probe_version: profile.probe_version,
            issued_monotonic: issued.as_secs_f64(),
            release_confirmed: report.release_confirmed(),
            retained_session: retained,
            controller,
            observations: report.observations().clone(),
        };
        let mut state = self.state.lock().unwrap();
        if !self.registry.matches(&context) || !same(&self.registry.config(&config.domain)?, config)
        {
            return Err(error(
                "StaleProof",
                "Configuration/context changed during probe",
            ));
        }
        state.proofs.insert(
            proof.proof_id.clone(),
            ProofEntry {
                proof: proof.clone(),
                config: config.clone(),
                context,
                issued,
            },
        );
        Ok(proof)
    }
    pub fn register_verified(
        &self,
        proof_id: &str,
        digest: &str,
        rev: u64,
    ) -> Result<(), WorkerError> {
        let mut state = self.state.lock().unwrap();
        let entry = state.proofs.get(proof_id).ok_or_else(|| {
            error(
                "StaleProof",
                "Proof unknown, superseded, or already consumed",
            )
        })?;
        let now = self.clock.now();
        if now < entry.issued
            || now - entry.issued >= Duration::from_secs(60)
            || digest != entry.proof.config_digest
            || rev != entry.proof.config_rev
            || !self.registry.matches(&entry.context)
            || !same(&self.registry.config(&entry.config.domain)?, &entry.config)
        {
            return Err(error(
                "StaleProof",
                "Proof expired or draft/session changed",
            ));
        }
        if entry.proof.retained_session
            && self
                .port
                .supervised_snapshot(&entry.config)
                .is_none_or(|s| {
                    s.context != entry.context || Some(s.controller) != entry.proof.controller
                })
        {
            return Err(error("StaleProof", "Controlled session authority ended"));
        }
        let mut config = entry.config.clone();
        config.expected_identity = entry.proof.identity.clone();
        self.registry
            .refine_identity(&entry.context, rev, &config.expected_identity)?;
        state.registered.insert(config.domain.clone(), config);
        state.proofs.remove(proof_id);
        Ok(())
    }
    pub fn registered(&self, domain: &DomainRef) -> bool {
        let state = self.state.lock().unwrap();
        state.registered.get(domain).is_some_and(|saved| {
            self.registry
                .config(domain)
                .is_ok_and(|current| same(saved, &current))
        })
    }
    pub fn confirm_probe_release(
        &self,
        domain: &DomainRef,
        cleanup: &CleanupReport,
    ) -> Result<(), WorkerError> {
        let mut state = self.state.lock().unwrap();
        let token = state
            .probes
            .get(domain)
            .and_then(Option::as_ref)
            .ok_or_else(|| error("ReleaseUnconfirmed", "No retained probe claim"))?;
        self.claims.confirm_release(token, cleanup)?;
        state.probes.remove(domain);
        Ok(())
    }
    pub fn authorize_checks(
        &self,
        config: &DomainConfig,
        auth: &ProbeAuthorization,
        interval_s: u64,
    ) -> Result<(), WorkerError> {
        self.binding(config, auth)?;
        let catalog = Catalog::load(DOCUMENT)?;
        let profile = catalog.profile(&config.model_id, config.profile_id.as_deref().unwrap())?;
        if interval_s < 10
            || interval_s > 86400
            || !profile.automatic_probe
            || profile.probe_mode != "readonly"
            || auth.stage != "readonly"
            || auth.supervised
            || auth.retain_session
        {
            return Err(error(
                "Authorization",
                "Periodic read-only checks require an eligible profile and bound consent",
            ));
        }
        self.state.lock().unwrap().checks.insert(
            config.domain.clone(),
            CheckPolicy {
                config: config.clone(),
                authorization: auth.clone(),
                next: self.clock.now() + Duration::from_secs(interval_s),
                interval: Duration::from_secs(interval_s),
            },
        );
        Ok(())
    }
    /// Intent only: the management scheduler must admit this as a probe job. No I/O.
    pub fn due_check(&self) -> Option<(DomainConfig, ProbeAuthorization)> {
        let now = self.clock.now();
        let mut state = self.state.lock().unwrap();
        if !state.probes.is_empty() {
            return None;
        }
        let snapshots = self.registry.snapshot();
        state.checks.retain(|domain, policy| {
            self.registry
                .config(domain)
                .is_ok_and(|c| same(&c, &policy.config))
        });
        for snapshot in snapshots {
            let domain = snapshot.context.domain.as_ref().unwrap();
            if snapshot.pending != 0
                || snapshot.responsibility
                || snapshot.context.connection_id.is_some()
            {
                continue;
            }
            if let Some(policy) = state.checks.get_mut(domain) {
                if now >= policy.next {
                    policy.next = now + policy.interval;
                    return Some((policy.config.clone(), policy.authorization.clone()));
                }
            }
        }
        None
    }
}
