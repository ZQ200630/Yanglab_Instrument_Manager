use crate::{DataError, PlotPoint, SampleRange, WaveformRecord};
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AnalysisMethod {
    SingleDrop,
    Fsr,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AnalysisRequest {
    pub record_id: String,
    pub frame_id: String,
    pub channel: String,
    pub roi: SampleRange,
    pub method: AnalysisMethod,
    pub excursion_pm: f64,
    pub ramp_hz: f64,
    pub wavelength_nm: f64,
    pub scan_speed_nm_s: f64,
    pub prominence_fraction: f64,
    pub min_distance_samples: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AnalysisResult {
    pub schema: u32,
    pub id: String,
    pub record_id: String,
    pub frame_id: String,
    pub channel: String,
    pub roi: SampleRange,
    pub method: AnalysisMethod,
    pub algorithm: String,
    pub parameters: serde_json::Value,
    pub result: serde_json::Value,
    pub curve: Vec<PlotPoint>,
    pub markers: Vec<PlotPoint>,
    pub created_at_unix_ms: u64,
}
/// Pure, versioned analysis; never consumes display-reduced curves.
pub fn analyze(
    record: &WaveformRecord,
    request: &AnalysisRequest,
) -> Result<AnalysisResult, DataError> {
    analyze_with_cancel(record, request, &|| false)
}
pub fn analyze_with_cancel(
    record: &WaveformRecord,
    request: &AnalysisRequest,
    cancelled: &dyn Fn() -> bool,
) -> Result<AnalysisResult, DataError> {
    crate::npy::check(cancelled)?;
    if [
        request.excursion_pm,
        request.ramp_hz,
        request.wavelength_nm,
        request.scan_speed_nm_s,
        request.prominence_fraction,
    ]
    .iter()
    .any(|v| !v.is_finite())
    {
        return Err(DataError::Invalid(
            "analysis parameters must be finite".into(),
        ));
    }
    if record.id != request.record_id {
        return Err(DataError::Invalid(
            "analysis record identity mismatch".into(),
        ));
    }
    let f = record
        .frames
        .iter()
        .find(|f| f.id == request.frame_id)
        .ok_or_else(|| DataError::NotFound("analysis frame absent".into()))?;
    let c = f
        .channels
        .iter()
        .find(|c| c.channel == request.channel)
        .ok_or_else(|| DataError::NotFound("analysis channel absent".into()))?;
    c.validate_with_cancel(cancelled)?;
    request.roi.validate(c.samples.len())?;
    let n = request.roi.end - request.roi.start;
    if n < 8 {
        return Err(DataError::Invalid(
            "analysis needs at least eight ROI samples".into(),
        ));
    }
    let value = |i| c.calibration.voltage_v(c.samples.value(i).unwrap());
    let mut min_i = request.roi.start;
    let mut min = value(min_i);
    let mut max = min;
    for i in request.roi.start + 1..request.roi.end {
        if i % 16384 == 0 {
            crate::npy::check(cancelled)?;
        }
        let v = value(i);
        if v < min {
            min = v;
            min_i = i;
        }
        max = max.max(v);
    }
    if max - min <= 1e-12 * max.abs().max(1.0) {
        return Err(DataError::Invalid("flat ROI has no resolvable dip".into()));
    }
    let mut out = AnalysisResult {
        schema: 1,
        id: crate::new_id()?,
        record_id: record.id.clone(),
        frame_id: f.id.clone(),
        channel: c.channel.clone(),
        roi: request.roi.clone(),
        method: request.method.clone(),
        algorithm: String::new(),
        parameters: serde_json::to_value(request)?,
        result: serde_json::Value::Null,
        curve: vec![],
        markers: vec![],
        created_at_unix_ms: crate::unix_ms(),
    };
    match request.method {
        AnalysisMethod::SingleDrop => {
            let scan = 2.0 * request.ramp_hz * request.excursion_pm;
            if !scan.is_finite()
                || scan <= 0.0
                || request.ramp_hz <= 0.0
                || request.excursion_pm <= 0.0
                || !request.wavelength_nm.is_finite()
                || request.wavelength_nm <= 0.0
            {
                return Err(DataError::Invalid(
                    "Q requires positive finite excursion, ramp and wavelength".into(),
                ));
            }
            let span = (n - 1) as f64 * c.calibration.x_increment_s * scan;
            if !span.is_finite() || span <= 0.0 {
                return Err(DataError::Invalid("scan axis overflows".into()));
            }
            // Uniform original-index fit sampling, plus the actual global minimum. No display reducer.
            let fit_n = n.min(20_000);
            let mut indices: Vec<usize> = (0..fit_n)
                .map(|i| request.roi.start + (n - 1) * i / (fit_n - 1))
                .collect();
            indices.push(min_i);
            indices.sort_unstable();
            indices.dedup();
            let x: Vec<f64> = indices
                .iter()
                .map(|&i| (i - request.roi.start) as f64 / (n - 1) as f64)
                .collect();
            let y: Vec<f64> = indices.iter().map(|&i| value(i)).collect();
            let center = (min_i - request.roi.start) as f64 / (n - 1) as f64;
            let half = max - (max - min) / 2.0;
            let mut l = None;
            for i in (request.roi.start..min_i).rev() {
                if i % 16384 == 0 {
                    crate::npy::check(cancelled)?;
                }
                if value(i) >= half {
                    l = Some(i);
                    break;
                }
            }
            let mut r = None;
            for i in min_i + 1..request.roi.end {
                if i % 16384 == 0 {
                    crate::npy::check(cancelled)?;
                }
                if value(i) >= half {
                    r = Some(i);
                    break;
                }
            }
            let width = match (l, r) {
                (Some(l), Some(r)) => (r - l) as f64 / (n - 1) as f64,
                _ => 0.1,
            };
            let p = fit_lorentzian(
                &x,
                &y,
                [(max - min).ln(), center, width.ln(), max],
                cancelled,
            )?;
            let fwhm = p[2].exp() * span;
            let center_pm = p[1] * span;
            if fwhm <= span / (n - 1) as f64 * 0.5 || fwhm > 2.0 * span {
                return Err(DataError::Invalid(
                    "fitted linewidth is unresolved or outside ROI".into(),
                ));
            }
            let (r2, rmse) =
                statistics(&y, &x.iter().map(|&x| lorentz(x, &p)).collect::<Vec<_>>())?;
            let q = request.wavelength_nm * 1000.0 / fwhm;
            if !q.is_finite() {
                return Err(DataError::Invalid("Q overflows".into()));
            }
            out.algorithm = "inverted-lorentzian-positive-width-v1".into();
            out.parameters["fit_sampling"] = serde_json::json!({"policy":"uniform_original_indices_plus_global_minimum","maximum_samples":20001,"used_samples":x.len(),"center_origin":"ROI start","maximum_iterations":200});
            out.result = serde_json::json!({"q":q,"fwhm_pm":fwhm,"center_pm":center_pm,"r_squared":r2,"rmse_v":rmse,"amplitude_v":p[0].exp(),"baseline_v":p[3],"fit_status":"converged"});
            let count = n.min(8192);
            out.curve = (0..count)
                .map(|j| {
                    let i = request.roi.start + (n - 1) * j / (count - 1);
                    PlotPoint {
                        x: i as f64,
                        y: lorentz((i - request.roi.start) as f64 / (n - 1) as f64, &p),
                    }
                })
                .collect();
            out.markers.push(PlotPoint {
                x: request.roi.start as f64 + p[1] * (n - 1) as f64,
                y: lorentz(p[1], &p),
            });
        }
        AnalysisMethod::Fsr => {
            if !request.scan_speed_nm_s.is_finite()
                || request.scan_speed_nm_s <= 0.0
                || !request.prominence_fraction.is_finite()
                || request.prominence_fraction <= 0.0
                || request.prominence_fraction > 1.0
                || request.min_distance_samples == 0
            {
                return Err(DataError::Invalid(
                    "FSR requires positive scan speed, prominence in (0,1], and nonzero distance"
                        .into(),
                ));
            }
            let mut candidates = vec![];
            let mut i = request.roi.start + 1;
            while i + 1 < request.roi.end {
                if i % 16384 == 0 {
                    crate::npy::check(cancelled)?;
                }
                if value(i) < value(i - 1) {
                    let start = i;
                    while i + 1 < request.roi.end && value(i + 1) == value(start) {
                        if i % 16384 == 0 {
                            crate::npy::check(cancelled)?;
                        }
                        i += 1;
                    }
                    if i + 1 < request.roi.end && value(i) < value(i + 1) {
                        candidates.push((start + i) / 2);
                        if candidates.len() > 4096 {
                            return Err(DataError::Limit(
                                "FSR exceeds 4096 candidate dips; increase distance or narrow ROI"
                                    .into(),
                            ));
                        }
                    }
                }
                i += 1;
            }
            // Conservative adjacent-basin prominence, documented separately from SciPy.
            let mut ranked = vec![];
            for (k, &p) in candidates.iter().enumerate() {
                let l = if k == 0 {
                    request.roi.start
                } else {
                    candidates[k - 1]
                };
                let r = if k + 1 == candidates.len() {
                    request.roi.end - 1
                } else {
                    candidates[k + 1]
                };
                let basin_max = |start, end| -> Result<f64, DataError> {
                    let mut max = f64::NEG_INFINITY;
                    for j in start..=end {
                        if j % 16384 == 0 {
                            crate::npy::check(cancelled)?;
                        }
                        max = max.max(value(j));
                    }
                    Ok(max)
                };
                let left = basin_max(l, p)?;
                let right = basin_max(p, r)?;
                let prominence = left.min(right) - value(p);
                if prominence >= (max - min) * request.prominence_fraction {
                    ranked.push((p, prominence));
                }
            }
            ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            let mut peaks: Vec<usize> = vec![];
            for (p, _) in ranked {
                if peaks
                    .iter()
                    .all(|&j| p.abs_diff(j) >= request.min_distance_samples)
                {
                    peaks.push(p);
                }
            }
            peaks.sort_unstable();
            if peaks.len() < 2 {
                return Err(DataError::Invalid(
                    "FSR requires at least two resolved dips".into(),
                ));
            }
            let mut sy = vec![0.0];
            let mut sy2 = vec![0.0];
            let mut siy = vec![0.0];
            for (i, &v) in peaks.iter().enumerate() {
                sy.push(sy[i] + v as f64);
                sy2.push(sy2[i] + (v as f64).powi(2));
                siy.push(siy[i] + i as f64 * v as f64);
            }
            let mut best = (0usize, 2usize, 0.0, 0.0);
            let mut score = -1.0;
            for a in 0..peaks.len() - 1 {
                crate::npy::check(cancelled)?;
                for b in a + 2..=peaks.len() {
                    if b % 128 == 0 {
                        crate::npy::check(cancelled)?;
                    }
                    let m = (b - a) as f64;
                    let ysum = sy[b] - sy[a];
                    let sxy = siy[b] - siy[a] - a as f64 * ysum;
                    let sx = m * (m - 1.0) / 2.0;
                    let sxx = m * (m - 1.0) * (2.0 * m - 1.0) / 6.0;
                    let cov = sxy - sx * ysum / m;
                    let xx = sxx - sx * sx / m;
                    let yy = sy2[b] - sy2[a] - ysum * ysum / m;
                    if xx <= 0.0 || yy <= 0.0 {
                        continue;
                    }
                    let slope = cov / xx;
                    let r2 = (cov * cov / (xx * yy)).clamp(0.0, 1.0);
                    let s = r2 * m;
                    if slope > 0.0 && s > score {
                        score = s;
                        best = (a, b, slope, r2);
                    }
                }
            }
            if score < 0.0 {
                return Err(DataError::Invalid("degenerate FSR regression".into()));
            }
            let fsr = best.2 * c.calibration.x_increment_s * request.scan_speed_nm_s;
            if !fsr.is_finite() || fsr <= 0.0 {
                return Err(DataError::Invalid("FSR scan mapping overflows".into()));
            }
            out.algorithm = "fsr-adjacent-prominence-contiguous-regression-v1".into();
            out.parameters["subset_score"] =
                serde_json::json!("r_squared * dip_count (legacy comparison)");
            out.parameters["prominence_policy"] =
                serde_json::json!("conservative maxima in adjacent dip basins; no smoothing");
            out.result = serde_json::json!({"fsr_nm":fsr,"spacing_samples":best.2,"r_squared":best.3,"dip_count":best.1-best.0,"candidate_count":peaks.len()});
            out.markers = peaks[best.0..best.1]
                .iter()
                .map(|&i| PlotPoint {
                    x: i as f64,
                    y: value(i),
                })
                .collect();
        }
    }
    crate::npy::check(cancelled)?;
    Ok(out)
}
fn lorentz(x: f64, p: &[f64; 4]) -> f64 {
    let g = p[2].exp() / 2.0;
    p[3] - p[0].exp() * g * g / ((x - p[1]).powi(2) + g * g)
}
fn statistics(y: &[f64], fit: &[f64]) -> Result<(f64, f64), DataError> {
    let mean = y.iter().sum::<f64>() / y.len() as f64;
    let ss = y.iter().map(|v| (v - mean).powi(2)).sum::<f64>();
    let err = y.iter().zip(fit).map(|(a, b)| (a - b).powi(2)).sum::<f64>();
    let r2 = 1.0 - err / ss;
    let rmse = (err / y.len() as f64).sqrt();
    if !r2.is_finite() || !rmse.is_finite() {
        Err(DataError::Invalid("nonfinite residual statistics".into()))
    } else {
        Ok((r2, rmse))
    }
}
fn fit_lorentzian(
    x: &[f64],
    y: &[f64],
    mut p: [f64; 4],
    cancelled: &dyn Fn() -> bool,
) -> Result<[f64; 4], DataError> {
    let cost = |p: &[f64; 4]| {
        x.iter()
            .zip(y)
            .map(|(&x, &y)| (y - lorentz(x, p)).powi(2))
            .sum::<f64>()
    };
    let mut loss = cost(&p);
    let mut lambda = 1e-3;
    for _ in 0..200 {
        crate::npy::check(cancelled)?;
        let mut a = [[0.0; 4]; 4];
        let mut b = [0.0; 4];
        for (&x, &y) in x.iter().zip(y) {
            let amp = p[0].exp();
            let g2 = (p[2].exp() / 2.0).powi(2);
            let d = x - p[1];
            let den = d * d + g2;
            let j = [
                -amp * g2 / den,
                -2.0 * amp * g2 * d / (den * den),
                -2.0 * amp * g2 * d * d / (den * den),
                1.0,
            ];
            let residual = y - lorentz(x, &p);
            for r in 0..4 {
                b[r] += j[r] * residual;
                for c in 0..4 {
                    a[r][c] += j[r] * j[c];
                }
            }
        }
        for i in 0..4 {
            a[i][i] += lambda * a[i][i].max(1e-12);
        }
        let Some(delta) = solve4(a, b) else {
            lambda *= 10.0;
            continue;
        };
        let mut next = p;
        for i in 0..4 {
            next[i] += delta[i];
        }
        if next.iter().any(|v| !v.is_finite())
            || next[1] < 0.0
            || next[1] > 1.0
            || next[2] < -20.0
            || next[2] > 2.0
        {
            lambda *= 10.0;
            continue;
        }
        let next_loss = cost(&next);
        if next_loss < loss {
            let gain = loss - next_loss;
            p = next;
            loss = next_loss;
            lambda = (lambda / 3.0).max(1e-12);
            if gain <= 1e-12 * (1.0 + loss)
                || delta.iter().map(|v| v.abs()).fold(0.0, f64::max) < 1e-9
            {
                return Ok(p);
            }
        } else {
            if loss < 1e-20 * y.len() as f64 {
                return Ok(p);
            }
            lambda *= 10.0;
        }
        if lambda > 1e20 {
            break;
        }
    }
    Err(DataError::Invalid("Lorentzian fit did not converge".into()))
}
fn solve4(mut a: [[f64; 4]; 4], mut b: [f64; 4]) -> Option<[f64; 4]> {
    for i in 0..4 {
        let pivot = (i..4).max_by(|&j, &k| a[j][i].abs().total_cmp(&a[k][i].abs()))?;
        if a[pivot][i].abs() < 1e-20 {
            return None;
        }
        a.swap(i, pivot);
        b.swap(i, pivot);
        let d = a[i][i];
        for k in i..4 {
            a[i][k] /= d;
        }
        b[i] /= d;
        for j in 0..4 {
            if i != j {
                let f = a[j][i];
                for k in i..4 {
                    a[j][k] -= f * a[i][k];
                }
                b[j] -= f * b[i];
            }
        }
    }
    Some(b)
}
