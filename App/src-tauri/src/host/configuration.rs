//! One bounded configuration actor. Disk and probe waits never occupy IPC threads.
use super::operations::{ExecuteParams, OperationActor, OperationRecord};
use super::{
    contracts::*,
    leases::{DomainRef, LeaseBook, Session},
    registry::{new_id, Registry},
    verification::{Consent, ProbeEvidence, VerificationPort, Verifier},
};
use crate::runtime::{WorkerRequest, WorkerRuntime};
use serde_json::{json, Value};
#[derive(Clone)]
pub(crate) struct VerificationControl {
    pub session: Session,
    pub token: String,
    pub epoch: u64,
    pub request_id: String,
    pub sequence: u64,
}
use std::{
    collections::BTreeMap,
    sync::{mpsc, Arc, Mutex},
    time::{Duration, Instant},
};
type Job = Box<dyn FnOnce(&mut Verifier) -> Result<Value, HostError> + Send>;
pub(crate) struct Configuration {
    sender: mpsc::SyncSender<(Job, tokio::sync::oneshot::Sender<Result<Value, HostError>>)>,
    pub cache: Arc<Mutex<RegistrySnapshot>>,
}
impl Configuration {
    pub fn new(registry: Registry, port: Box<dyn VerificationPort>) -> Result<Self, HostError> {
        let cache = Arc::new(Mutex::new(registry.snapshot()?));
        let published = cache.clone();
        let (tx, rx) =
            mpsc::sync_channel::<(Job, tokio::sync::oneshot::Sender<Result<Value, HostError>>)>(31);
        std::thread::Builder::new()
            .name("host-configuration".into())
            .spawn(move || {
                let mut verifier = Verifier::new(registry, port);
                while let Ok((job, reply)) = rx.recv() {
                    let result = job(&mut verifier);
                    if let Ok(snapshot) = verifier.registry.snapshot() {
                        *published.lock().unwrap() = snapshot;
                    }
                    let _ = reply.send(result);
                }
            })
            .map_err(|error| HostError::new("HostRuntime", error.to_string()))?;
        Ok(Self { sender: tx, cache })
    }
    pub async fn call(&self, job: Job) -> Result<Value, HostError> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.sender.try_send((job, tx)).map_err(|_| {
            HostError::new("ConfigCapacity", "Configuration actor busy or unavailable")
        })?;
        rx.await.map_err(|_| {
            HostError::new("ConfigRetained", "Configuration actor stopped before reply")
        })?
    }
}
#[derive(Clone)]
pub(crate) struct WorkerVerificationPort {
    pub query_gate: Arc<tokio::sync::Mutex<()>>,
    pub worker: Arc<WorkerRuntime>,
    pub leases: Arc<Mutex<LeaseBook>>,
    pub bindings: Arc<Mutex<BTreeMap<DomainRef, Value>>>,
    pub control: Arc<Mutex<BTreeMap<String, VerificationControl>>>,
    pub operations: Arc<OperationActor>,
    pub proofs: Arc<Mutex<BTreeMap<String, String>>>,
    pub retired: Arc<Mutex<BTreeMap<DomainRef, String>>>,
}
impl WorkerVerificationPort {
    pub fn quiescent(&self, domain: &DomainRef) -> Result<(), HostError> {
        if self.worker.domain_pending(&domain.key()) != 0
            || self
                .worker
                .domain_context(&domain.key())
                .is_some_and(|ctx| !ctx["connection_id"].is_null())
        {
            return Err(HostError::new(
                "ReleaseRequired",
                "Safe stop and await confirmed release before changing configuration",
            ));
        }
        if self.leases.lock().unwrap().snapshot()[domain.key()]["state"] == "RETAINED" {
            return Err(HostError::new(
                "ReleaseRequired",
                "Cleanup responsibility remains retained",
            ));
        }
        Ok(())
    }
    pub fn configure_wire(&self, config: Value) -> Result<(), HostError> {
        let domain: DomainRef = serde_json::from_value(config["domain"].clone())
            .map_err(|e| worker_error(e.to_string()))?;
        let reply = self
            .worker
            .submit(WorkerRequest::V3(
                json!({"v":3,"id":new_id()?,"method":"configure_domain","params":{"config":config},
            "context":self.worker.global_context().map_err(worker_error)?}),
            ))
            .map_err(|e| worker_error(e.message))?
            .wait(Duration::from_secs(30))
            .map_err(|e| worker_error(e.message))?;
        if reply["ok"] != true {
            return Err(worker_error(reply.to_string()));
        }
        self.leases.lock().unwrap().register(&domain)?;
        self.bindings
            .lock()
            .unwrap()
            .insert(domain, reply["result"]["context"].clone());
        Ok(())
    }
    pub fn configure(&self, draft: &DraftRecord) -> Result<(), HostError> {
        let domain = DomainRef {
            kind: "device".into(),
            id: draft.device_id.clone(),
        };
        if self.bindings.lock().unwrap().contains_key(&domain) {
            return Ok(());
        }
        let catalog = super::catalog::Catalog::load(super::catalog::DOCUMENT)?;
        let model = catalog.model(draft.model_id.as_deref().unwrap_or(""))?;
        let reply=self.worker.submit(WorkerRequest::V3(json!({"v":3,"id":new_id()?,"method":"configure_domain","params":{"config":{
            "domain":domain,"config_rev":draft.revision,"driver_kind":model.driver_kind,"model_id":model.id,"profile_id":draft.profile_id,
            "params":draft.params,"expected_identity":{},"members":[]}},"context":self.worker.global_context().map_err(worker_error)?})))
            .map_err(|e|worker_error(e.message))?.wait(Duration::from_secs(30)).map_err(|e|worker_error(e.message))?;
        if reply["ok"] != true {
            return Err(worker_error(reply.to_string()));
        }
        self.leases.lock().unwrap().register(&domain)?;
        self.bindings
            .lock()
            .unwrap()
            .insert(domain, reply["result"]["context"].clone());
        Ok(())
    }
}
pub(crate) fn device_wire(device: &DeviceRecord) -> Result<Value, HostError> {
    let catalog = super::catalog::Catalog::load(super::catalog::DOCUMENT)?;
    Ok(
        json!({"domain":{"kind":"device","id":device.device_id},"config_rev":device.config_rev,"driver_kind":catalog.model(&device.model_id)?.driver_kind,
        "model_id":device.model_id,"profile_id":device.profile_id,"params":device.params,"expected_identity":device.expected_identity,"members":[]}),
    )
}
pub(crate) fn setup_wire(
    setup: &SetupRecord,
    snapshot: &RegistrySnapshot,
) -> Result<Value, HostError> {
    let members = setup
        .members
        .iter()
        .map(|id| {
            snapshot
                .devices
                .iter()
                .find(|d| d.device_id == *id)
                .ok_or_else(|| worker_error("Setup member missing"))
        })
        .map(|d| d.and_then(device_wire))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(
        json!({"domain":{"kind":"setup","id":setup.setup_id},"config_rev":setup.config_rev,"driver_kind":"fiber","model_id":"fiber-coupling","profile_id":null,"params":{},"expected_identity":{},"members":members}),
    )
}
impl VerificationPort for WorkerVerificationPort {
    fn probe(
        &mut self,
        draft: &DraftRecord,
        consent: &Consent,
    ) -> Result<ProbeEvidence, HostError> {
        self.configure(draft)?;
        let domain = DomainRef {
            kind: "device".into(),
            id: draft.device_id.clone(),
        };
        let control = self
            .control
            .lock()
            .unwrap()
            .get(&draft.device_id)
            .cloned()
            .ok_or_else(|| {
                HostError::new("ControlDenied", "Verification requires current control")
            })?;
        let session = control.session;
        let token = control.token;
        let epoch = control.epoch;
        let context = self
            .worker
            .domain_context(&domain.key())
            .or_else(|| self.bindings.lock().unwrap().get(&domain).cloned())
            .ok_or_else(|| worker_error("Context unavailable"))?;
        let authorization = json!({"accepted":consent.accepted,"stage":if consent.supervised{"supervised"}else{"readonly"},"supervised":consent.supervised,
            "retain_session":consent.retain_session,"binding":{"domain":domain,"mode":draft.mode,"model_id":draft.model_id,
                "profile_id":draft.profile_id,"config_rev":draft.revision,"config_digest":draft.config_digest,"controller":session.id()}});
        let intent = ExecuteParams {
            domain: domain.clone(),
            lease_token: token.clone(),
            control_epoch: epoch,
            config_rev: draft.revision,
            context: context.clone(),
            method: "probe".into(),
            params: json!({"authorization":authorization}),
            sequence: control.sequence,
            confirmation: None,
        };
        let existing = self
            .operations
            .read(|book| book.lookup(&session, &control.request_id, &intent))?;
        let reply = if let Some(record) = existing {
            if record.status != "Terminal" {
                return Err(HostError::new(
                    "OutcomeUnknown",
                    "Previous verification may still own a call; never replay",
                ));
            };
            record.result
        } else {
            let record = {
                let session = session.clone();
                let mut intent = intent.clone();
                let id = control.request_id.clone();
                self.operations
                    .call_sync::<OperationRecord>(Box::new(move |book| {
                        intent.confirmation = Some(
                            book.prepare(&session, intent.clone(), Instant::now())?
                                .token,
                        );
                        Ok(serde_json::to_value(book.admit(
                            &session,
                            &id,
                            intent,
                            Instant::now(),
                        )?)
                        .unwrap())
                    }))?
            };
            let pending = {
                let mut leases = self.leases.lock().unwrap();
                leases.admit(&token, &session, &domain, epoch, Instant::now())?;
                self.worker.submit(WorkerRequest::V3(json!({"v":3,"id":record.worker_id,"method":"probe","params":intent.params,"context":context}))).map_err(|e|worker_error(e.message))?
            };
            let reply = pending
                .wait(Duration::from_secs(90))
                .map_err(|e| worker_error(e.message))?;
            let result = reply.clone();
            let id = record.operation_id.clone();
            let _: OperationRecord = self.operations.call_sync(Box::new(move |book| {
                Ok(serde_json::to_value(book.finish(
                    &id,
                    result["phase"].as_str().unwrap_or("timed_out_unknown"),
                    result.clone(),
                )?)
                .unwrap())
            }))?;
            reply
        };
        if reply["ok"] != true {
            return Err(HostError::new("VerificationFailed", reply.to_string()));
        }
        let evidence = &reply["result"];
        let proof = evidence["proof_id"]
            .as_str()
            .filter(|id| super::contracts::valid_id(id))
            .ok_or_else(|| worker_error("Worker proof ID missing"))?;
        self.proofs
            .lock()
            .unwrap()
            .insert(draft.device_id.clone(), proof.into());
        Ok(ProbeEvidence {
            draft_id: draft.device_id.clone(),
            model_id: draft.model_id.clone().unwrap(),
            profile_id: draft.profile_id.clone().unwrap(),
            config_digest: draft.config_digest.clone(),
            config_rev: draft.revision,
            identity: evidence["identity"].clone(),
            mode: draft.mode.clone(),
            probe_version: evidence["probe_version"]
                .as_u64()
                .ok_or_else(|| worker_error("Missing probe version"))?,
            release_confirmed: evidence["release_confirmed"] == true,
            retained_session: evidence["retained_session"] == true,
            controller: evidence["controller"].as_str().map(str::to_owned),
        })
    }
    fn published(&mut self, e: &ProbeEvidence) -> Result<(), HostError> {
        let proof = self
            .proofs
            .lock()
            .unwrap()
            .get(&e.draft_id)
            .cloned()
            .ok_or_else(|| worker_error("Private worker proof not found"))?;
        let reply=self.worker.submit(WorkerRequest::V3(json!({"v":3,"id":new_id()?,"method":"register_verified","params":{
            "domain":{"kind":"device","id":e.draft_id},"proof_id":proof,"config_digest":e.config_digest,"config_rev":e.config_rev},
            "context":self.worker.global_context().map_err(worker_error)?}))).map_err(|e|worker_error(e.message))?.wait(Duration::from_secs(30)).map_err(|e|worker_error(e.message))?;
        if reply["ok"] != true {
            return Err(HostError::new("RegistrationBindingPending","Configuration committed; worker identity binding failed. Ordinary control remains blocked; safely restart Host."));
        };
        Ok(())
    }
    fn retained_controller_valid(&self, e: &ProbeEvidence) -> bool {
        self.control
            .lock()
            .unwrap()
            .get(&e.draft_id)
            .is_some_and(|control| {
                e.controller.as_deref() == Some(control.session.id())
                    && self
                        .leases
                        .lock()
                        .unwrap()
                        .admit(
                            &control.token,
                            &control.session,
                            &DomainRef {
                                kind: "device".into(),
                                id: e.draft_id.clone(),
                            },
                            control.epoch,
                            Instant::now(),
                        )
                        .is_ok()
            })
    }
    fn cancel(&mut self, draft: &DraftRecord) -> Result<String, HostError> {
        let domain = DomainRef {
            kind: "device".into(),
            id: draft.device_id.clone(),
        };
        self.retire(&domain, draft.revision)
    }
}
impl WorkerVerificationPort {
    pub fn retire(&mut self, domain: &DomainRef, revision: u64) -> Result<String, HostError> {
        if let Some(attempt) = self.retired.lock().unwrap().get(&domain) {
            return Ok(attempt.clone());
        }
        if self.retired.lock().unwrap().len() >= 4096 {
            return Err(HostError::new(
                "ConfigCapacity",
                "Retirement evidence capacity reached",
            ));
        }
        self.quiescent(&domain)?;
        if self.worker.domain_pending(&domain.key()) != 0 {
            return Err(HostError::new(
                "ReleaseRequired",
                "Domain has pending responsibility",
            ));
        }
        if self
            .worker
            .domain_context(&domain.key())
            .is_some_and(|ctx| !ctx["connection_id"].is_null())
        {
            return Err(HostError::new(
                "ReleaseRequired",
                "Safe stop the supervised session first",
            ));
        }
        let id = new_id()?;
        if !self.bindings.lock().unwrap().contains_key(domain)
            && self.worker.domain_context(&domain.key()).is_none()
        {
            let _query = self.query_gate.blocking_lock();
            let reply=self.worker.submit(WorkerRequest::V3(json!({"v":3,"id":id,"method":"status","params":{},"context":self.worker.global_context().map_err(worker_error)?}))).map_err(|e|worker_error(e.message))?.wait(Duration::from_secs(5)).map_err(|e|worker_error(e.message))?;
            if reply["ok"] != true
                || !reply["result"]["domains"].is_object()
                || !reply["result"]["domains"][domain.key()].is_null()
            {
                return Err(HostError::new(
                    "ReleaseRequired",
                    "Current worker absence could not be confirmed",
                ));
            }
            self.leases.lock().unwrap().forget_released(domain, true)?;
            self.retired
                .lock()
                .unwrap()
                .insert(domain.clone(), id.clone());
            return Ok(id);
        }
        let reply=self.worker.submit(WorkerRequest::V3(json!({"v":3,"id":id,"method":"retire_domain","params":{"domain":domain,"config_rev":revision},
            "context":self.worker.global_context().map_err(worker_error)?}))).map_err(|e|worker_error(e.message))?.wait(Duration::from_secs(30)).map_err(|e|worker_error(e.message))?;
        if reply["ok"] != true {
            return Err(HostError::new("ReleaseRequired", reply.to_string()));
        };
        self.leases.lock().unwrap().forget_released(&domain, true)?;
        self.bindings.lock().unwrap().remove(&domain);
        self.control.lock().unwrap().remove(&domain.id);
        self.proofs.lock().unwrap().remove(&domain.id);
        self.retired
            .lock()
            .unwrap()
            .insert(domain.clone(), id.clone());
        Ok(id)
    }
}
fn worker_error(message: impl Into<String>) -> HostError {
    HostError::new("WorkerVerification", message)
}
