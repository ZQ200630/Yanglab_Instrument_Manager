//! The same published table used by the Rust driver and Python adapter.
use super::contracts::HostError;
use serde_json::{Value,json};
pub async fn serialize_change<T>(admission:&tokio::sync::Mutex<()>,work:impl std::future::Future<Output=Result<T,HostError>>)->Result<T,HostError> {
    let _ordered=admission.lock().await;work.await
}
#[derive(Default)]
pub struct ActivationState(std::sync::Mutex<std::collections::BTreeMap<String,HostError>>);
impl ActivationState {
    pub fn ready(&self,domain:&str)->Result<(),HostError> {
        match self.0.lock().unwrap().get(domain) {Some(error)=>Err(error.clone()),None=>Ok(())}
    }
    pub fn record(&self,domain:&str,result:Result<(),HostError>)->Result<(),HostError> {
        if result.is_err() {
            let error=HostError::new("ConfigRetained","Limits were saved but could not be activated. Restart Host before reconnecting.");
            self.0.lock().unwrap().insert(domain.into(),error.clone());Err(error)
        } else {self.0.lock().unwrap().remove(domain);Ok(())}
    }
}
pub fn validate_limits(head:&str,limits:&Value)->Result<Value,HostError> {
    let invalid=||HostError::new("LimitsInvalid","Operating limits must be within the laser head's hardware limits");
    let object=limits.as_object().ok_or_else(invalid)?;
    if object.len()!=3||!["min_nm","max_nm","max_speed_nm_s"].iter().all(|k|object.contains_key(*k)) {return Err(invalid());}
    let model=head.strip_prefix("TLB-").unwrap_or(head);let model=model.strip_suffix("-P").unwrap_or(model);
    let table:Value=serde_json::from_slice(include_bytes!("../../../../Code/Utils/tlb_models.json")).map_err(|_|invalid())?;
    let a=table[model][0].as_f64().ok_or_else(invalid)?;
    let b=table[model][1].as_f64().ok_or_else(invalid)?;
    let speed=table[model][2].as_f64().ok_or_else(invalid)?;
    let min=limits["min_nm"].as_f64().ok_or_else(invalid)?;
    let max=limits["max_nm"].as_f64().ok_or_else(invalid)?;
    let v=limits["max_speed_nm_s"].as_f64().ok_or_else(invalid)?;
    if !min.is_finite()||!max.is_finite()||!v.is_finite()||min<a||max>b||min>=max||v<0.01||v>speed {return Err(invalid());}
    Ok(json!({"operating_min_nm":min,"operating_max_nm":max,"scan_speed_limit_nm_s":v}))
}
pub fn validate_params(head:&str,params:&Value)->Result<(),HostError> {
    let keys=["operating_min_nm","operating_max_nm","scan_speed_limit_nm_s"];
    if keys.iter().any(|k|params.get(*k).is_some()) {
        validate_limits(head,&json!({"min_nm":params[keys[0]],"max_nm":params[keys[1]],"max_speed_nm_s":params[keys[2]]}))?;
    }
    Ok(())
}
#[cfg(test)] mod tests {
    use super::*;
    #[test] fn operating_limits_cannot_expand_hardware_or_custom_heads() {
        let limits=json!({"min_nm":1050.0,"max_nm":1080.0,"max_speed_nm_s":1.0});
        assert!(validate_limits("6722-P",&limits).is_ok());
        for bad in [json!({"min_nm":1044.0,"max_nm":1080.0,"max_speed_nm_s":1.0}),
            json!({"min_nm":1050.0,"max_nm":1086.0,"max_speed_nm_s":1.0}),
            json!({"min_nm":1080.0,"max_nm":1050.0,"max_speed_nm_s":1.0}),
            json!({"min_nm":1050.0,"max_nm":1080.0,"max_speed_nm_s":11.0}),
            json!({"min_nm":1050.0,"max_nm":1080.0,"max_speed_nm_s":0.0}),
            json!({"min_nm":true,"max_nm":1080.0,"max_speed_nm_s":1.0}),
            json!({"min_nm":1050.0,"max_nm":1080.0,"max_speed_nm_s":1.0,"raw":"*RST"})] {
            assert!(validate_limits("6722-P",&bad).is_err());
        }
        assert!(validate_limits("6722-P-EXT",&limits).is_err());
    }
    #[test] fn configuration_holds_connection_admission_until_activation() {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
        use std::sync::Arc;
        let admission=Arc::new(tokio::sync::Mutex::new(()));
        let gate=admission.clone();
        let (entered_tx,entered_rx)=tokio::sync::oneshot::channel();
        let (release_tx,release_rx)=tokio::sync::oneshot::channel();
        let change=tokio::spawn(async move {serialize_change(&gate,async move {
            entered_tx.send(()).unwrap();release_rx.await.unwrap();Ok::<_,HostError>(())
        }).await});
        entered_rx.await.unwrap();
        assert!(admission.try_lock().is_err(),"another window must not admit an old-config connection");
        release_tx.send(()).unwrap();change.await.unwrap().unwrap();
        assert!(admission.try_lock().is_ok());
        });
    }
    #[test] fn persisted_but_unactivated_limits_block_every_later_connection() {
        let activation=ActivationState::default();
        activation.ready("device:laser").unwrap();
        assert!(activation.record("device:laser",Err(HostError::new("Injected","configuration reply lost"))).is_err());
        assert_eq!(activation.ready("device:laser").unwrap_err().code,"ConfigRetained");
        activation.ready("device:other").unwrap();
        activation.record("device:laser",Ok(())).unwrap();
        activation.ready("device:laser").unwrap();
    }
}
