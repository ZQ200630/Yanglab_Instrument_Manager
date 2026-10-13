use super::contracts::HostError;
use serde_json::Value;
use yang_data::{Calibration, ChannelWaveform, Samples, WaveformFrame};
fn error(e: impl std::fmt::Display) -> HostError {
    HostError::new("WaveformTransfer", e.to_string())
}
/// Decode one bounded frame from independently hashed raw chunks. No sample JSON.
/// Caller retains Worker staging until a verified RAM preview or durable frame
/// commit has succeeded; transferring a manual preview does not save a Record.
pub fn receive_waveform(
    descriptor: &Value,
    mut read: impl FnMut(u64, u64) -> Result<Vec<u8>, HostError>,
    cancelled: &dyn Fn() -> bool,
    mut progress: impl FnMut(u64, u64),
) -> Result<WaveformFrame, HostError> {
    if cancelled() {
        return Err(HostError::new(
            "Cancelled",
            "Waveform transfer cancelled before allocation",
        ));
    }
    if descriptor["kind"] != "waveform"
        || descriptor["schema"] != 1
        || !descriptor["capture_id"]
            .as_str()
            .is_some_and(yang_data::valid_id)
    {
        return Err(error("Invalid waveform descriptor"));
    }
    let frame = &descriptor["metadata"]["frame"];
    let id = frame["id"]
        .as_str()
        .filter(|id| yang_data::valid_id(id))
        .ok_or_else(|| error("Invalid frame identity"))?;
    let total = descriptor["byte_count"]
        .as_u64()
        .filter(|n| *n > 0 && *n <= 536_870_912)
        .ok_or_else(|| error("Waveform byte budget exceeded"))?;
    let entries = frame["channels"]
        .as_array()
        .filter(|v| !v.is_empty() && v.len() <= 4)
        .ok_or_else(|| error("Invalid waveform channels"))?;
    let mut count = 0u64;
    let mut offset = 0u64;
    let mut names = std::collections::BTreeSet::new();
    for c in entries {
        let n = c["point_count"]
            .as_u64()
            .filter(|n| *n > 0 && *n <= 250_000_000)
            .ok_or_else(|| error("Invalid channel point count"))?;
        let width = match c["dtype"].as_str() {
            Some("u8" | "i8") => 1,
            Some("u16_le" | "i16_le") => 2,
            Some("f64_le") => 8,
            _ => return Err(error("Unsupported raw dtype")),
        };
        let bytes = n
            .checked_mul(width)
            .ok_or_else(|| error("Channel byte overflow"))?;
        if c["byte_count"] != bytes
            || c["byte_offset"] != offset
            || !c["channel"].as_str().is_some_and(|s| {
                matches!(s, "CH1" | "CH2" | "CH3" | "CH4") && names.insert(s.to_owned())
            })
        {
            return Err(error(
                "Channel layout is overlapping, sparse or inconsistent",
            ));
        }
        let calibration: Calibration =
            serde_json::from_value(c["calibration"].clone()).map_err(error)?;
        calibration.validate().map_err(error)?;
        offset = offset
            .checked_add(bytes)
            .ok_or_else(|| error("Frame size overflow"))?;
        count = count
            .checked_add(n)
            .ok_or_else(|| error("Frame count overflow"))?;
    }
    if offset != total || descriptor["point_count"] != count {
        return Err(error("Descriptor total does not match channel layout"));
    }
    let mut digest = ring::digest::Context::new(&ring::digest::SHA256);
    let mut channels = vec![];
    let mut received = 0u64;
    for c in entries {
        let n = c["point_count"].as_u64().unwrap() as usize;
        let dtype = c["dtype"].as_str().unwrap();
        let samples = match dtype {
            "u8" => Samples::U8(Vec::new()),
            "i8" => Samples::I8(Vec::new()),
            "u16_le" => Samples::U16(Vec::new()),
            "i16_le" => Samples::I16(Vec::new()),
            _ => Samples::F64(Vec::new()),
        };
        let mut samples = samples;
        match &mut samples {
            Samples::U8(v) => v.try_reserve_exact(n).map_err(error)?,
            Samples::I8(v) => v.try_reserve_exact(n).map_err(error)?,
            Samples::U16(v) => v.try_reserve_exact(n).map_err(error)?,
            Samples::I16(v) => v.try_reserve_exact(n).map_err(error)?,
            Samples::F64(v) => v.try_reserve_exact(n).map_err(error)?,
        };
        let end = received + c["byte_count"].as_u64().unwrap();
        while received < end {
            if cancelled() {
                return Err(HostError::new(
                    "Cancelled",
                    "Waveform transfer cancelled; staging remains retained",
                ));
            }
            let length = (end - received).min(16384);
            let bytes = read(received, length)?;
            if bytes.len() != length as usize {
                return Err(error("Truncated waveform chunk"));
            }
            digest.update(&bytes);
            match &mut samples {
                Samples::U8(v) => v.extend_from_slice(&bytes),
                Samples::I8(v) => v.extend(bytes.iter().map(|&b| b as i8)),
                Samples::U16(v) => v.extend(
                    bytes
                        .chunks_exact(2)
                        .map(|b| u16::from_le_bytes([b[0], b[1]])),
                ),
                Samples::I16(v) => v.extend(
                    bytes
                        .chunks_exact(2)
                        .map(|b| i16::from_le_bytes([b[0], b[1]])),
                ),
                Samples::F64(v) => v.extend(
                    bytes
                        .chunks_exact(8)
                        .map(|b| f64::from_le_bytes(b.try_into().unwrap())),
                ),
            }
            received += length;
            progress(received, total);
        }
        let wave = ChannelWaveform {
            channel: c["channel"].as_str().unwrap().into(),
            samples,
            calibration: serde_json::from_value(c["calibration"].clone()).map_err(error)?,
            metadata: c["metadata"].clone(),
        };
        wave.validate_with_cancel(cancelled).map_err(|e| {
            HostError::new(
                if e == yang_data::DataError::Cancelled {
                    "Cancelled"
                } else {
                    "WaveformTransfer"
                },
                e.to_string(),
            )
        })?;
        channels.push(wave);
    }
    let hash = digest
        .finish()
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    if descriptor["sha256"] != hash {
        return Err(error("Waveform SHA256 mismatch; samples not committed"));
    }
    if cancelled() {
        return Err(HostError::new(
            "Cancelled",
            "Waveform transfer cancelled before commit",
        ));
    }
    Ok(WaveformFrame {
        id: id.into(),
        captured_at_unix_ms: frame["captured_at_unix_ms"]
            .as_u64()
            .ok_or_else(|| error("Frame timestamp missing"))?,
        nominal_offset_s: if frame["nominal_offset_s"].is_null() {
            None
        } else {
            Some(
                frame["nominal_offset_s"]
                    .as_f64()
                    .filter(|v| v.is_finite())
                    .ok_or_else(|| error("Invalid nominal offset"))?,
            )
        },
        channels,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use yang_data::{Calibration, Samples};
    fn descriptor(bytes: &[u8]) -> serde_json::Value {
        json!({"schema":1,"kind":"waveform","capture_id":"a".repeat(32),"point_count":3,"byte_count":6,"sha256":super::super::verification::sha256_bytes(bytes).unwrap(),"metadata":{"schema":1,"frame":{"id":"b".repeat(32),"captured_at_unix_ms":100,"nominal_offset_s":null,"channels":[{"channel":"CH1","dtype":"u16_le","point_count":3,"byte_offset":0,"byte_count":6,"calibration":Calibration{x_increment_s:1e-6,x_origin_s:0.,x_reference:0.,y_increment_v:0.01,y_origin:0.,y_reference:0.},"metadata":{}}]},"source":{"model":"MHO5054","identity":"RIGOL,MHO5054,SN1,FW"}}})
    }
    #[test]
    fn literal_unsigned_word_bytes_are_preserved_without_json_sample_arrays() {
        let bytes = [0u8, 0, 255, 127, 255, 255];
        let d = descriptor(&bytes);
        let f = receive_waveform(
            &d,
            |offset, len| Ok(bytes[offset as usize..(offset + len) as usize].to_vec()),
            &|| false,
            |_, _| {},
        )
        .unwrap();
        assert_eq!(f.channels[0].samples, Samples::U16(vec![0, 32767, 65535]));
        assert_eq!(f.captured_at_unix_ms, 100);
    }
    #[test]
    fn ten_million_points_per_channel_transfer_through_both_wire_guards() {
        let bytes: Vec<u8> = (0..20_000_000usize).map(|n| (n % 251) as u8).collect();
        let calibration = Calibration { x_increment_s: 2e-8, x_origin_s: -0.1,
            x_reference: 0., y_increment_v: 0.01, y_origin: 0., y_reference: 128. };
        let d = json!({"schema":1,"kind":"waveform","capture_id":"a".repeat(32),
            "point_count":20_000_000,"byte_count":20_000_000,
            "sha256":super::super::verification::sha256_bytes(&bytes).unwrap(),
            "metadata":{"frame":{"id":"b".repeat(32),"captured_at_unix_ms":100,
                "nominal_offset_s":null,"channels":[
                    {"channel":"CH1","dtype":"u8","point_count":10_000_000,"byte_offset":0,
                        "byte_count":10_000_000,"calibration":calibration,"metadata":{}},
                    {"channel":"CH4","dtype":"u8","point_count":10_000_000,"byte_offset":10_000_000,
                        "byte_count":10_000_000,"calibration":calibration,"metadata":{}}]}}});
        let mut completed = 0;
        let f = receive_waveform(&d, |offset, length| {
            let request = serde_json::to_vec(&json!({"v":3,"id":"large-chunk",
                "method":"read_capture_chunk","params":{"ownership_nonce":"c".repeat(32),
                    "capture_id":"a".repeat(32),"offset":offset,"length":length},
                "context":{"session_id":"d".repeat(32),"domain":null,
                    "connection_id":null,"epoch":0}})).unwrap();
            crate::runtime::decode_v3(&request).map_err(error)?;
            yang_protocol::parse_request(&request).map_err(error)?;
            Ok(bytes[offset as usize..(offset + length) as usize].to_vec())
        }, &|| false, |done, total| {
            assert_eq!(total, 20_000_000);
            assert!(done > completed);
            completed = done;
        }).unwrap();
        assert_eq!(completed, 20_000_000);
        assert_eq!(f.channels.len(), 2);
        assert_eq!(f.channels[0].samples, Samples::U8(bytes[..10_000_000].to_vec()));
        assert_eq!(f.channels[1].samples, Samples::U8(bytes[10_000_000..].to_vec()));
    }
    #[test]
    fn corruption_cancellation_and_overlapping_channel_layout_fail() {
        let bytes = [0u8, 0, 1, 0, 2, 0];
        let d = descriptor(&bytes);
        assert!(receive_waveform(&d, |_, _| Ok(vec![9; 6]), &|| false, |_, _| {}).is_err());
        assert!(receive_waveform(
            &d,
            |_, _| panic!("read must not start"),
            &|| true,
            |_, _| {}
        )
        .is_err());
        let mut bad = d;
        bad["metadata"]["frame"]["channels"][0]["byte_offset"] = json!(1);
        assert!(receive_waveform(
            &bad,
            |_, _| panic!("invalidlayoutmustnotread"),
            &|| false,
            |_, _| {}
        )
        .is_err());
    }
}
