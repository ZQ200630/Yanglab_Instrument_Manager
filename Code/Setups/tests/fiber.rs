#[path = "../../Utils/tests/support/mdt_wire.rs"]
mod peer;
use std::{collections::BTreeMap, sync::Arc};
use yang_drivers::{
    clock::ManualClock,
    transport::{
        serial_abi::{SerialBackend, SerialIo},
        serial_discovery::DeviceRecord,
        ResourceBook,
    },
    DriverResult,
};
use yang_setups::{
    FiberConfig, FiberCouplingSetup, LogicalAxis, MemberBinding, StageSide, Vector3Um,
};
struct Ports {
    wires: BTreeMap<String, Arc<peer::Wire>>,
}
impl SerialBackend for Ports {
    fn enumerate(&self) -> DriverResult<Vec<DeviceRecord>> {
        Ok(self
            .wires
            .iter()
            .rev()
            .map(|(port, w)| DeviceRecord {
                port: port.clone(),
                instance_id: format!(
                    "USB\\VID_0403&PID_6001\\{}",
                    w.data.lock().unwrap().values["serial?"]
                ),
                parents: vec![],
                description: "Thorlabs MDT693B".into(),
            })
            .collect())
    }
    fn open(&self, p: &str) -> DriverResult<Box<dyn SerialIo>> {
        let w = self.wires.get(p.strip_prefix("\\\\.\\").unwrap()).unwrap();
        peer::Backend(w.clone()).open("\\\\.\\COM15")
    }
}
fn setup(
    both: bool,
) -> (
    FiberCouplingSetup,
    Arc<peer::Wire>,
    Arc<peer::Wire>,
    ResourceBook,
    Arc<Ports>,
) {
    let l = peer::Wire::new();
    let r = peer::Wire::new();
    r.data
        .lock()
        .unwrap()
        .values
        .insert("serial?".into(), "160721175410".into());
    let ports = Arc::new(Ports {
        wires: if both {
            BTreeMap::from([("COM9".into(), l.clone()), ("COM2".into(), r.clone())])
        } else {
            BTreeMap::from([("COM2".into(), r.clone())])
        },
    });
    let book = ResourceBook::isolated();
    let serials: Vec<_> = if both {
        vec!["160721175410".into(), "2110148249-10".into()]
    } else {
        vec!["160721175410".into()]
    };
    let s = FiberCouplingSetup::with_backend(
        &serials,
        FiberConfig::default(),
        book.clone(),
        Arc::new(ManualClock::default()),
        ports.clone(),
    )
    .unwrap();
    (s, l, r, book, ports)
}
fn adopt(s: &mut FiberCouplingSetup, side: StageSide) {
    let a = s.stage(side).baseline_attestation(true, true).unwrap();
    s.adopt_baseline(side, a).unwrap();
}
#[test]
fn serials_define_sides_not_com_order() {
    let (mut s, l, r, _, _) = setup(true);
    s.connect().unwrap();
    assert_eq!(
        s.stage(StageSide::Left).status().resource.as_deref(),
        Some("COM9")
    );
    assert_eq!(
        s.stage(StageSide::Right).status().resource.as_deref(),
        Some("COM2")
    );
    s.close().unwrap();
    assert!(l
        .commands()
        .iter()
        .chain(r.commands().iter())
        .all(|c| !c.contains('=')));
}
#[test]
fn one_side_is_supported() {
    let (mut s, _, _, _, _) = setup(false);
    s.connect().unwrap();
    assert_eq!(s.available_sides(), vec![StageSide::Right]);
    assert_eq!(s.missing_sides(), vec![StageSide::Left]);
    assert!(!s.stage(StageSide::Left).status().available);
    assert!(s
        .stage(StageSide::Left)
        .baseline_attestation(true, true)
        .is_err());
    s.close().unwrap();
}
#[test]
fn logical_mapping_and_toward_chip_signs() {
    let (mut s, l, r, _, _) = setup(true);
    s.connect().unwrap();
    adopt(&mut s, StageSide::Left);
    adopt(&mut s, StageSide::Right);
    let result = s
        .move_by_um(StageSide::Left, Vector3Um::new(0.1, 0., 0.).unwrap())
        .unwrap();
    assert!((result.voltage_delta_v[&LogicalAxis::X] - 0.375).abs() < 1e-12);
    assert_eq!(l.data.lock().unwrap().values["yvoltage?"], "30.375");
    assert_eq!(l.data.lock().unwrap().values["xvoltage?"], "20");
    s.move_by_um(StageSide::Right, Vector3Um::new(-0.1, 0., 0.).unwrap())
        .unwrap();
    assert_eq!(r.data.lock().unwrap().values["xvoltage?"], "19.625");
    s.move_by_um(StageSide::Left, Vector3Um::new(0., 0.1, 0.1).unwrap())
        .unwrap();
    assert_eq!(l.data.lock().unwrap().values["xvoltage?"], "20.375");
    assert_eq!(l.data.lock().unwrap().values["zvoltage?"], "40.375");
    s.move_by_um(StageSide::Right, Vector3Um::new(0., 0.1, 0.1).unwrap())
        .unwrap();
    assert_eq!(r.data.lock().unwrap().values["yvoltage?"], "30.375");
    assert_eq!(r.data.lock().unwrap().values["zvoltage?"], "40.375");
    assert!(s
        .move_by_um(StageSide::Left, Vector3Um::new(0.200001, 0., 0.).unwrap())
        .is_err());
    assert!(s
        .move_by_um(StageSide::Right, Vector3Um::new(-0.200001, 0., 0.).unwrap())
        .is_err());
    assert!(s
        .move_by_um(StageSide::Left, Vector3Um::new(0., 1.000001, 0.).unwrap())
        .is_err());
    s.close().unwrap();
}
#[test]
fn nominal_conversion_requires_authorization() {
    let (mut s, _, _, _, _) = setup(true);
    s.connect().unwrap();
    assert!(s
        .stage(StageSide::Left)
        .baseline_attestation(true, false)
        .is_err());
    assert!(s
        .move_by_um(StageSide::Left, Vector3Um::new(0.1, 0., 0.).unwrap())
        .is_err());
    adopt(&mut s, StageSide::Left);
    let st = s.stage(StageSide::Left).status();
    assert!(st.nominal_authorized);
    assert_eq!(
        st.estimated_position_um.unwrap(),
        Vector3Um::new(0., 0., 0.).unwrap()
    );
    s.close().unwrap();
}
#[test]
fn partial_motion_invalidates_without_rollback() {
    let (mut s, l, _, _, _) = setup(true);
    s.connect().unwrap();
    adopt(&mut s, StageSide::Left);
    l.data.lock().unwrap().fail_setter = Some(2);
    assert!(s
        .move_by_um(StageSide::Left, Vector3Um::new(0.1, 0.1, 0.).unwrap())
        .is_err());
    let st = s.stage(StageSide::Left).status();
    assert!(!st.baseline_known);
    assert!(st.estimated_position_um.is_none());
    assert!(st.motion_evidence.is_some());
    assert!(!l
        .commands()
        .iter()
        .any(|c| c == "allvoltage=0" || c == "xvoltage=20"));
    s.close().unwrap();
}
#[test]
fn setup_and_member_cannot_double_own() {
    let (mut s, _, _, book, ports) = setup(true);
    s.connect().unwrap();
    let mut member = yang_drivers::mdt::Mdt693b::with_backend(
        yang_drivers::mdt::MdtConfig {
            port: "COM9".into(),
            start_monitor: false,
            ..Default::default()
        },
        book,
        Arc::new(ManualClock::default()),
        ports,
    )
    .unwrap();
    assert!(member.connect().is_err());
    s.close().unwrap();
    member.connect().unwrap();
    member.close().unwrap();
}
#[test]
fn strict_config_and_explicit_bindings_preserve_limits() {
    let mut value: serde_json::Value =
        serde_json::from_slice(include_bytes!("../../../config/fiber_coupling.json")).unwrap();
    value["operator_limits_um"]["toward_chip"] = serde_json::json!(0.3);
    assert!(FiberConfig::from_json(&serde_json::to_vec(&value).unwrap()).is_err());
    let duplicate = include_str!("../../../config/fiber_coupling.json").replacen(
        "\"model\": \"MAX312D\"",
        "\"model\": \"MAX312D\", \"model\": \"MAX312D\"",
        1,
    );
    assert!(FiberConfig::from_json(duplicate.as_bytes()).is_err());
    assert!(FiberConfig::default()
        .with_bindings(vec![
            MemberBinding {
                serial: "2110148249-10".into(),
                port: "COM1".into()
            },
            MemberBinding {
                serial: "160721175410".into(),
                port: "com1".into()
            }
        ])
        .is_err());
    assert!(Vector3Um::new(f64::NAN, 0., 0.).is_err());
}
#[test]
fn external_manual_motion_and_stop_invalidate_session_estimate() {
    let (mut s, l, _, _, _) = setup(true);
    s.connect().unwrap();
    adopt(&mut s, StageSide::Left);
    l.data
        .lock()
        .unwrap()
        .values
        .insert("yvoltage?".into(), "30.1".into());
    assert!(s
        .move_by_um(StageSide::Left, Vector3Um::new(0.1, 0., 0.).unwrap())
        .is_err());
    assert!(!s.stage(StageSide::Left).status().baseline_known);
    adopt(&mut s, StageSide::Right);
    s.stop_handle().request_stop();
    assert!(!s.stage(StageSide::Right).status().baseline_known);
    s.close().unwrap();
}
#[test]
fn unopened_setup_close_is_confirmed_without_queries() {
    let (mut s, l, r, _, _) = setup(true);
    let report = s.close().unwrap();
    assert!(report.resources_released());
    assert!(l.commands().is_empty() && r.commands().is_empty());
    assert_eq!(s.close().unwrap(), report);
}
