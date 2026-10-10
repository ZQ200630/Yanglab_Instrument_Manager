//! Development diagnostics only; absent from App routes and shipped packages.
use serde::{Deserialize,Serialize};
use serde_json::{json,Value};
use std::{path::PathBuf,time::Duration};
use yang_lab_tlb::{Bus,Wire,Identity,ControlLimits,Control};
use yang_drivers::clock::Clock;
#[derive(Deserialize,Serialize)]
#[serde(deny_unknown_fields)]
pub struct Probe {origin_nm:f64,target_nm:f64,speed_nm_s:f64,stop_after_s:Option<f64>}
#[derive(Deserialize,Serialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {stage:String,output:PathBuf,identity:Option<Identity>,limits:Option<ControlLimits>,probe:Option<Probe>}
pub fn parse_plan(bytes:&[u8])->Result<Plan,String> {
    if bytes.len()>16384 {return Err("Plan exceeds capacity".into());}
    let plan:Plan=serde_json::from_slice(bytes).map_err(|e|e.to_string())?;
    crate::validate_output(&plan.output)?;
    validate_plan_fields(&plan)?;
    Ok(plan)
}
fn validate_plan_fields(plan:&Plan)->Result<(),String> {
    match plan.stage.as_str() {
        "enumerate" if plan.identity.is_none()&&plan.limits.is_none()&&plan.probe.is_none()=>{},
        "readonly"|"action"=>{
            let id=plan.identity.as_ref().ok_or("Exact identity required")?;
            let limits=plan.limits.as_ref().ok_or("Existing operating limits required")?;
            let (a,b,speed)=yang_lab_tlb::model_spec(&id.head_model).ok_or("Unknown head envelope")?;
            if id.manufacturer!="New Focus"||id.model!="TLB-6700"||id.firmware.is_empty()||id.head_serial.is_empty()||
                !yang_lab_tlb::valid_key(&format!("6700 SN{}",id.serial))||
                !limits.min_nm.is_finite()||!limits.max_nm.is_finite()||!limits.max_speed_nm_s.is_finite()||
                limits.min_nm<a||limits.max_nm>b||limits.min_nm>=limits.max_nm||limits.max_speed_nm_s<0.01||limits.max_speed_nm_s>speed {
                return Err("Invalid identity or operating limits".into());
            }
            if plan.stage=="readonly" {if plan.probe.is_some(){return Err("Readonly forbids motion".into());}}
            else {
                let p=plan.probe.as_ref().ok_or("Explicit bounded probe required")?;
                if !p.origin_nm.is_finite()||!p.target_nm.is_finite()||
                    !(0.099999..=0.250001).contains(&(p.origin_nm-p.target_nm).abs())||
                    ![0.05,0.10].contains(&p.speed_nm_s)||limits.max_speed_nm_s<speed||
                    p.origin_nm<limits.min_nm||p.origin_nm>limits.max_nm||p.target_nm<limits.min_nm||p.target_nm>limits.max_nm||
                    p.stop_after_s.is_some_and(|s|!s.is_finite()||!(0.5..=2.).contains(&s)) {
                    return Err("Probe must stay within 0.10–0.25 nm at 0.05 or 0.10 nm/s, with ceiling covering unknown RESET rate; optional Stop after 0.5–2 seconds".into());
                }
            }
        },_=>return Err("Unknown or mixed diagnostic stage".into()),
    }
    Ok(())
}
pub fn preview(plan:&Plan)->Value {
    json!({"preview_only":true,"plan":plan,"output_changes_authorized":false,"production_capability_changed":false,"possible_reset_speed_nm_s":plan.probe.as_ref().and_then(|_|plan.identity.as_ref()).and_then(|id|yang_lab_tlb::max_scan_speed(&id.head_model)),"action_effects":match plan.stage.as_str(){"enumerate"=>"list controller serials and release SDK; no output setters", "readonly"=>"read exact identity, status and scan settings; preserving release; no output setters", _=>"program temporary scan Start and both rates; one RESET move; STOP once at arrival, selected interrupt time or 12-second timeout; STOP also turns motor tracking off; observe one-second hold with Tracking Off and OPC complete; preserve emission and blanking settings; do not automatically return to origin"},"acceptance":"coarse controller readback, not optical measurement: arrival within 0.02 nm; >=3 intermediate positions over >=0.08 nm, observed rate within 20% and conservative time bounds; one-second stopped hold; midway Stop must hold inside the interval, within 0.06 nm of its pre-stop readback, with stopped evidence within 2 seconds of Stop completion"})
}
const ARRIVAL_NM:f64=yang_lab_tlb::SCAN_ARRIVAL_TOLERANCE_NM;
const STOP_DRIFT_NM:f64=0.060001;
const HOLD_SPAN_NM:f64=0.040001;
fn finite(value:&Value,name:&str)->Result<f64,String> {
    value[name].as_f64().filter(|v|v.is_finite()).ok_or_else(||format!("Missing/nonfinite {name}"))
}
fn flag(value:&Value,name:&str)->Result<bool,String> {
    value[name].as_bool().ok_or_else(||format!("Missing boolean {name}"))
}
fn final_motion_valid(sample:&Value,limits:ControlLimits)->Result<(),String> {
    let actual=finite(sample,"wavelength_nm")?;let setpoint=finite(sample,"wavelength_setpoint_nm")?;
    if actual<limits.min_nm||actual>limits.max_nm||setpoint<limits.min_nm||setpoint>limits.max_nm||
        !flag(sample,"operation_complete")?||flag(sample,"tracking")? {return Err("Final controller evidence is not a stopped in-bounds position".into());}
    Ok(())
}
#[derive(Serialize)]
struct RateEvidence {verified:bool,direction:i8,intermediate_samples:usize,span_nm:f64,observed_nm_s:Option<f64>,minimum_nm_s:Option<f64>,maximum_nm_s:Option<f64>,reason:String}
/// Getter start/end bounds are retained. Slow exchanges cannot stand in for
/// independent movement observations or prove a rate from ACK to one endpoint.
fn rate_evidence(samples:&[Value],p:&Probe,limits:ControlLimits)->Result<RateEvidence,String> {
    let sign=(p.target_nm-p.origin_nm).signum();
    let mut evidence=RateEvidence{verified:false,direction:if sign>0.{1}else{-1},intermediate_samples:0,span_nm:0.,observed_nm_s:None,minimum_nm_s:None,maximum_nm_s:None,reason:"Insufficient intermediate motion evidence".into()};
    let mut points=Vec::new();let mut last_end=0.;let mut last_position=None;
    for sample in samples {
        let start=finite(sample,"read_start_s")?;let end=finite(sample,"read_end_s")?;
        if start<0.||end<start||start<last_end {return Err("Non-monotonic trajectory timestamps".into());}
        last_end=end;
        let m=&sample["motion"];let position=finite(m,"wavelength_nm")?;
        let _=finite(m,"wavelength_setpoint_nm")?;
        if position<limits.min_nm||position>limits.max_nm {return Err("Trajectory exceeded operating bounds".into());}
        let tracking=flag(m,"tracking")?;let complete=flag(m,"operation_complete")?;
        if sample["phase"]=="moving" {
            if last_position.is_some_and(|old:f64|sign*(position-old)< -ARRIVAL_NM) {
                evidence.reason="Trajectory moved against the requested direction".into();return Ok(evidence);
            }
            last_position=Some(position);
            if tracking&&!complete&&sign*(position-p.origin_nm)>ARRIVAL_NM&&sign*(p.target_nm-position)>ARRIVAL_NM {
                points.push((start,end,position));
            }
        } else if sample["phase"]!="holding"&&sample["phase"]!="confirming_hold" {return Err("Unknown trajectory phase".into());}
    }
    evidence.intermediate_samples=points.len();
    if let (Some(first),Some(last))=(points.first(),points.last()) {
        let span=sign*(last.2-first.2);evidence.span_nm=span;
        let center=(last.0+last.1-first.0-first.1)/2.;let minimum_time=last.0-first.1;let maximum_time=last.1-first.0;
        if span>=0.079999&&points.len()>=3&&minimum_time>0.&&center>0. {
            let observed=span/center;
            let low=(span-HOLD_SPAN_NM).max(0.)/maximum_time;
            let high=(span+HOLD_SPAN_NM)/minimum_time;
            evidence.observed_nm_s=Some(observed);evidence.minimum_nm_s=Some(low);evidence.maximum_nm_s=Some(high);
            // Every intermediate read bracket must support the requested slope.
            // A fast jump then pause must not hide behind the first/last mean.
            let linear=points.iter().all(|point|{
                let dt=(point.0+point.1-first.0-first.1)/2.;
                let uncertainty=HOLD_SPAN_NM+p.speed_nm_s*((point.1-point.0)+(first.1-first.0))/2.;
                (sign*(point.2-first.2)-p.speed_nm_s*dt).abs()<=uncertainty
            });
            // Central readback rate must agree; conservative time/readback
            // brackets must also exclude a gross (2x / 0.5x) mismatch.
            evidence.verified=linear&&(observed-p.speed_nm_s).abs()<=p.speed_nm_s*0.20+1e-8&&low>=p.speed_nm_s*0.5-1e-8&&high<=p.speed_nm_s*2.+1e-8;
            evidence.reason=if evidence.verified{"Requested rate observed within coarse readback/time bounds"}else{"Observed rate or conservative time bounds do not support the requested rate"}.into();
        }
    }
    Ok(evidence)
}
fn hold_evidence(samples:&[Value],limits:ControlLimits)->Result<(bool,f64,f64,usize),String> {
    let mut first_end=None;let mut last_start=0.;let mut low=f64::INFINITY;let mut high=f64::NEG_INFINITY;let mut count=0;
    for sample in samples {
        if sample["phase"]!="holding" {
            // A later pending/busy sample invalidates the earlier hold window.
            // Only the final uninterrupted stopped sample window can qualify.
            first_end=None;last_start=0.;low=f64::INFINITY;high=f64::NEG_INFINITY;count=0;continue;
        }
        let m=&sample["motion"];let position=finite(m,"wavelength_nm")?;
        if position<limits.min_nm||position>limits.max_nm||!flag(m,"operation_complete")?||flag(m,"tracking")? {return Err("Hold sample is not a stopped in-bounds position".into());}
        first_end.get_or_insert(finite(sample,"read_end_s")?);last_start=finite(sample,"read_start_s")?;
        low=low.min(position);high=high.max(position);count+=1;
    }
    let duration=first_end.map_or(0.,|first|last_start-first);let span=if count>0{high-low}else{0.};
    Ok((count>=2&&duration>=1.-1e-8&&span<=HOLD_SPAN_NM,duration,span,count))
}

