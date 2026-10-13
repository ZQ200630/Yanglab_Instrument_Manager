# Frozen waveform contract v1

This is the producer/consumer contract for the experiment/analysis implementation. Crate `yang-data` exports these names from its root. Rust members are public and serde uses snake_case. Structs derive Clone, Debug, Serialize and Deserialize where appropriate. IDs are 32 lowercase hex digits. Sample range end is exclusive.

```rust
pub enum Samples { U8(Vec<u8>), I8(Vec<i8>), U16(Vec<u16>), I16(Vec<i16>), F64(Vec<f64>) }
pub struct Calibration {
    pub x_increment_s: f64, pub x_origin_s: f64, pub x_reference: f64,
    pub y_increment_v: f64, pub y_origin: f64, pub y_reference: f64,
}
pub struct ChannelWaveform {
    pub channel: String, pub samples: Samples,
    pub calibration: Calibration, pub metadata: serde_json::Value,
}
pub struct WaveformFrame {
    pub id: String, pub captured_at_unix_ms: u64,
    pub nominal_offset_s: Option<f64>, pub channels: Vec<ChannelWaveform>,
}
pub struct WaveformRecord {
    pub schema: u32, pub id: String, pub name: String, pub comment: String,
    pub created_at_unix_ms: u64, pub source: serde_json::Value,
    pub frames: Vec<WaveformFrame>, pub analyses: Vec<AnalysisResult>,
}
pub struct SampleRange { pub start: usize, pub end: usize }
pub struct PlotPoint { pub x: f64, pub y: f64 }
pub enum AnalysisMethod { SingleDrop, Fsr }
pub struct AnalysisRequest {
    pub record_id: String, pub frame_id: String, pub channel: String,
    pub roi: SampleRange, pub method: AnalysisMethod,
    pub excursion_pm: f64, pub ramp_hz: f64, pub wavelength_nm: f64,
    pub scan_speed_nm_s: f64, pub prominence_fraction: f64,
    pub min_distance_samples: usize,
}
pub struct AnalysisResult {
    pub schema: u32, pub id: String, pub record_id: String, pub frame_id: String,
    pub channel: String, pub roi: SampleRange, pub method: AnalysisMethod,
    pub algorithm: String, pub parameters: serde_json::Value,
    pub result: serde_json::Value, pub curve: Vec<PlotPoint>,
    pub markers: Vec<PlotPoint>, pub created_at_unix_ms: u64,
}
pub fn analyze(record: &WaveformRecord, request: &AnalysisRequest) -> Result<AnalysisResult, DataError>;
```

Calibration is normalized: `time_s(i) = (i - x_reference) * x_increment_s + x_origin_s`; `voltage_v(raw) = (raw - y_reference) * y_increment_v + y_origin`. Thus legacy Rigol Y origin in ADC codes becomes `y_origin = -legacy_yorigin * legacy_yincrement`. Metadata retains the original convention/preamble. CSV/NPZ calibrated arrays use identity Y calibration; never scale them twice.

`Samples::len() -> usize` and `Samples::value(index: usize) -> Option<f64>` expose raw numeric codes. `Calibration::time_s(index: usize) -> f64`, `Calibration::voltage_v(raw: f64) -> f64` and `ChannelWaveform::validate() -> Result<(),DataError>` are shared helpers. Plot X is the absolute sample index and Y is calibrated volts; UI chooses displayed axis units independently. Analysis results use `q`, `fwhm_pm`, `center_pm`, `r_squared`, `rmse_v` for SingleDrop and `fsr_nm`, `spacing_samples`, `r_squared`, `dip_count` for FSR when meaningful. No NaN in results.

New driver capture returns one `WaveformFrame`; the acquisition job assembles records. Driver and data store must retain raw samples. Each channel is calibrated independently. Full sample arrays are native only and are never included in status/snapshot/UI RPC replies.

Representation and storage bounds are separate from rendering limits. A model may advertise up to 250M points but the configured request byte budget must be checked before allocation; over-budget requests fail explicitly. Consumer preview requests are bounded to 8192 returned points per channel and chunks to the existing 16384-byte transfer maximum.
