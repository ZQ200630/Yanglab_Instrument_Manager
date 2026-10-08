//! One Host-owned idle check, never a control lease or a driver reconnection.
use super::{
    availability::CheckStage,
    contracts::{DeviceRecord, HostError},
    leases::DomainRef,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;
#[derive(Clone)]
pub struct Intent {
    pub device: DeviceRecord,
    pub context: Value,
    pub stage: CheckStage,
    pub started_ms: u64,
    binding: String,
}

#[cfg(test)]
mod refresh_tests {
    use super::*;
    #[test]
    fn explicit_refresh_selects_the_requested_device_before_other_due_devices() {
        use super::super::contracts::CheckBinding;
        fn record(id: &str) -> DeviceRecord {
            let mut device = DeviceRecord {
                device_id: id.into(), name: id.into(), category: "OSA".into(),
                model_id: "aq6370".into(), driver_id: "aq6370".into(),
                profile_id: "gpib-visa".into(), params: json!({}),
                expected_identity: json!({"model":"AQ6370","serial":id}),
                identity_strength:"strong".into(),verified_mode:"real".into(),
                config_rev:1,check_policy:Default::default(),
            };
            device.check_policy = device.check_policy.authorize(CheckStage::Enumeration, CheckBinding {
                mode:"real".into(),config_rev:1,profile_id:device.profile_id.clone(),identity:device.expected_identity.clone(),probe_version:1,
            });
            device
        }
        let devices=[record("a"),record("b")];
        let contexts=devices.iter().map(|d|(DomainRef {kind:"device".into(),id:d.device_id.clone()},json!({"connection_id":null,"epoch":0}))).collect();
        let control=json!({"device:a":{"state":"AVAILABLE"},"device:b":{"state":"AVAILABLE"}});
        let mut checks=Checks::default();checks.refresh_now("b");
        let selected=checks.begin(&devices,"real",&contexts,&control,0,|_|false).unwrap().unwrap();
        assert_eq!(selected.device.device_id,"b");
    }
    #[test]
    fn explicit_refresh_makes_the_selected_interval_due() {
        let mut checks=Checks::default();
        checks.states.insert("a".into(),State{binding:"binding".into(),next_ms:30_000,failures:0,last_success:None,quality:"detected".into(),detected:Some(true)});
        checks.refresh_now("a");
        assert_eq!(checks.states["a"].next_ms,0);
    }
}
struct State {
    binding: String,
    next_ms: u64,
    failures: usize,
    last_success: Option<u64>,
    quality: String,
    detected: Option<bool>,
}
#[derive(Default)]
pub struct Checks {
    states: BTreeMap<String, State>,
    active: Option<Intent>,
    requested: Option<String>,
}
fn binding(d: &DeviceRecord, c: &Value) -> String {
    serde_json::to_string(&(d, c)).unwrap()
}
impl Checks {
    pub fn active(&self) -> bool { self.active.is_some() }
    pub fn refresh_now(&mut self, id: &str) {
        self.requested = Some(id.into());
        if let Some(state) = self.states.get_mut(id) { state.next_ms = 0; }
    }
    pub fn checking(&self, d: &DomainRef) -> bool {
        self.active
            .as_ref()
            .is_some_and(|i| d.kind == "device" && i.device.device_id == d.id)
    }
    pub fn begin(
        &mut self,
        devices: &[DeviceRecord],
        mode: &str,
        contexts: &BTreeMap<DomainRef, Value>,
        control: &Value,
        now: u64,
        pending: impl Fn(&str) -> bool,
    ) -> Result<Option<Intent>, HostError> {
        self.states
            .retain(|id, _| devices.iter().any(|d| &d.device_id == id));
        if self.active.is_some() {
            return Ok(None);
        }
        let requested = self.requested.take();
        let mut ordered: Vec<_> = devices.iter().enumerate().collect();
        ordered.sort_by_key(|(_, d)| requested.as_deref() != Some(d.device_id.as_str()));
        for (index, d) in ordered {
            let domain = DomainRef {
                kind: "device".into(),
                id: d.device_id.clone(),
            };
            let key = domain.key();
            let Some(context) = contexts.get(&domain) else {
                continue;
            };
            let bound = binding(d, context);
            let state = self
                .states
                .entry(d.device_id.clone())
                .or_insert_with(|| State {
                    binding: bound.clone(),
                    next_ms: now + index as u64 * 500,
                    failures: 0,
                    last_success: None,
                    quality: "authorization_required".into(),
                    detected: None,
                });
            if state.binding != bound {
                *state = State {
                    binding: bound.clone(),
                    next_ms: now + index as u64 * 500,
                    failures: 0,
                    last_success: None,
                    quality: "authorization_required".into(),
                    detected: None,
                };
            }
            if requested.as_deref() == Some(d.device_id.as_str()) { state.next_ms = 0; }
            let stage = if d.check_policy.permits(CheckStage::Readonly, d, mode)? {
                CheckStage::Readonly
            } else if d.check_policy.permits(CheckStage::Enumeration, d, mode)? {
                CheckStage::Enumeration
            } else {
                state.quality = "authorization_required".into();
                continue;
            };
            if now < state.next_ms {
                continue;
            }
            if !context["connection_id"].is_null()
                || control[&key]["state"] != "AVAILABLE"
                || pending(&key)
            {
                state.quality = "busy_or_owned_cache".into();
                continue;
            }
            state.quality = "checking".into();
            let intent = Intent {
                device: d.clone(),
                context: context.clone(),
                stage,
                started_ms: now,
                binding: bound,
            };
            self.active = Some(intent.clone());
            return Ok(Some(intent));
        }
        Ok(None)
    }
    pub fn finish(
        &mut self,
        i: &Intent,
        device: Option<&DeviceRecord>,
        context: Option<&Value>,
        now: u64,
        success: bool,
        detected: Option<bool>,
        retained: bool,
    ) {
        self.active = None;
        let Some(state) = self.states.get_mut(&i.device.device_id) else {
            return;
        };
        if !device
            .zip(context)
            .is_some_and(|(d, c)| binding(d, c) == i.binding)
        {
            state.last_success = None;
            state.quality = "context_changed".into();
            return;
        }
        if retained {
            state.quality = "cleanup_unresolved".into();
            state.next_ms = u64::MAX;
            return;
        }
        if success {
            state.failures = 0;
            state.next_ms = now + i.device.check_policy.interval_s as u64 * 1000;
            state.detected = detected.or(state.detected);
            state.quality = if i.stage == CheckStage::Readonly {
                state.last_success = Some(i.started_ms);
                "fresh"
            } else {
                "detected"
            }
            .into();
        } else {
            state.next_ms = now + [30000, 60000, 120000, 300000][state.failures.min(3)];
            state.failures = (state.failures + 1).min(3);
            state.quality = "failed".into();
        }
    }
    pub fn state(&self, d: &DeviceRecord, c: &Value, now: u64) -> Value {
        let Some(s) = self.states.get(&d.device_id) else {
            return json!({"communication":"UNKNOWN","quality":"authorization_required"});
        };
        if s.binding != binding(d, c) {
            return json!({"communication":"UNKNOWN","quality":"context_changed"});
        }
        let age = s.last_success.and_then(|v| now.checked_sub(v));
        let ttl = super::catalog::Catalog::load(super::catalog::DOCUMENT)
            .ok()
            .and_then(|cat| {
                cat.profile(&d.model_id, &d.profile_id)
                    .ok()
                    .map(|p| p.communication_ttl_s as u64 * 1000)
            })
            .unwrap_or(0);
        json!({"communication":if age.is_some_and(|n|n<ttl){"ONLINE"}else{"UNKNOWN"},"quality":s.quality,"detected":s.detected,"last_success_ms":s.last_success,"observed_age_ms":age})
    }
}
