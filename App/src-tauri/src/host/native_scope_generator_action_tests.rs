use super::{connection_checks, validate_action};
use serde_json::json;

#[test]
fn scope_generator_host_actions_admit_only_typed_family_core() {
    for (kind, name, args) in [
        ("scope", "read_status", json!({})), ("scope", "run", json!({})), ("scope", "stop", json!({})),
        ("scope", "set_channel_display", json!({"channel":4,"enabled":true})),
        ("scope", "set_memory_depth", json!({"points":250_000_000})),
        ("scope", "capture", json!({"channels":[1,3],"source":"screen"})),
        ("scope", "capture", json!({"channels":[1,2],"source":"memory","points":250_000_000,"byte_budget":536_870_912})),
        ("scope", "retry_staging", json!({"capture_id":"a".repeat(32)})),
        ("generator", "read_status", json!({})),
        ("generator", "configure_signal", json!({"channel":2,"waveform":"triangle","frequency_hz":50.,"amplitude_vpp":2.,"offset_v":0.})),
        ("generator", "set_output", json!({"channel":1,"enabled":false})),
        ("generator", "set_phase", json!({"channel":2,"degrees":360.})),
        ("generator", "set_load", json!({"channel":1,"load":{"kind":"high_z"}})),
        ("generator", "set_load", json!({"channel":1,"load":{"kind":"ohms","ohms":50.}})),
        ("generator", "set_polarity", json!({"channel":1,"polarity":"inverted"})),
    ] { assert!(validate_action(kind,name,&args).is_ok(),"{kind} {name} {args}"); }
    for kind in ["scope","generator"] {
        let config=json!({"driver_kind":kind,"domain":{"kind":"device","id":"a".repeat(32)}});
        assert_eq!(connection_checks(&config).unwrap(),vec![config]);
    }
}

#[test]
fn scope_generator_host_actions_reject_unsafe_shape_ranges_and_raw_commands() {
    for (kind,name,args) in [
        ("scope","read_status",json!({"command":"*RST"})),
        ("scope","set_channel_display",json!({"channel":0,"enabled":true})),
        ("scope","set_channel_display",json!({"channel":1,"enabled":1})),
        ("scope","set_memory_depth",json!({"points":250_000_001})),
        ("scope","capture",json!({"channels":[1,1],"source":"memory"})),
        ("scope","capture",json!({"channels":[],"source":"screen"})),
        ("scope","capture",json!({"channels":[5],"source":"memory"})),
        ("scope","capture",json!({"channels":[1],"source":"screen","points":1001})),
        ("scope","capture",json!({"channels":[1],"source":"memory","points":250_000_001})),
        ("scope","capture",json!({"channels":[1],"source":"memory","byte_budget":536_870_913})),
        ("scope","capture",json!({"channels":[1],"source":"raw"})),
        ("scope","retry_staging",json!({"capture_id":"../outside"})),
        ("generator","set_output",json!({"channel":3,"enabled":true})),
        ("generator","configure_signal",json!({"channel":1,"waveform":"USER:SCPI","frequency_hz":50.,"amplitude_vpp":2.,"offset_v":0.})),
        ("generator","configure_signal",json!({"channel":1,"waveform":"sine","frequency_hz":0.,"amplitude_vpp":2.,"offset_v":0.})),
        ("generator","configure_signal",json!({"channel":1,"waveform":"sine","frequency_hz":200_000_001.,"amplitude_vpp":2.,"offset_v":0.})),
        ("generator","configure_signal",json!({"channel":1,"waveform":"sine","frequency_hz":50.,"amplitude_vpp":10.,"offset_v":6.})),
        ("generator","set_phase",json!({"channel":1,"degrees":-1.})),
        ("generator","set_phase",json!({"channel":1,"phase_deg":90.})),
        ("generator","set_load",json!({"channel":1,"load":{"kind":"high_z","ohms":50}})),
        ("generator","set_load",json!({"channel":1,"load":{"kind":"ohms","ohms":0.}})),
        ("generator","set_polarity",json!({"channel":1,"polarity":"NEG"})),
        ("generator","raw_scpi",json!({"command":"OUTP1 ON"})),
    ] { assert!(validate_action(kind,name,&args).is_err(),"{kind} {name} {args}"); }
}

#[test]
fn scope_host_stopped_capture_admission_requires_exclusive_boolean_stop_choice() {
    for args in [
        json!({"channels":[1],"source":"memory","fresh":false,"stop_before":true}),
        json!({"channels":[1],"source":"screen","stop_before":true}),
        json!({"channels":[1],"source":"memory","fresh":true,"stop_before":false}),
    ] {
        assert!(validate_action("scope", "capture", &args).is_ok(), "{args}");
    }
    for args in [
        json!({"channels":[1],"source":"memory","fresh":true,"stop_before":true}),
        json!({"channels":[1],"source":"memory","fresh":"false","stop_before":true}),
        json!({"channels":[1],"source":"memory","stop_before":"true"}),
        json!({"channels":[1],"source":"memory","stop_before":null}),
        json!({"channels":[1],"source":"memory","stop_before":1}),
    ] {
        assert!(validate_action("scope", "capture", &args).is_err(), "{args}");
    }
}
