use serde_json::json;
use yang_worker::{
    actions::parse,
    session::{DriverFactory, SystemFactory},
};
#[test]
fn all_catalog_actions_have_typed_dispatch() {
    let cases = [
        ("osa", "read_trace", json!({})),
        ("osa", "acquire", json!({"trace":"B"})),
        ("voltage", "set_channel", json!({"channel":1,"voltage":0.1})),
        ("voltage", "set_all", json!({"values":[0,0,0,0,0,0,0,0]})),
        ("voltage", "zero", json!({})),
        ("gain", "set_temperature", json!({"temperature_c":22})),
        ("gain", "set_current", json!({"current_ma":10})),
        ("gain", "enable_tec", json!({})),
        ("gain", "disable_tec", json!({})),
        ("gain", "wait_stable", json!({"timeout_s":6})),
        ("gain", "enable_current", json!({})),
        ("gain", "disable_current", json!({})),
        ("pm400", "measure_power", json!({})),
        ("pm400", "measure_kind", json!({"kind":"temperature"})),
        (
            "pm400",
            "read_setting",
            json!({"setting":"sense.wavelength_nm"}),
        ),
        (
            "pm400",
            "write_setting",
            json!({"setting":"sense.power_unit","value":"W"}),
        ),
        (
            "pm400",
            "run_maintenance",
            json!({"command":"measurement.fetch"}),
        ),
        ("mdt", "read_status", json!({})),
        (
            "fiber",
            "adopt_baseline",
            json!({"side":"left","confirm":true,"allow_nominal":true}),
        ),
        ("fiber", "move", json!({"side":"right","dx":-0.1})),
    ];
    for (kind, name, args) in cases {
        assert!(parse(kind, name, &args).is_ok(), "{kind}/{name}");
    }
}
#[test]
fn bad_arguments_are_rejected_before_any_native_call() {
    for (kind, name, args) in [
        ("voltage", "set_channel", json!({"channel":0,"voltage":1})),
        (
            "voltage",
            "set_channel",
            json!({"channel":1,"voltage":14.01}),
        ),
        ("voltage", "set_all", json!({"values":[0,0]})),
        ("gain", "set_current", json!({"current_ma":201})),
        ("gain", "set_temperature", json!({"temperature_c":14})),
        ("gain", "enable_current", json!({"force":true})),
        (
            "pm400",
            "write_setting",
            json!({"setting":"input.adapter_type","value":"thermal"}),
        ),
        ("pm400", "read_setting", json!({"setting":"raw_scpi"})),
        (
            "pm400",
            "write_setting",
            json!({"setting":"sense.power_unit","value":"dbm"}),
        ),
        (
            "pm400",
            "write_setting",
            json!({"setting":"system.date","value":"2025-02-30"}),
        ),
        ("fiber", "move", json!({"side":"right","dx":-0.20001})),
        (
            "fiber",
            "adopt_baseline",
            json!({"side":"left","confirm":false}),
        ),
    ] {
        assert!(parse(kind, name, &args).is_err(), "{kind}/{name}");
    }
}
#[test]
fn factories_are_lazy_and_complete_without_native_dependencies() {
    let factory = SystemFactory::new(std::sync::Arc::new(
        yang_drivers::clock::SystemClock::default(),
    ));
    for (kind, model, profile, params) in [
        (
            "osa",
            "aq6370",
            "gpib-visa",
            json!({"resource":"GPIB0::4::INSTR"}),
        ),
        ("voltage", "voltage", "ch340-serial", json!({"port":"COM7"})),
        ("gain", "gain", "cp210x-serial", json!({"port":"COM8"})),
        (
            "pm400",
            "pm400",
            "usb-visa",
            json!({"resource":"USB0::0x1313::0x8075::P1::INSTR"}),
        ),
        ("mdt", "mdt693b", "serial", json!({"port":"COM9"})),
    ] {
        let config = yang_protocol::DomainConfig {
            domain: yang_protocol::DomainRef {
                kind: "device".into(),
                id: "b".repeat(32),
            },
            driver_kind: kind.into(),
            model_id: model.into(),
            profile_id: Some(profile.into()),
            params,
            expected_identity: json!({}),
            config_rev: 1,
            members: vec![],
        };
        let session = factory.create(&config).unwrap();
        assert!(!session.has_responsibility());
        assert!(!factory.auxiliary_responsibility());
        let mut session = session;
        assert!(session.close().unwrap().resources_released());
    }
}
