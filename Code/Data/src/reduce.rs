use crate::{ChannelWaveform, DataError, PlotPoint, SampleRange};
pub(crate) struct Envelope {
    roi: SampleRange,
    budget: usize,
    first: Option<PlotPoint>,
    last: Option<PlotPoint>,
    small: Vec<PlotPoint>,
    buckets: Vec<Option<(PlotPoint, PlotPoint)>>,
}
impl Envelope {
    pub(crate) fn new(roi: SampleRange, budget: usize, len: usize) -> Result<Self, DataError> {
        roi.validate(len)?;
        if !(4..=8192).contains(&budget) {
            return Err(DataError::Limit(
                "viewport budget must be 4..8192 points".into(),
            ));
        }
        let bucket_count = if roi.end - roi.start > budget {
            (budget - 2) / 2
        } else {
            0
        };
        Ok(Self {
            roi,
            budget,
            first: None,
            last: None,
            small: vec![],
            buckets: vec![None; bucket_count],
        })
    }
    pub(crate) fn push(&mut self, index: usize, value: f64) -> Result<(), DataError> {
        if !value.is_finite() {
            return Err(DataError::Invalid(format!(
                "nonfinite calibrated sample at {index}"
            )));
        }
        if index < self.roi.start || index >= self.roi.end {
            return Ok(());
        }
        let point = PlotPoint {
            x: index as f64,
            y: value,
        };
        if self.buckets.is_empty() {
            self.small.push(point);
            return Ok(());
        }
        if index == self.roi.start {
            self.first = Some(point);
        } else if index + 1 == self.roi.end {
            self.last = Some(point);
        } else {
            let inner = self.roi.end - self.roi.start - 2;
            let bucket = ((index - self.roi.start) * self.buckets.len() - 1) / inner;
            let slot = &mut self.buckets[bucket];
            match slot {
                None => *slot = Some((point.clone(), point)),
                Some((lo, hi)) => {
                    if value < lo.y {
                        *lo = point.clone();
                    }
                    if value > hi.y {
                        *hi = point;
                    }
                }
            }
        }
        Ok(())
    }
    pub(crate) fn finish(self) -> Result<Vec<PlotPoint>, DataError> {
        if self.buckets.is_empty() {
            if self.small.len() != self.roi.end - self.roi.start {
                return Err(DataError::Invalid(
                    "viewport stream ended before ROI tail".into(),
                ));
            }
            return Ok(self.small);
        }
        let mut points = vec![self
            .first
            .ok_or_else(|| DataError::Invalid("viewport first sample absent".into()))?];
        for bucket in self.buckets {
            let (lo, hi) =
                bucket.ok_or_else(|| DataError::Invalid("viewport bucket absent".into()))?;
            if lo.x <= hi.x {
                points.push(lo.clone());
                if hi.x != lo.x {
                    points.push(hi);
                }
            } else {
                points.push(hi);
                points.push(lo);
            }
        }
        points.push(
            self.last
                .ok_or_else(|| DataError::Invalid("viewport tail sample absent".into()))?,
        );
        debug_assert!(points.len() <= self.budget);
        Ok(points)
    }
}
/// Display-only extrema envelope. X remains the absolute source sample index.
pub fn reduce_viewport(
    channel: &ChannelWaveform,
    roi: SampleRange,
    max_points: usize,
) -> Result<Vec<PlotPoint>, DataError> {
    channel.validate()?;
    roi.validate(channel.samples.len())?;
    if !(4..=8192).contains(&max_points) {
        return Err(DataError::Limit(
            "viewport budget must be 4..8192 points".into(),
        ));
    }
    let n = roi.end - roi.start;
    let value = |i| {
        channel
            .calibration
            .voltage_v(channel.samples.value(i).unwrap())
    };
    let mut indices = vec![roi.start];
    if n <= max_points {
        indices.extend(roi.start + 1..roi.end);
    } else {
        let buckets = (max_points - 2) / 2;
        for b in 0..buckets {
            let start = roi.start + 1 + (n - 2) * b / buckets;
            let end = roi.start + 1 + (n - 2) * (b + 1) / buckets;
            let mut lo = start;
            let mut hi = start;
            for i in start + 1..end {
                if value(i) < value(lo) {
                    lo = i;
                }
                if value(i) > value(hi) {
                    hi = i;
                }
            }
            if lo <= hi {
                indices.push(lo);
                if hi != lo {
                    indices.push(hi);
                }
            } else {
                indices.push(hi);
                indices.push(lo);
            }
        }
        indices.push(roi.end - 1);
    }
    Ok(indices
        .into_iter()
        .map(|i| PlotPoint {
            x: i as f64,
            y: value(i),
        })
        .collect())
}
