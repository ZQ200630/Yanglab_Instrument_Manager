use crate::DataError;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Samples {
    U8(Vec<u8>),
    I8(Vec<i8>),
    U16(Vec<u16>),
    I16(Vec<i16>),
    F64(Vec<f64>),
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Calibration {
    pub x_increment_s: f64,
    pub x_origin_s: f64,
    pub x_reference: f64,
    pub y_increment_v: f64,
    pub y_origin: f64,
    pub y_reference: f64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ChannelWaveform {
    pub channel: String,
    pub samples: Samples,
    pub calibration: Calibration,
    pub metadata: serde_json::Value,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WaveformFrame {
    pub id: String,
    pub captured_at_unix_ms: u64,
    pub nominal_offset_s: Option<f64>,
    pub channels: Vec<ChannelWaveform>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WaveformRecord {
    pub schema: u32,
    pub id: String,
    pub name: String,
    pub comment: String,
    pub created_at_unix_ms: u64,
    pub source: serde_json::Value,
    pub frames: Vec<WaveformFrame>,
    pub analyses: Vec<crate::AnalysisResult>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SampleRange {
    pub start: usize,
    pub end: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PlotPoint {
    pub x: f64,
    pub y: f64,
}
impl Samples {
    pub fn len(&self) -> usize {
        match self {
            Self::U8(v) => v.len(),
            Self::I8(v) => v.len(),
            Self::U16(v) => v.len(),
            Self::I16(v) => v.len(),
            Self::F64(v) => v.len(),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn value(&self, index: usize) -> Option<f64> {
        match self {
            Self::U8(v) => v.get(index).map(|&x| x as f64),
            Self::I8(v) => v.get(index).map(|&x| x as f64),
            Self::U16(v) => v.get(index).map(|&x| x as f64),
            Self::I16(v) => v.get(index).map(|&x| x as f64),
            Self::F64(v) => v.get(index).copied(),
        }
    }
    pub fn byte_len(&self) -> usize {
        self.len().saturating_mul(match self {
            Self::U8(_) | Self::I8(_) => 1,
            Self::U16(_) | Self::I16(_) => 2,
            Self::F64(_) => 8,
        })
    }
}
impl Calibration {
    pub fn time_s(&self, index: usize) -> f64 {
        (index as f64 - self.x_reference) * self.x_increment_s + self.x_origin_s
    }
    pub fn voltage_v(&self, raw: f64) -> f64 {
        (raw - self.y_reference) * self.y_increment_v + self.y_origin
    }
    pub fn validate(&self) -> Result<(), DataError> {
        if [
            self.x_increment_s,
            self.x_origin_s,
            self.x_reference,
            self.y_increment_v,
            self.y_origin,
            self.y_reference,
        ]
        .iter()
        .any(|v| !v.is_finite())
            || self.x_increment_s <= 0.0
            || self.y_increment_v == 0.0
        {
            return Err(DataError::Invalid("calibration must be finite with positive time increment and nonzero voltage increment".into()));
        }
        Ok(())
    }
}
impl ChannelWaveform {
    pub fn validate(&self) -> Result<(), DataError> {
        self.validate_with_cancel(&|| false)
    }
    pub fn validate_with_cancel(&self, cancelled: &dyn Fn() -> bool) -> Result<(), DataError> {
        crate::npy::check(cancelled)?;
        self.calibration.validate()?;
        if self.channel.trim().is_empty() || self.channel.len() > 128 || self.samples.is_empty() {
            return Err(DataError::Invalid(
                "channel name and samples must be nonempty".into(),
            ));
        }
        if self.samples.len() > 250_000_000 {
            return Err(DataError::Limit(
                "channel exceeds 250M sample representation limit".into(),
            ));
        }
        if !self.calibration.time_s(self.samples.len() - 1).is_finite() {
            return Err(DataError::Invalid("calibrated time axis overflows".into()));
        }
        for i in 0..self.samples.len() {
            if i % 16384 == 0 {
                crate::npy::check(cancelled)?;
            }
            let raw = self.samples.value(i).unwrap();
            if !raw.is_finite() || !self.calibration.voltage_v(raw).is_finite() {
                return Err(DataError::Invalid(format!("nonfinite sample at index {i}")));
            }
        }
        Ok(())
    }
    pub fn calibrated(
        &self,
        roi: SampleRange,
    ) -> Result<impl Iterator<Item = (usize, f64, f64)> + '_, DataError> {
        self.validate()?;
        roi.validate(self.samples.len())?;
        Ok((roi.start..roi.end).map(|i| {
            (
                i,
                self.calibration.time_s(i),
                self.calibration.voltage_v(self.samples.value(i).unwrap()),
            )
        }))
    }
}
impl SampleRange {
    pub fn validate(&self, len: usize) -> Result<(), DataError> {
        if self.start >= self.end || self.end > len {
            Err(DataError::Invalid(
                "ROI must be nonempty and inside the channel; end is exclusive".into(),
            ))
        } else {
            Ok(())
        }
    }
}
pub fn valid_id(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
impl WaveformRecord {
    pub fn validate(&self) -> Result<(), DataError> {
        self.validate_with_cancel(&|| false)
    }
    pub fn validate_with_cancel(&self, cancelled: &dyn Fn() -> bool) -> Result<(), DataError> {
        crate::npy::check(cancelled)?;
        if self.schema != 1
            || !valid_id(&self.id)
            || self.frames.is_empty()
            || self.frames.len() > 9999
            || self.name.len() > 1024
            || self.comment.len() > 65536
        {
            return Err(DataError::Invalid(
                "invalid waveform schema, identity, labels or frame count".into(),
            ));
        }
        let mut ids = std::collections::HashSet::new();
        for f in &self.frames {
            if !valid_id(&f.id)
                || !ids.insert(&f.id)
                || f.channels.is_empty()
                || f.channels.len() > 64
                || f.nominal_offset_s.is_some_and(|v| !v.is_finite())
            {
                return Err(DataError::Invalid(
                    "invalid or duplicate frame identity, channels or offset".into(),
                ));
            }
            let mut channels = std::collections::HashSet::new();
            for c in &f.channels {
                if !channels.insert(&c.channel) {
                    return Err(DataError::Invalid("duplicate channel".into()));
                }
                c.validate_with_cancel(cancelled)?;
            }
        }
        for a in &self.analyses {
            if a.schema != 1 || !valid_id(&a.id) || a.record_id != self.id {
                return Err(DataError::Invalid(
                    "analysis provenance does not belong to this record".into(),
                ));
            }
            let f = self
                .frames
                .iter()
                .find(|f| f.id == a.frame_id)
                .ok_or_else(|| DataError::Invalid("analysis frame absent".into()))?;
            let c = f
                .channels
                .iter()
                .find(|c| c.channel == a.channel)
                .ok_or_else(|| DataError::Invalid("analysis channel absent".into()))?;
            a.roi.validate(c.samples.len())?;
            if a.curve.len() > 8192
                || a.markers.len() > 8192
                || a.curve
                    .iter()
                    .chain(&a.markers)
                    .any(|p| !p.x.is_finite() || !p.y.is_finite())
            {
                return Err(DataError::Invalid(
                    "nonfinite or over-budget analysis overlay".into(),
                ));
            }
        }
        Ok(())
    }
}
