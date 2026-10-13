use crate::{ExperimentDefinition,ExperimentError};
use serde_json::{json, Value};
use std::collections::BTreeSet;

pub fn templates() -> Value {json!({"templates":[
    {"id":"waveform_capture","version":1,"name":"Waveform capture","roles":[{"role":"detector","family":"scope","channel":true}],"changes_outputs":false},
    {"id":"q_scan","version":1,"name":"Q Measurement","roles":[{"role":"laser","family":"laser","channel":false},{"role":"scan_signal","family":"generator","channel":true},{"role":"detector","family":"scope","channel":true}],"changes_outputs":true},
    {"id":"fsr_scan","version":1,"name":"FSR Measurement","roles":[{"role":"laser","family":"laser","channel":false},{"role":"scan_signal","family":"generator","channel":true},{"role":"detector","family":"scope","channel":true}],"changes_outputs":true}
]})}
fn invalid(message:&str)->ExperimentError {ExperimentError::new("InvalidExperiment",message)}
fn id(value:&str)->bool{value.len()==32&&value.bytes().all(|b|b.is_ascii_digit()||(b'a'..=b'f').contains(&b))}
fn finite(value:&Value,min:f64,max:f64)->bool{value.as_f64().is_some_and(|v|v.is_finite()&&v>=min&&v<=max)}
pub fn validate_definition(definition:&ExperimentDefinition)->Result<(),ExperimentError>{
    let d=definition;
    if d.schema!=1||d.template_version!=1||!id(&d.id)||d.name.trim().is_empty()||d.name.len()>160
        ||!matches!(d.template_id.as_str(),"waveform_capture"|"q_scan"|"fsr_scan") {return Err(invalid("Invalid experiment identity, name or template version"));}
    let expected=if d.template_id=="waveform_capture"{vec!["detector"]}else{vec!["detector","scan_signal","laser"]};
    let mut roles=BTreeSet::new();let mut devices=BTreeSet::new();
    if d.roles.len()!=expected.len(){return Err(invalid("Required instrument roles are missing or duplicated"));}
    for role in &d.roles {
        if !expected.contains(&role.role.as_str())||!roles.insert(role.role.as_str())||!id(&role.host_id)||!id(&role.domain_id)
            ||!devices.insert((&role.host_id,&role.domain_id))||role.role!="laser"&&!role.channel.is_some_and(|c|(1..=64).contains(&c))
            ||role.role=="laser"&&role.channel.is_some() {return Err(invalid("Invalid or conflicting role binding"));}
    }
    let p=d.parameters.as_object().ok_or_else(||invalid("Experiment parameters must be an object"))?;
    let allowed=["source","memory_depth","frames","interval_ms","channels","acquisition","resume_after_capture","waveform","frequency_hz","amplitude_vpp","offset_v","coarse_wavelength_nm","excursion_pm","ramp_hz","wavelength_nm","scan_speed_nm_s","prominence_fraction","min_distance_samples","roi"];
    if p.keys().any(|k|!allowed.contains(&k.as_str())) {return Err(invalid("Unknown experiment parameter"));}
    if !matches!(d.parameters["source"].as_str(),Some("screen"|"memory"))
        ||!d.parameters["frames"].as_u64().is_some_and(|n|(1..=9999).contains(&n))
        ||!d.parameters["interval_ms"].as_u64().is_some_and(|n|n<=3_600_000)
        ||!d.parameters["memory_depth"].is_null()&&!d.parameters["memory_depth"].as_u64().is_some_and(|n|(1..=250_000_000).contains(&n)) {return Err(invalid("Invalid source, frame count, interval or memory depth"));}
    if let Some(acquisition)=p.get("acquisition") {
        if d.template_id!="waveform_capture"||!matches!(acquisition.as_str(),Some("fresh"|"existing_screen"|"stopped")) {
            return Err(invalid("Acquisition choice is supported only for waveform capture"));
        }
        if acquisition=="existing_screen"&&(d.parameters["source"]!="screen"||d.parameters["frames"]!=1
            ||d.parameters["interval_ms"]!=0||!d.parameters["memory_depth"].is_null()) {
            return Err(invalid("Existing screen read requires one screen frame, no interval and no memory-depth change"));
        }
    }
    if let Some(resume)=p.get("resume_after_capture") {
        if d.template_id!="waveform_capture"||d.parameters["acquisition"]!="stopped"||!resume.is_boolean() {
            return Err(invalid("Resume after capture requires a boolean policy on stopped waveform capture"));
        }
    }
    let channels=d.parameters["channels"].as_array().ok_or_else(||invalid("Capture channels are required"))?;
    let mut seen=BTreeSet::new();
    if channels.is_empty()||channels.len()>64||channels.iter().any(|c|!c.as_u64().is_some_and(|n|(1..=64).contains(&n)&&seen.insert(n))){return Err(invalid("Capture channels must be unique valid channel numbers"));}
    if !d.parameters["roi"].is_null(){let r:yang_data::SampleRange=serde_json::from_value(d.parameters["roi"].clone()).map_err(|_|invalid("Invalid ROI"))?;if r.start>=r.end{return Err(invalid("ROI is empty"));}}
    if d.template_id!="waveform_capture" {
        if d.parameters["waveform"]!="triangle"||!finite(&d.parameters["frequency_hz"],0.001,1e9)
            ||!finite(&d.parameters["amplitude_vpp"],0.000001,20.)||!finite(&d.parameters["offset_v"],-10.,10.)
            ||d.parameters["offset_v"].as_f64().unwrap().abs()+d.parameters["amplitude_vpp"].as_f64().unwrap()/2.>10.
            ||!d.parameters["coarse_wavelength_nm"].is_null()&&!finite(&d.parameters["coarse_wavelength_nm"],1.,5000.) {return Err(invalid("Invalid triangle signal or coarse wavelength"));}
        if d.template_id=="q_scan"&&(!finite(&d.parameters["excursion_pm"],f64::MIN_POSITIVE,1e9)||!finite(&d.parameters["ramp_hz"],f64::MIN_POSITIVE,1e9)||!finite(&d.parameters["wavelength_nm"],f64::MIN_POSITIVE,1e6)) {return Err(invalid("Q measurement requires positive scan excursion, ramp frequency and wavelength"));}
        if d.template_id=="q_scan"&&d.parameters["ramp_hz"].as_f64()!=d.parameters["frequency_hz"].as_f64(){return Err(invalid("Q ramp calibration frequency must match the requested generator frequency"));}
        if d.template_id=="fsr_scan"&&(!finite(&d.parameters["scan_speed_nm_s"],f64::MIN_POSITIVE,1e6)||!finite(&d.parameters["prominence_fraction"],0.000001,1.)||!d.parameters["min_distance_samples"].as_u64().is_some_and(|n|n>=1&&n<=250_000_000)){return Err(invalid("FSR measurement requires positive scan speed, prominence and dip spacing"));}
    }
    Ok(())
}
