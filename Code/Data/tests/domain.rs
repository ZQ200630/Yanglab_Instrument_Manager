use serde_json::json;
use yang_data::*;
fn calibration() -> Calibration {
    Calibration {
        x_increment_s: 0.5,
        x_origin_s: 2.0,
        x_reference: 1.0,
        y_increment_v: 0.25,
        y_origin: -2.0,
        y_reference: 128.0,
    }
}
fn channel() -> ChannelWaveform {
    ChannelWaveform {
        channel: "CH1".into(),
        samples: Samples::U8(vec![0, 128, 255]),
        calibration: calibration(),
        metadata: json!({}),
    }
}
#[test]
fn unsigned_codes_and_independent_absolute_calibration() {
    assert_eq!(Samples::U8(vec![255]).value(0), Some(255.0));
    assert_eq!(Samples::U16(vec![65535]).value(0), Some(65535.0));
    assert_eq!(Samples::I8(vec![-1]).value(0), Some(-1.0));
    let ch = channel();
    let values: Vec<_> = ch
        .calibrated(SampleRange { start: 1, end: 3 })
        .unwrap()
        .collect();
    assert_eq!(values, vec![(1, 2.0, -2.0), (2, 2.5, 29.75)]);
    let mut other = ch.clone();
    other.calibration.x_origin_s = 20.0;
    assert_eq!(other.calibration.time_s(2), 20.5);
    assert!(ch.calibrated(SampleRange { start: 2, end: 4 }).is_err());
}
#[test]
fn extrema_keep_original_indices_and_tail_with_bounded_output() {
    let mut ch = channel();
    ch.samples = Samples::U8(vec![10, 11, 255, 12, 0, 13, 14, 15, 17]);
    let points = reduce_viewport(&ch, SampleRange { start: 1, end: 9 }, 6).unwrap();
    assert!(points.len() <= 6);
    assert_eq!(points.first().unwrap().x, 1.0);
    assert_eq!(points.last().unwrap().x, 8.0);
    assert!(points.iter().any(|p| p.x == 2.0 && p.y == 29.75));
    assert!(points.iter().any(|p| p.x == 4.0 && p.y == -34.0));
}
#[test]
fn invalid_waveform_is_rejected_before_numeric_work() {
    let mut ch = channel();
    assert!(ch.validate().is_ok());
    ch.samples = Samples::F64(vec![f64::NAN]);
    assert!(ch.validate().is_err());
    ch.samples = Samples::U8(vec![1]);
    ch.calibration.x_increment_s = 0.0;
    assert!(ch.validate().is_err());
}