/// Midway Stop must hold an interior position, not finish naturally later.
/// Readback tolerance includes the native Stop exchange; this is not optical
/// stopping distance or a promise of instantaneous motor response.
fn stop_evidence(samples:&[Value],p:&Probe,observation:&Value)->Result<bool,String> {
    let Some(scheduled)=p.stop_after_s else{return Ok(true)};
    let start=finite(observation,"stop_start_s")?;let end=finite(observation,"stop_end_s")?;
    if start<scheduled-1e-8||start>scheduled+0.5||end<start||end-start>2. {return Ok(false);}
    let Some(previous)=samples.iter().rev().find(|sample|sample["phase"]=="moving") else{return Ok(false)};
    let prior_end=finite(previous,"read_end_s")?;let previous=&previous["motion"];
    if prior_end>start+1e-8||start-prior_end>0.5||flag(previous,"operation_complete")?||!flag(previous,"tracking")? {return Ok(false);}
    let prior=finite(previous,"wavelength_nm")?;let sign=(p.target_nm-p.origin_nm).signum();
    let inside=|position:f64|sign*(position-p.origin_nm)>ARRIVAL_NM&&sign*(p.target_nm-position)>ARRIVAL_NM;
    if !inside(prior)||!inside(finite(&observation["after"],"wavelength_nm")?) {return Ok(false);}
    let mut first_hold=None;
    for sample in samples.iter().filter(|sample|sample["phase"]!="moving") {
        let read_start=finite(sample,"read_start_s")?;
        let position=finite(&sample["motion"],"wavelength_nm")?;
        if read_start<end-1e-8||!inside(position)||(position-prior).abs()>STOP_DRIFT_NM {return Ok(false);}
        if sample["phase"]=="holding" {first_hold.get_or_insert(read_start);}
    }
    Ok(first_hold.is_some_and(|at|at-end<=2.)&&
       (finite(&observation["after"],"wavelength_nm")?-prior).abs()<=STOP_DRIFT_NM)
}
fn send_probe_stop<T:Wire>(bus:&mut Bus<T>,key:&str,clock:&dyn Clock,started:Duration,report:&mut Value)->Result<(),String> {
    report["observation"]["stop_start_s"]=json!(clock.now().saturating_sub(started).as_secs_f64());
    let result=bus.control(key,Control::ScanStop,true);
    report["observation"]["stop_end_s"]=json!(clock.now().saturating_sub(started).as_secs_f64());
    result.map_err(|e|e.message)
}
/// Read explicitly supplied immutable diagnostic files; this API never opens
/// hardware or changes the compiled production qualification set.
pub fn review_candidate(paths:&[PathBuf])->Result<Value,String> {
    use std::collections::BTreeSet;
    if paths.len()!=6 {return Err("Exactly six explicit endpoint/interrupt reports are required".into());}
    let mut hashes=BTreeSet::new();let mut evidence_hashes=Vec::new();let mut cases=BTreeSet::new();
    let mut common_identity=None;let mut common_sdk=None;
    for path in paths {
        let metadata=std::fs::symlink_metadata(path).map_err(|e|e.to_string())?;
        if !metadata.is_file()||metadata.file_type().is_symlink()||metadata.len()>1024*1024 {return Err("Reports must be bounded ordinary files".into());}
        #[cfg(windows)] {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes()&0x400!=0 {return Err("Report reparse points are not allowed".into());}
        }
        let bytes=std::fs::read(path).map_err(|e|e.to_string())?;
        if bytes.len()>1024*1024 {return Err("Report exceeds capacity".into());}
        let hash=ring::digest::digest(&ring::digest::SHA256,&bytes).as_ref().iter().map(|b|format!("{b:02x}")).collect::<String>();
        if !hashes.insert(hash.clone()) {return Err("Duplicate report evidence".into());}
        let report:Value=serde_json::from_slice(&bytes).map_err(|e|e.to_string())?;
        if report["schema"]!=2||report["stage"]!="action"||report["source_kind"]!="real"||
            report.get("error")!=Some(&Value::Null)||report["resources_released"]!=true||
            report["production_capability_changed"]!=false||report["physical_measurement_verified"]!=false||
            report["cleanup"]["action"]!="preserving_close"||report["cleanup"].get("error")!=Some(&Value::Null)||report["cleanup"]["resources_released"]!=true {
            return Err("Only successful real schema-2 action reports with preserving release are reviewable".into());
        }
        let sdk=report["sdk_sha256"].as_str().filter(|s|s.len()==64&&s.bytes().all(|b|b.is_ascii_digit()||(b'a'..=b'f').contains(&b))).ok_or("Missing lowercase SDK SHA256")?.to_string();
        let plan:Plan=serde_json::from_value(report["plan"].clone()).map_err(|e|e.to_string())?;validate_plan_fields(&plan)?;
        if plan.stage!="action" {return Err("Report plan is not an action".into());}
        let identity=plan.identity.as_ref().ok_or("Missing plan identity")?;
        let observed:Identity=serde_json::from_value(report["observation"]["identity"].clone()).map_err(|e|e.to_string())?;
        if observed!=*identity||common_identity.as_ref().is_some_and(|id|id!=identity)||common_sdk.as_ref().is_some_and(|id|id!=&sdk) {return Err("Controller/head/firmware identity or SDK changed between reports".into());}
        common_identity=Some(identity.clone());common_sdk=Some(sdk);
        let p=plan.probe.as_ref().ok_or("Missing probe")?;let limits=plan.limits.ok_or("Missing limits")?;
        let observation=&report["observation"];
        let samples=observation["trajectory"].as_array().filter(|v|v.len()<=160).ok_or("Missing/bounded trajectory required")?;
        let rate=rate_evidence(samples,p,limits)?;let (held,_,_,_)=hold_evidence(samples,limits)?;
        if !rate.verified||!held {return Err("Raw trajectory does not establish requested rate and one-second stopped hold".into());}
        if !stop_evidence(samples,p,observation)? {return Err("Midway Stop did not establish a bounded interior hold".into());}
        let before=&observation["before"];let after=&observation["after"];
        final_motion_valid(after,limits)?;
        if flag(before,"output_enabled")?!=flag(after,"output_enabled")?||!flag(after,"operation_complete")?||flag(after,"tracking")? {return Err("Output state changed or final controller is not stopped".into());}
        let cfg_before=observation["scan_settings_before"]["scan_configuration"].as_u64().filter(|v|*v<=255).ok_or("Invalid initial blanking configuration")?;
        let settings=&observation["scan_settings_after"];
        if settings["scan_configuration"].as_u64()!=Some(cfg_before)||
            (finite(settings,"start_nm")?-p.target_nm).abs()>0.005001||
            (finite(settings,"forward_speed_nm_s")?-p.speed_nm_s).abs()>0.000001||
            (finite(settings,"backward_speed_nm_s")?-p.speed_nm_s).abs()>0.000001 {
            return Err("Blanking or verified scan settings differ from the reviewed probe".into());
        }
        if observation["timeout_stop"]!=false||observation["stop_command_sent"]!=true {return Err("Timed-out or missing explicit hold cannot qualify".into());}
        let last_moving=samples.iter().rev().find(|s|s["phase"]=="moving").ok_or("Missing pre-stop motion")?;
        let position=finite(&last_moving["motion"],"wavelength_nm")?;
        let endpoint=flag(&last_moving["motion"],"operation_complete")?&&(position-p.target_nm).abs()<=ARRIVAL_NM;
        let interrupted=p.stop_after_s.is_some();
        if endpoint==interrupted||observation["arrival_observed"]!=Value::Bool(endpoint)||interrupted&&p.speed_nm_s!=0.10 {
            return Err("Endpoint/interrupt report does not match a required qualification case".into());
        }
        let direction=if p.target_nm>p.origin_nm {1i8}else{-1};let rate_key=if p.speed_nm_s==0.05{5}else{10};
        if !cases.insert((direction,rate_key,interrupted)) {return Err("Duplicate endpoint/interrupt coverage".into());}
        evidence_hashes.push(hash);
    }
    // Four endpoint cases (two rates, both directions), two interrupted cases
    // at 0.10 nm/s. A candidate is source-review input, never an installation.
    if cases.len()!=6 {return Err("Incomplete case coverage".into());}
    Ok(json!({"schema":1,"records":[{"identity":common_identity.unwrap(),"sdk_sha256":common_sdk.unwrap(),"rates_nm_s":[0.05,0.10],"evidence_sha256":evidence_hashes,"protocol_revision":1}]}))
}
/// A human-review artifact in a new diagnostic Result directory. It is never
/// installed as an App configuration or added to the compiled capability set.
pub fn write_candidate(path:&std::path::Path,candidate:&Value)->Result<(),String> {
    if path.file_name().and_then(|v|v.to_str())!=Some("profile.json") {return Err("Candidate filename must be profile.json".into());}
    let parent=path.parent().ok_or("Missing candidate directory")?;crate::validate_output(parent)?;
    let bytes=serde_json::to_vec_pretty(candidate).map_err(|e|e.to_string())?;
    std::fs::create_dir(parent).map_err(|e|e.to_string())?;
    crate::write_new(path,&bytes)
}
/// Explicit bounded injected transports only; production CLI uses execute().
pub fn run_with<T:Wire>(plan:&Plan,confirmation:Option<&str>,bus:&mut Bus<T>,clock:&dyn Clock)->Result<Value,String> {
    run_stage(plan,confirmation,bus,clock,"bounded_transport_test")
}
fn run_stage<T:Wire>(plan:&Plan,confirmation:Option<&str>,bus:&mut Bus<T>,clock:&dyn Clock,source:&str)->Result<Value,String> {
    let Some(stage)=confirmation else{return Ok(preview(plan))};
    if stage!=plan.stage {return Err("Separate confirmation for this exact stage required".into());}
    if !bus.resources_released() {return Err("Existing owner must release all resources before diagnostics".into());}
    crate::validate_output(&plan.output)?;
    std::fs::create_dir(&plan.output).map_err(|e|e.to_string())?;
    let mut report=json!({"schema":2,"source_kind":source,"stage":stage,"plan":plan,"sdk_sha256":null,"production_capability_changed":false,"error":null,"observation":{"rate_verified":false,"hold_observed":false,"arrival_observed":false,"stop_command_sent":false,"timeout_stop":false},"cleanup":null,"resources_released":false,"physical_measurement_verified":false});
    let result=(||->Result<(),String>{
        if stage=="enumerate" {report["observation"]["controller_keys"]=json!(bus.enumerate().map_err(|e|e.message)?);return Ok(());}
        let expected=plan.identity.as_ref().ok_or("Missing identity")?;let key=format!("6700 SN{}",expected.serial);
        let actual=bus.connect(&key).map_err(|e|e.message)?;
        #[cfg(windows)] if source=="real" {report["sdk_sha256"]=json!(bus.loaded_sdk_sha256().ok_or("Loaded Newport SDK fingerprint is unavailable")?);}
        if actual!=*expected {return Err("Controller/head identity changed; no action sent".into());}
        bus.set_limits(&key,plan.limits.ok_or("Missing operating limits")?).map_err(|e|e.message)?;
        let before=bus.status(&key).map_err(|e|e.message)?;
        let settings=bus.scan_settings(&key).map_err(|e|e.message)?;
        report["observation"]["identity"]=json!(actual);report["observation"]["before"]=json!(before);report["observation"]["scan_settings_before"]=settings;
        if stage=="readonly" {return Ok(());}
        let p=plan.probe.as_ref().ok_or("Missing probe")?;
        bus.probe_single_scan(&key,expected,p.origin_nm,p.target_nm,p.speed_nm_s,true).map_err(|e|e.message)?;
        let started=clock.now();let mut stop_time=None;let mut arrived=false;let mut stopped=false;let mut timed_out=false;let mut samples=Vec::<Value>::new();
        let limits=plan.limits.ok_or("Missing limits")?;
        loop {
            let elapsed=clock.now().saturating_sub(started).as_secs_f64();
            if !stopped&&(p.stop_after_s.is_some_and(|s|elapsed>=s)||elapsed>=12.) {
                // Disclosed and authorized with the probe, never a RESET or replay.
                send_probe_stop(bus,&key,clock,started,&mut report)?;
                stopped=true;timed_out=elapsed>=12.;stop_time=Some(clock.now());
            }
            let read_start=clock.now();let sample=bus.motion(&key).map_err(|e|e.message)?;
            let read_end=clock.now();
            let wavelength=sample.wavelength_nm;let complete=sample.operation_complete;
            let phase=if !stopped{"moving"}else if complete&&!sample.tracking{"holding"}else{"confirming_hold"};
            samples.push(json!({"read_start_s":read_start.saturating_sub(started).as_secs_f64(),"read_end_s":read_end.saturating_sub(started).as_secs_f64(),"phase":phase,"motion":sample}));
            report["observation"]["trajectory"]=json!(samples);
            if !stopped&&complete&&(wavelength-p.target_nm).abs()<=ARRIVAL_NM {
                arrived=true;
                // This one endpoint hold is part of the disclosed action stage.
                // Never retry a setter after an uncertain native exchange.
                send_probe_stop(bus,&key,clock,started,&mut report)?;
                stopped=true;stop_time=Some(clock.now());
            }
            report["observation"]["arrival_observed"]=json!(arrived);report["observation"]["stop_command_sent"]=json!(stopped);report["observation"]["timeout_stop"]=json!(timed_out);
            let rate=rate_evidence(&samples,p,limits)?;report["observation"]["rate_verified"]=json!(rate.verified);report["observation"]["rate"]=json!(rate);
            let (held,duration,span,count)=hold_evidence(&samples,limits)?;
            report["observation"]["hold_observed"]=json!(held);report["observation"]["hold_duration_s"]=json!(duration);report["observation"]["hold_span_nm"]=json!(span);report["observation"]["hold_samples"]=json!(count);
            if held||stop_time.is_some_and(|t|clock.now().saturating_sub(t)>=Duration::from_secs(5)){break;}
            if samples.len()>=160 {return Err("Observation capacity reached; outcome uncertain".into());}
            clock.wait(Duration::from_millis(100));
        }
        report["observation"]["arrival_observed"]=json!(arrived);
        report["observation"]["stop_command_sent"]=json!(stopped);
        report["observation"]["timeout_stop"]=json!(timed_out);
        report["observation"]["after"]=json!(bus.status(&key).map_err(|e|e.message)?);
        final_motion_valid(&report["observation"]["after"],limits)?;
        report["observation"]["scan_settings_after"]=bus.scan_settings(&key).map_err(|e|e.message)?;
        let output_same=report["observation"]["before"]["output_enabled"]==report["observation"]["after"]["output_enabled"];
        let config_same=report["observation"]["scan_settings_before"]["scan_configuration"]==report["observation"]["scan_settings_after"]["scan_configuration"];
        report["observation"]["output_preserved"]=json!(output_same);report["observation"]["configuration_preserved"]=json!(config_same);
        let stop_valid=stop_evidence(&samples,p,&report["observation"])?;
        report["observation"]["midway_stop_verified"]=json!(stop_valid&&p.stop_after_s.is_some());
        if p.stop_after_s.is_some()&&!stop_valid {return Err("Midway Stop did not establish a bounded interior hold; no qualification".into());}
        if timed_out {return Err("Endpoint not observed within 12 seconds; one STOP sent. No production capability enabled".into());}
        if report["observation"]["rate_verified"]!=true {return Err("Intermediate controller-readback evidence did not establish the requested rate".into());}
        if report["observation"]["hold_observed"]!=true {return Err("Insufficient one-second Tracking-Off/OPC-complete hold evidence".into());}
        if !output_same||!config_same {return Err("Output state or scan blanking configuration changed during the probe".into());}
        Ok(())
    })();
    if let Err(error)=result {report["error"]=json!(error);}
    let release=bus.close_all();report["cleanup"]=json!({"action":"preserving_close","error":release.as_ref().err(),"resources_released":bus.resources_released()});
    report["resources_released"]=json!(release.is_ok()&&bus.resources_released());
    crate::write_new(&plan.output.join("report.json"),&serde_json::to_vec_pretty(&report).map_err(|e|e.to_string())?)?;
    Ok(report)
}
#[cfg(windows)]
pub fn execute(plan:&Plan,confirmation:&str)->Result<Value,String> {
    if confirmation!=plan.stage {return Err("Confirmation stage differs from plan".into());}
    let _host=sil_instrument_console::host::instance::InstanceGuard::acquire_diagnostic().map_err(|e|e.to_string())?;
    let _newport=yang_lab_tlb::sdk::OwnerGuard::acquire().map_err(|e|e.message)?;
    let mut bus=Bus::new(yang_lab_tlb::sdk::Sdk::default());
    let result=run_stage(plan,Some(confirmation),&mut bus,&yang_drivers::clock::SystemClock::default(),"real");
    while !bus.resources_released() {
        eprintln!("Preserving release remains pending; diagnostic owner retained. No motion replay.");
        std::thread::sleep(Duration::from_secs(1));let _=bus.close_all();
    }
    result
}
