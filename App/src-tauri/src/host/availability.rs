//! Persistent authorization is independent of boot/session/control leases.
use super::catalog::{Catalog, DOCUMENT};
use super::contracts::{CheckBinding, CheckPolicy, DeviceRecord, HostError};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStage {
    Enumeration,
    Readonly,
}

impl CheckPolicy {
    /// Pure policy construction; storing a policy grants neither a lease nor I/O.
    pub fn authorize(mut self, stage: CheckStage, binding: CheckBinding) -> Self {
        match stage {
            CheckStage::Enumeration => self.enumeration = Some(binding),
            CheckStage::Readonly => self.readonly = Some(binding),
        }
        self
    }
    pub fn permits(
        &self,
        stage: CheckStage,
        device: &DeviceRecord,
        mode: &str,
    ) -> Result<bool, HostError> {
        if self.interval_s < 10 {
            return Err(HostError::new(
                "CheckPolicyInvalid",
                "Check interval must be at least ten seconds",
            ));
        }
        let binding = match stage {
            CheckStage::Enumeration => &self.enumeration,
            CheckStage::Readonly => &self.readonly,
        };
        let Some(binding) = binding else {
            return Ok(false);
        };
        let catalog = Catalog::load(DOCUMENT)?;
        let profile = catalog.profile(&device.model_id, &device.profile_id)?;
        let current = binding.mode == mode
            && binding.identity == device.expected_identity
            && binding.config_rev == device.config_rev
            && binding.profile_id == device.profile_id
            && binding.probe_version == profile.probe_version;
        Ok(current
            && (stage == CheckStage::Enumeration
                || profile.probe_mode == "readonly"
                    && profile.automatic_probe
                    && profile.open_effects.is_empty()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn binding() -> CheckBinding {
        CheckBinding {
            mode: "real".into(),
            identity: json!({"model":"AQ6370D","serial":"SN"}),
            config_rev: 1,
            profile_id: "gpib-visa".into(),
            probe_version: 1,
        }
    }
    fn device(model: &str, profile: &str) -> DeviceRecord {
        let catalog = Catalog::load(DOCUMENT).unwrap();
        let selected = catalog.model(model).unwrap();
        DeviceRecord {
            device_id: "1".repeat(32),
            name: "Instrument".into(),
            category: selected.category.clone(),
            model_id: model.into(),
            driver_id: selected.driver_id.clone(),
            profile_id: profile.into(),
            params: json!({}),
            expected_identity: binding().identity,
            identity_strength: "strong".into(),
            verified_mode: "real".into(),
            config_rev: 1,
            check_policy: CheckPolicy::default(),
        }
    }
    #[test]
    fn enumeration_and_readonly_require_separate_current_bindings() {
        let record = device("aq6370", "gpib-visa");
        let policy = CheckPolicy::default().authorize(CheckStage::Enumeration, binding());
        assert!(policy
            .permits(CheckStage::Enumeration, &record, "real")
            .unwrap());
        assert!(!policy
            .permits(CheckStage::Readonly, &record, "real")
            .unwrap());
        assert!(!policy
            .permits(CheckStage::Enumeration, &record, "simulate")
            .unwrap());
        let mut changed = record;
        changed.config_rev = 2;
        assert!(!policy
            .permits(CheckStage::Enumeration, &changed, "real")
            .unwrap());
    }
    #[test]
    fn source_gain_and_dtr_uncertainty_disable_idle_active_probes() {
        for (model, profile) in [
            ("gain", "cp210x-serial"),
            ("voltage", "ch340-serial"),
            ("mdt693b", "serial"),
        ] {
            let mut record = device(model, profile);
            let mut consent = binding();
            consent.profile_id = profile.into();
            consent.identity = record.expected_identity.clone();
            record.check_policy = CheckPolicy::default().authorize(CheckStage::Readonly, consent);
            assert!(!record
                .check_policy
                .permits(CheckStage::Readonly, &record, "real")
                .unwrap());
        }
    }
    #[test]
    fn permission_has_no_boot_or_lease_and_invalid_interval_is_rejected() {
        let record = device("aq6370", "gpib-visa");
        let policy = CheckPolicy::default().authorize(CheckStage::Readonly, binding());
        assert!(policy
            .permits(CheckStage::Readonly, &record, "real")
            .unwrap());
        let fields = serde_json::to_value(&policy).unwrap();
        assert!(!fields.to_string().contains("session_id"));
        let mut bad = policy;
        bad.interval_s = 9;
        assert!(bad.permits(CheckStage::Readonly, &record, "real").is_err());
    }
}
