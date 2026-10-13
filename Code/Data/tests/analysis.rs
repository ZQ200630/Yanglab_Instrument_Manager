use serde_json::json;
use yang_data::*;
fn fixture(y: Vec<f64>) -> (WaveformRecord, AnalysisRequest) {
    let record = WaveformRecord {
        schema: 1,
        id: "11111111111111111111111111111111".into(),
        name: "known".into(),
        comment: String::new(),
        created_at_unix_ms: 0,
        source: json!({}),
        analyses: vec![],
        frames: vec![WaveformFrame {
            id: "22222222222222222222222222222222".into(),
            captured_at_unix_ms: 0,
            nominal_offset_s: None,
            channels: vec![ChannelWaveform {
                channel: "CH1".into(),
                samples: Samples::F64(y.clone()),
                calibration: Calibration {
                    x_increment_s: 0.001,
                    x_origin_s: 0.0,
                    x_reference: 0.0,
                    y_increment_v: 1.0,
                    y_origin: 0.0,
                    y_reference: 0.0,
                },
                metadata: json!({}),
            }],
        }],
    };
    let req = AnalysisRequest {
        record_id: record.id.clone(),
        frame_id: record.frames[0].id.clone(),
        channel: "CH1".into(),
        roi: SampleRange {
            start: 0,
            end: y.len(),
        },
        method: AnalysisMethod::SingleDrop,
        excursion_pm: 100.0,
        ramp_hz: 1.0,
        wavelength_nm: 1550.0,
        scan_speed_nm_s: 10.0,
        prominence_fraction: 0.05,
        min_distance_samples: 20,
    };
    (record, req)
}
#[test]
fn positive_lorentzian_width_has_physical_q_and_absolute_curve_indices() {
    // Width 20 pm, center 100 pm, scan 200 pm/s, 0.2 pm/sample.
    let y = (0..1001)
        .map(|i| {
            let x = i as f64 * 0.2;
            2.0 - 1.5 * 100.0 / ((x - 100.0).powi(2) + 100.0)
        })
        .collect();
    let (record, req) = fixture(y);
    let fit = analyze(&record, &req).unwrap();
    assert!((fit.result["fwhm_pm"].as_f64().unwrap() - 20.0).abs() < 0.01);
    assert!((fit.result["center_pm"].as_f64().unwrap() - 100.0).abs() < 0.01);
    assert!((fit.result["q"].as_f64().unwrap() - 77500.0).abs() < 50.0);
    assert!(fit.result["r_squared"].as_f64().unwrap() > 0.99999);
    assert_eq!(fit.curve.first().unwrap().x, 0.0);
    assert_eq!(fit.curve.last().unwrap().x, 1000.0);
    assert_eq!(record.frames[0].channels[0].samples.len(), 1001);
}
#[test]
fn fsr_uses_original_samples_and_scan_speed() {
    let y = (0..501)
        .map(|i| {
            let mut v = 1.0;
            for c in [50.0, 150.0, 250.0, 350.0, 450.0] {
                v -= 0.5 * (-((i as f64 - c) / 6.0).powi(2)).exp();
            }
            v
        })
        .collect();
    let (record, mut req) = fixture(y);
    req.method = AnalysisMethod::Fsr;
    let fit = analyze(&record, &req).unwrap();
    assert_eq!(fit.result["dip_count"], 5);
    assert!((fit.result["spacing_samples"].as_f64().unwrap() - 100.0).abs() < 1e-8);
    assert!((fit.result["fsr_nm"].as_f64().unwrap() - 1.0).abs() < 1e-8);
    assert_eq!(fit.markers[0].x, 50.0);
}
#[test]
fn invalid_and_degenerate_analysis_never_makes_successful_nan() {
    let (mut record, mut req) = fixture(vec![1.0; 100]);
    assert!(analyze(&record, &req).is_err());
    req.roi.end = 2;
    assert!(analyze(&record, &req).is_err());
    req.roi.end = 101;
    assert!(analyze(&record, &req).is_err());
    req.roi.end = 100;
    req.excursion_pm = 0.0;
    assert!(analyze(&record, &req).is_err());
    req.excursion_pm = 100.0;
    record.frames[0].channels[0].samples = Samples::F64(vec![f64::INFINITY; 100]);
    assert!(analyze(&record, &req).is_err());
}
#[test]
fn negative_scan_parameters_do_not_cancel_each_other_into_valid_q() {
    let y = (0..1001)
        .map(|i| {
            let x = i as f64 * 0.2;
            2.0 - 1.5 * 100.0 / ((x - 100.0).powi(2) + 100.0)
        })
        .collect();
    let (record, mut req) = fixture(y);
    req.excursion_pm = -100.0;
    req.ramp_hz = -1.0;
    assert!(matches!(analyze(&record, &req), Err(DataError::Invalid(_))));
}
#[test]
fn cancellation_interrupts_analysis_at_native_checkpoint() {
    let y = (0..50001)
        .map(|i| {
            let x = i as f64 / 250.0;
            2.0 - 100.0 / ((x - 100.0).powi(2) + 100.0)
        })
        .collect();
    let (record, req) = fixture(y);
    assert_eq!(
        analyze_with_cancel(&record, &req, &|| true).unwrap_err(),
        DataError::Cancelled
    );
    let calls = std::cell::Cell::new(0);
    let cancel = || {
        calls.set(calls.get() + 1);
        calls.get() >= 4
    };
    assert_eq!(
        analyze_with_cancel(&record, &req, &cancel).unwrap_err(),
        DataError::Cancelled
    );
}
