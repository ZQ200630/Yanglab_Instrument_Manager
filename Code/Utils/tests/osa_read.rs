#[path = "support/osa_wire.rs"]
mod support;
use support::*;
use yang_drivers::osa::{TraceId, TransferFormat};
#[test]
fn read_trace_never_changes_panel() {
    for format in [
        TransferFormat::Ascii,
        TransferFormat::Real32,
        TransferFormat::Real64,
    ] {
        let wire = Wire::new(2049, format);
        let mut osa = wire.osa();
        osa.connect().unwrap();
        let capture = osa.read_trace(TraceId::A, deadline()).unwrap();
        assert_eq!(capture.native_values().len(), 2049);
        assert!(osa.close().unwrap().resources_released());
        let data = wire.data.lock().unwrap();
        assert!(data.commands.iter().all(|c| c.contains('?')));
        assert_eq!(
            data.commands
                .iter()
                .filter(|c| c.starts_with(":TRACe:X?"))
                .count(),
            3
        );
        assert!(data.read_sizes.iter().all(|n| *n <= 4096));
    }
}
#[test]
fn changed_context_rejects_capture() {
    let wire = Wire::new(2, TransferFormat::Ascii);
    wire.data.lock().unwrap().changed_center = true;
    let mut osa = wire.osa();
    osa.connect().unwrap();
    assert!(osa.read_trace(TraceId::A, deadline()).is_err());
    osa.close().unwrap();
    let data = wire.data.lock().unwrap();
    assert_eq!(data.counts[":TRACe:X? TRA,1,2"], 1);
    assert!(data.commands.iter().all(|c| c.contains('?')));
}
#[test]
fn unchanged_context_is_unproven() {
    let wire = Wire::new(2, TransferFormat::Real64);
    let mut osa = wire.osa();
    osa.connect().unwrap();
    let capture = osa.read_trace(TraceId::A, deadline()).unwrap();
    assert_eq!(capture.consistency(), "unproven");
    assert_eq!(capture.wavelength_nm()[0], 1550.);
    assert_eq!(capture.context_before(), capture.context_after());
    osa.close().unwrap();
}
#[test]
fn native_complete_reply_uses_visa_completion_not_embedded_lf() {
    let wire = Wire::new(2, TransferFormat::Real64);
    let manager = wire.manager();
    let resource = manager.canonicalize("GPIB0::4::INSTR").unwrap();
    let mut session = manager.open(resource, deadline()).unwrap();
    session.write_all(b"*IDN?\n", deadline()).unwrap();
    assert_eq!(
        session.read_reply(256, deadline()).unwrap(),
        b"YOKOGAWA,AQ6370E,SN,FW\n"
    );
    assert!(session.close().unwrap().released);
}
#[test]
fn maximum_trace_uses_all_consecutive_bounded_ranges() {
    let wire = Wire::new(200001, TransferFormat::Real64);
    wire.data.lock().unwrap().fragment = 4096;
    let mut osa = wire.osa();
    osa.connect().unwrap();
    let capture = osa.read_trace(TraceId::A, deadline()).unwrap();
    assert_eq!(capture.wavelength_nm().len(), 200001);
    assert_eq!(capture.native_values()[200000], 199960.);
    let data = wire.data.lock().unwrap();
    let queries: Vec<_> = data
        .commands
        .iter()
        .filter(|c| c.starts_with(":TRACe:X?"))
        .collect();
    assert_eq!(queries.len(), 196);
    assert_eq!(queries[0], ":TRACe:X? TRA,1,1024");
    assert_eq!(queries[195], ":TRACe:X? TRA,199681,200001");
    drop(data);
    assert!(osa.close().unwrap().resources_released());
}
#[test]
fn unsupported_interpretations_and_nonactive_trace_never_read_samples() {
    for (command, value) in [
        (":TRACe:ACTive?", "TRB"),
        (":UNIT:X?", "1"),
        (":DISPlay:TRACe:Y1:SCALe:UNIT?", "2"),
        (":TRACe:ATTRibute?", "5"),
        (":TRACe:DATA:SNUMber? TRA", "0"),
    ] {
        let wire = Wire::new(2, TransferFormat::Ascii);
        wire.data
            .lock()
            .unwrap()
            .metadata
            .insert(command.into(), value.into());
        let mut osa = wire.osa();
        osa.connect().unwrap();
        assert!(osa.read_trace(TraceId::A, deadline()).is_err());
        assert!(wire
            .data
            .lock()
            .unwrap()
            .commands
            .iter()
            .all(|c| !c.starts_with(":TRACe:X?")));
        assert!(osa.close().unwrap().resources_released());
    }
}
#[test]
fn malformed_or_oversized_reply_is_not_retried_or_published() {
    for value in ["-40,".to_owned(), "1".repeat(65537)] {
        let wire = Wire::new(2, TransferFormat::Ascii);
        {
            let mut data = wire.data.lock().unwrap();
            data.fragment = 4096;
            data.metadata.insert(":TRACe:Y? TRA,1,2".into(), value);
        }
        let mut osa = wire.osa();
        osa.connect().unwrap();
        assert!(osa.read_trace(TraceId::A, deadline()).is_err());
        assert_eq!(wire.data.lock().unwrap().counts[":TRACe:Y? TRA,1,2"], 1);
        assert!(osa.close().unwrap().resources_released());
    }
}
