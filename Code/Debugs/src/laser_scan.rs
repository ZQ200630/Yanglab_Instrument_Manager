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
    Ok(plan)
}
pub fn preview(plan:&Plan)->Value {
    json!({"preview_only":true,"plan":plan,"output_changes_authorized":false,"production_capability_changed":false,"possible_reset_speed_nm_s":plan.probe.as_ref().and_then(|_|plan.identity.as_ref()).and_then(|id|yang_lab_tlb::max_scan_speed(&id.head_model)),"action_effects":match plan.stage.as_str(){"enumerate"=>"list controller serials and release SDK; no output setters", "readonly"=>"read exact identity, status and scan settings; preserving release; no output setters", _=>"program temporary scan Start and both rates; one RESET move; optionally STOP at selected time, or STOP once at 12-second timeout; STOP also turns motor tracking off; observe 1-second hold; preserve emission and blanking settings; do not automatically return to origin"}})
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
    let mut report=json!({"schema":1,"source_kind":source,"stage":stage,"plan":plan,"production_capability_changed":false,"error":null,"observation":null,"cleanup":null,"resources_released":false,"physical_measurement_verified":false});
    let result=(||->Result<(),String>{
        if stage=="enumerate" {report["observation"]=json!({"controller_keys":bus.enumerate().map_err(|e|e.message)?});return Ok(());}
        let expected=plan.identity.as_ref().ok_or("Missing identity")?;let key=format!("6700 SN{}",expected.serial);
        let actual=bus.connect(&key).map_err(|e|e.message)?;
        if actual!=*expected {return Err("Controller/head identity changed; no action sent".into());}
        bus.set_limits(&key,plan.limits.ok_or("Missing operating limits")?).map_err(|e|e.message)?;
        let before=bus.status(&key).map_err(|e|e.message)?;
        let settings=bus.scan_settings(&key).map_err(|e|e.message)?;
        report["observation"]=json!({"identity":actual,"before":before,"scan_settings_before":settings});
        if stage=="readonly" {return Ok(());}
        let p=plan.probe.as_ref().ok_or("Missing probe")?;
        bus.probe_single_scan(&key,expected,p.origin_nm,p.target_nm,p.speed_nm_s,true).map_err(|e|e.message)?;
        let started=clock.now();let mut hold_start=None;let mut held_first_end=None;let mut held=Vec::<f64>::new();let mut arrived=false;let mut stopped=false;let mut timed_out=false;let mut samples=Vec::<Value>::new();
        let hold_duration=loop {
            let elapsed=clock.now().saturating_sub(started).as_secs_f64();
            if hold_start.is_none()&&!stopped&&(p.stop_after_s.is_some_and(|s|elapsed>=s)||elapsed>=12.) {
                // Disclosed and authorized with the probe, never a RESET or replay.
                bus.control(&key,Control::ScanStop,true).map_err(|e|e.message)?;
                stopped=true;timed_out=elapsed>=12.;hold_start=Some(clock.now());
            }
            let read_start=clock.now();let sample=bus.status(&key).map_err(|e|e.message)?;
            let read_end=clock.now();let midpoint=(read_start.as_secs_f64()+read_end.as_secs_f64())/2.-started.as_secs_f64();
            let wavelength=sample.wavelength_nm;let complete=sample.operation_complete;
            samples.push(json!({"after_ack_s":midpoint,"status":sample}));
            report["observation"]["trajectory"]=json!(samples);
            if hold_start.is_none()&&complete&&(wavelength-p.target_nm).abs()<=0.005001 {arrived=true;hold_start=Some(clock.now());}
            if hold_start.is_some() {
                held.push(wavelength);
                // Getter latency cannot stand in for two distinct observations.
                // Use the first read's end and a later read's start so even slow
                // exchanges conservatively cover a full second of held position.
                if let Some(first_end)=held_first_end {
                    let duration=read_start.saturating_sub(first_end);
                    if held.len()>=2&&duration>=Duration::from_secs(1) {break duration;}
                } else {held_first_end=Some(read_end);}
            }
            if samples.len()>=160 {return Err("Observation capacity reached; outcome uncertain".into());}
            clock.wait(Duration::from_millis(100));
        };
        let low=held.iter().copied().fold(f64::INFINITY,f64::min);let high=held.iter().copied().fold(f64::NEG_INFINITY,f64::max);
        report["observation"]["arrival_observed"]=json!(arrived);
        report["observation"]["stop_command_sent"]=json!(stopped);
        report["observation"]["timeout_stop"]=json!(timed_out);
        report["observation"]["hold_span_nm"]=json!(high-low);
        report["observation"]["hold_duration_s"]=json!(hold_duration.as_secs_f64());
        let hold_observed=held.len()>=2&&hold_duration>=Duration::from_secs(1)&&high-low<=0.005001;
        report["observation"]["hold_observed"]=json!(hold_observed);
        report["observation"]["scan_settings_after"]=bus.scan_settings(&key).map_err(|e|e.message)?;
        if timed_out {return Err("Endpoint not observed within 12 seconds; one STOP sent. No production capability enabled".into());}
        if !hold_observed {return Err("Insufficient one-second hold evidence, or position exceeded controller-readback tolerance".into());}
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
