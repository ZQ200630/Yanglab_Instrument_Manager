use std::time::{Duration, SystemTime};
use yang_drivers::osa::*;
fn params(count: usize) -> TraceContextParams {
    TraceContextParams {
        transfer_format: TransferFormat::Ascii,
        sample_count: count,
        spacing: 0,
        level_unit: 0,
        x_unit: 0,
        trace_attribute: 0,
        active_trace: TraceId::A,
        center_m: 1.55e-6,
        span_m: 1e-9,
        resolution_m: 2e-11,
        sweep_mode: 2,
    }
}
fn timing() -> ReadTiming {
    ReadTiming {
        started_utc: SystemTime::UNIX_EPOCH + Duration::from_secs(1791244800),
        finished_utc: SystemTime::UNIX_EPOCH + Duration::from_secs(1791244801),
        elapsed: Duration::from_secs(1),
        decode: Duration::from_millis(1),
        io: Duration::from_millis(900),
    }
}
fn hex(text: &str) -> Vec<u8> {
    text.as_bytes()
        .chunks_exact(2)
        .map(|c| u8::from_str_radix(std::str::from_utf8(c).unwrap(), 16).unwrap())
        .collect()
}
#[test]
fn ascii_real32_real64_exact_values() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/osa/samples.json")).unwrap();
    for (format, reply) in [
        (
            TransferFormat::Ascii,
            fixture["ascii"].as_str().unwrap().as_bytes().to_vec(),
        ),
        (
            TransferFormat::Real32,
            hex(fixture["real32_hex"].as_str().unwrap()),
        ),
        (
            TransferFormat::Real64,
            hex(fixture["real64_hex"].as_str().unwrap()),
        ),
    ] {
        assert_eq!(
            decode_trace_reply(&reply, format, 2).unwrap(),
            vec![-40., -30.]
        );
    }
    let value = 0.1_f32;
    let mut block = b"#14".to_vec();
    block.extend(value.to_le_bytes());
    let widened = decode_trace_reply(&block, TransferFormat::Real32, 1).unwrap()[0];
    assert_eq!(widened.to_bits(), f64::from(value).to_bits());
    assert_ne!(widened.to_bits(), 0.1_f64.to_bits());
}
#[test]
fn truncated_block_is_rejected() {
    for reply in [
        b"#0payload".as_slice(),
        b"#",
        b"#x8",
        b"#18abc",
        b"#28abcd",
        b"#1999999999",
        b"#9167772160",
        b"#1-8",
        b"#1a",
        b"#1800000000extra",
    ] {
        assert!(decode_trace_reply(reply, TransferFormat::Real32, 2).is_err());
    }
    assert!(decode_trace_reply(b"-40,-30", TransferFormat::Ascii, 2).is_ok());
}
#[test]
fn nonfinite_and_density_are_rejected() {
    for reply in [
        b"nan,-30".as_slice(),
        b"inf,-30",
        b"1e999,-30",
        b"-40,, -30",
        b"-40,-30garbage",
        b"-40,-30\nX",
        b"-40,\xff",
        b"-40\n,-30",
        b"-40,",
        b"0x1,-30",
    ] {
        assert!(decode_trace_reply(reply, TransferFormat::Ascii, 2).is_err());
    }
    assert!(decode_trace_reply(
        b"#18\x00\x00\xc0\x7f\x00\x00\xf0\xc1",
        TransferFormat::Real32,
        2
    )
    .is_err());
    assert_eq!(
        TraceContext::new(params(2)).unwrap().native_unit().unwrap(),
        NativeUnit::Dbm
    );
    for (spacing, level, attribute) in [(0, 2, 0), (1, 3, 0), (0, 0, 5), (0, 1, 0), (1, 0, 0)] {
        let mut raw = params(2);
        raw.spacing = spacing;
        raw.level_unit = level;
        raw.trace_attribute = attribute;
        assert!(TraceContext::new(raw).unwrap().native_unit().is_err());
    }
    let mut raw = params(2);
    raw.x_unit = 1;
    assert!(TraceContext::new(raw).is_err());
}
#[test]
fn trace_limits_are_exact() {
    assert!(TraceContext::new(params(200001)).is_ok());
    assert!(TraceContext::new(params(200002)).is_err());
    assert!(TraceContext::new(params(0)).is_err());
    let reply = vec!["1"; 1024].join(",");
    assert_eq!(
        decode_trace_reply(reply.as_bytes(), TransferFormat::Ascii, 1024)
            .unwrap()
            .len(),
        1024
    );
    assert!(decode_trace_reply(reply.as_bytes(), TransferFormat::Ascii, 1025).is_err());
    assert!(decode_trace_reply(&vec![b'1'; 65537], TransferFormat::Ascii, 1).is_err());
    let wavelengths: Vec<_> = (0..200001).map(|i| 1500. + f64::from(i) * 0.001).collect();
    let context = TraceContext::new(params(200001)).unwrap();
    let capture = TraceCapture::new(
        wavelengths,
        vec![-40.; 200001],
        NativeUnit::Dbm,
        TraceId::A,
        "YOKOGAWA,AQ6370E,SN,FW".into(),
        timing(),
        context.clone(),
        context,
    )
    .unwrap();
    assert_eq!(capture.native_values().len(), 200001);
    assert_eq!(capture.consistency(), "unproven");
}
#[test]
fn capture_preserves_native_watts_and_rejects_bad_axis_or_context() {
    let mut raw = params(2);
    raw.spacing = 1;
    raw.level_unit = 1;
    let context = TraceContext::new(raw).unwrap();
    let capture = TraceCapture::new(
        vec![1550., 1551.],
        vec![0.001, 0.01],
        NativeUnit::Watt,
        TraceId::A,
        "OSA".into(),
        timing(),
        context.clone(),
        context.clone(),
    )
    .unwrap();
    assert_eq!(capture.native_values(), &[0.001, 0.01]);
    assert_eq!(capture.power_dbm().unwrap(), vec![0., 10.]);
    let zero = TraceCapture::new(
        vec![1550., 1551.],
        vec![0., 0.01],
        NativeUnit::Watt,
        TraceId::A,
        "OSA".into(),
        timing(),
        context.clone(),
        context.clone(),
    )
    .unwrap();
    assert!(zero.power_dbm().is_none());
    for (x, y) in [
        (vec![1551., 1550.], vec![0., 0.01]),
        (vec![1550., 1550.], vec![0., 0.01]),
        (vec![0., 1550.], vec![0., 0.01]),
        (vec![1550., 1551.], vec![-0.001, 0.01]),
        (vec![1550., 1551.], vec![f64::NAN, 0.01]),
        (vec![1550.], vec![0.]),
    ] {
        assert!(TraceCapture::new(
            x,
            y,
            NativeUnit::Watt,
            TraceId::A,
            "OSA".into(),
            timing(),
            context.clone(),
            context.clone()
        )
        .is_err());
    }
}

#[test]
fn context_and_read_interval_preserve_reviewed_metadata() {
    assert_eq!(
        serde_json::to_value(TransferFormat::Real32).unwrap(),
        serde_json::json!("REAL,32")
    );
    assert_eq!(
        serde_json::to_value(NativeUnit::Watt).unwrap(),
        serde_json::json!("W")
    );
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/osa/samples.json")).unwrap();
    let context = TraceContext::new(params(2)).unwrap();
    assert_eq!(serde_json::to_value(&context).unwrap(), fixture["context"]);
    let mut zero_span = params(2);
    zero_span.span_m = 0.;
    assert!(TraceContext::new(zero_span).is_ok());
    for (center, span, resolution) in [
        (0., 1e-9, 2e-11),
        (1.55e-6, -1., 2e-11),
        (1.55e-6, 1e-9, 0.),
        (f64::NAN, 1e-9, 2e-11),
        (1.55e-6, f64::INFINITY, 2e-11),
    ] {
        let mut raw = params(2);
        raw.center_m = center;
        raw.span_m = span;
        raw.resolution_m = resolution;
        assert!(TraceContext::new(raw).is_err());
    }
    let mut changed = params(2);
    changed.center_m = 1.56e-6;
    assert!(TraceCapture::new(
        vec![1550., 1551.],
        vec![-40., -30.],
        NativeUnit::Dbm,
        TraceId::A,
        "OSA".into(),
        timing(),
        context.clone(),
        TraceContext::new(changed).unwrap()
    )
    .is_err());
    let mut adjusted = timing();
    adjusted.finished_utc = adjusted.started_utc - Duration::from_secs(1);
    let capture = TraceCapture::new(
        vec![1550., 1551.],
        vec![-40., -30.],
        NativeUnit::Dbm,
        TraceId::A,
        "OSA".into(),
        adjusted,
        context.clone(),
        context,
    )
    .unwrap();
    assert_eq!(capture.timing().elapsed, Duration::from_secs(1));
    assert_eq!(capture.spectrum().unwrap().power_dbm(), &[-40., -30.]);
    for (input, expected) in [("A", TraceId::A), (" trg ", TraceId::G)] {
        assert_eq!(input.parse::<TraceId>().unwrap(), expected);
    }
    assert!("TRH".parse::<TraceId>().is_err());
    assert_eq!(
        " real, 32 ".parse::<TransferFormat>().unwrap(),
        TransferFormat::Real32
    );
    assert!("REAL,16".parse::<TransferFormat>().is_err());
}
