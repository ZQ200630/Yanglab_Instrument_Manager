//! Strict one-dimensional non-pickle NumPy array codec.
use crate::{Calibration, DataError, ImportLimits, PlotPoint, SampleRange, Samples};
use std::io::{Read, Write};
pub(crate) fn check(cancelled: &dyn Fn() -> bool) -> Result<(), DataError> {
    if cancelled() {
        Err(DataError::Cancelled)
    } else {
        Ok(())
    }
}
#[derive(Debug)]
struct Header {
    dtype: String,
    count: usize,
    big: bool,
    width: usize,
}
fn parse_header(text: &str) -> Result<(String, usize, bool), DataError> {
    struct Parser<'a> {
        bytes: &'a [u8],
        pos: usize,
    }
    impl Parser<'_> {
        fn ws(&mut self) {
            while self
                .bytes
                .get(self.pos)
                .is_some_and(u8::is_ascii_whitespace)
            {
                self.pos += 1;
            }
        }
        fn take(&mut self, want: u8) -> Result<(), DataError> {
            self.ws();
            if self.bytes.get(self.pos) == Some(&want) {
                self.pos += 1;
                Ok(())
            } else {
                Err(DataError::Invalid("malformed NPY header syntax".into()))
            }
        }
        fn string(&mut self) -> Result<String, DataError> {
            self.ws();
            let quote = *self
                .bytes
                .get(self.pos)
                .ok_or_else(|| DataError::Invalid("missing NPY string".into()))?;
            if quote != b'\'' && quote != b'"' {
                return Err(DataError::Invalid("NPY header field must be quoted".into()));
            }
            self.pos += 1;
            let start = self.pos;
            while let Some(&c) = self.bytes.get(self.pos) {
                if c == quote {
                    let result = std::str::from_utf8(&self.bytes[start..self.pos])
                        .map_err(|_| DataError::Invalid("invalid NPY header string".into()))?
                        .to_string();
                    self.pos += 1;
                    return Ok(result);
                }
                if c == b'\\' || c < 32 {
                    return Err(DataError::Invalid(
                        "escaped/control NPY header strings are unsupported".into(),
                    ));
                }
                self.pos += 1;
            }
            Err(DataError::Invalid("unterminated NPY header string".into()))
        }
        fn count(&mut self) -> Result<usize, DataError> {
            self.ws();
            let start = self.pos;
            while self.bytes.get(self.pos).is_some_and(u8::is_ascii_digit) {
                self.pos += 1;
            }
            std::str::from_utf8(&self.bytes[start..self.pos])
                .unwrap()
                .parse()
                .map_err(|_| DataError::Invalid("invalid NPY shape length".into()))
        }
    }
    let mut p = Parser {
        bytes: text.as_bytes(),
        pos: 0,
    };
    p.take(b'{')?;
    let mut keys = std::collections::HashSet::new();
    let mut dtype = None;
    let mut count = None;
    let mut fortran = None;
    loop {
        p.ws();
        if p.bytes.get(p.pos) == Some(&b'}') {
            p.pos += 1;
            break;
        }
        let key = p.string()?;
        if !keys.insert(key.clone()) {
            return Err(DataError::Invalid("duplicate NPY header field".into()));
        }
        p.take(b':')?;
        match key.as_str() {
            "descr" => dtype = Some(p.string()?),
            "fortran_order" => {
                p.ws();
                if p.bytes[p.pos..].starts_with(b"False") {
                    p.pos += 5;
                    fortran = Some(false);
                } else if p.bytes[p.pos..].starts_with(b"True") {
                    p.pos += 4;
                    fortran = Some(true);
                } else {
                    return Err(DataError::Invalid(
                        "NPY fortran_order must be a boolean literal".into(),
                    ));
                }
            }
            "shape" => {
                p.take(b'(')?;
                count = Some(p.count()?);
                p.take(b',')?;
                p.take(b')')?;
            }
            _ => return Err(DataError::Invalid("unknown NPY header field".into())),
        }
        p.ws();
        match p.bytes.get(p.pos) {
            Some(b',') => p.pos += 1,
            Some(b'}') => {}
            _ => return Err(DataError::Invalid("invalid NPY header separator".into())),
        }
    }
    p.ws();
    if p.pos != p.bytes.len() {
        return Err(DataError::Invalid("trailing NPY header syntax".into()));
    }
    Ok((
        dtype.ok_or_else(|| DataError::Invalid("NPY descr missing".into()))?,
        count.ok_or_else(|| DataError::Invalid("NPY shape missing".into()))?,
        fortran.ok_or_else(|| DataError::Invalid("NPY fortran_order missing".into()))?,
    ))
}
fn header(r: &mut impl Read, limits: &ImportLimits) -> Result<Header, DataError> {
    let mut prefix = [0; 8];
    r.read_exact(&mut prefix)?;
    if &prefix[..6] != b"\x93NUMPY" {
        return Err(DataError::Invalid("invalid NPY magic".into()));
    }
    let length = match prefix[6..] {
        [1, 0] => {
            let mut b = [0; 2];
            r.read_exact(&mut b)?;
            u16::from_le_bytes(b) as usize
        }
        [2, 0] | [3, 0] => {
            let mut b = [0; 4];
            r.read_exact(&mut b)?;
            u32::from_le_bytes(b) as usize
        }
        _ => return Err(DataError::Unsupported("unsupported NPY version".into())),
    };
    if length > 65536 || length as u64 > limits.max_metadata_bytes {
        return Err(DataError::Limit("NPY header exceeds bound".into()));
    }
    let mut bytes = vec![0; length];
    r.read_exact(&mut bytes)?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| DataError::Invalid("NPY header must be UTF-8/ASCII".into()))?;
    let (dtype, count, fortran) = parse_header(text)?;
    if fortran {
        return Err(DataError::Unsupported(
            "Fortran-order arrays are unsupported".into(),
        ));
    }
    if count == 0 || count > limits.max_samples_per_channel {
        return Err(DataError::Limit(
            "NPY sample count is empty or over budget".into(),
        ));
    }
    if dtype.len() < 3 {
        return Err(DataError::Unsupported("invalid NPY dtype".into()));
    }
    let endian = dtype.as_bytes()[0];
    if !b"<>|=".contains(&endian) {
        return Err(DataError::Unsupported("invalid NPY byte order".into()));
    }
    let kind = dtype.as_bytes()[1];
    let elements = dtype[2..]
        .parse::<usize>()
        .map_err(|_| DataError::Unsupported("invalid NPY item size".into()))?;
    let width = if kind == b'U' {
        elements.checked_mul(4)
    } else {
        Some(elements)
    }
    .ok_or_else(|| DataError::Limit("NPY item size overflow".into()))?;
    if width == 0 || width > 65536 {
        return Err(DataError::Limit(
            "NPY item size outside supported bound".into(),
        ));
    }
    if count
        .checked_mul(width)
        .is_none_or(|n| n as u64 > limits.max_member_bytes)
    {
        return Err(DataError::Limit("NPY declared bytes exceed budget".into()));
    }
    Ok(Header {
        dtype,
        count,
        big: endian == b'>' || (endian == b'=' && cfg!(target_endian = "big")),
        width,
    })
}
fn finish(r: &mut impl Read) -> Result<(), DataError> {
    let mut b = [0; 1];
    if r.read(&mut b)? != 0 {
        Err(DataError::Invalid(
            "NPY has trailing payload beyond declared shape".into(),
        ))
    } else {
        Ok(())
    }
}
pub(crate) fn read_numeric(
    r: &mut impl Read,
    limits: &ImportLimits,
    cancelled: &dyn Fn() -> bool,
) -> Result<(Samples, String), DataError> {
    check(cancelled)?;
    let h = header(r, limits)?;
    let kind = h.dtype.as_bytes()[1];
    if !matches!(
        (kind, h.width),
        (b'u', 1) | (b'i', 1) | (b'u', 2) | (b'i', 2) | (b'f', 4) | (b'f', 8)
    ) {
        return Err(DataError::Unsupported(format!(
            "unsupported NPY dtype {}; object/pickle arrays are never read",
            h.dtype
        )));
    }
    let output_width = if kind == b'f' { 8 } else { h.width };
    if h.count.checked_mul(output_width).is_none_or(|bytes| {
        bytes as u64 > limits.max_member_bytes || bytes as u64 > limits.max_uncompressed_bytes
    }) {
        return Err(DataError::Limit(
            "decoded NPY allocation exceeds configured byte budget".into(),
        ));
    }
    let mut samples = match (kind, h.width) {
        (b'u', 1) => Samples::U8(Vec::with_capacity(h.count)),
        (b'i', 1) => Samples::I8(Vec::with_capacity(h.count)),
        (b'u', 2) => Samples::U16(Vec::with_capacity(h.count)),
        (b'i', 2) => Samples::I16(Vec::with_capacity(h.count)),
        _ => Samples::F64(Vec::with_capacity(h.count)),
    };
    let mut buffer = [0u8; 16384];
    let mut done = 0;
    while done < h.count {
        check(cancelled)?;
        let count = (h.count - done).min(buffer.len() / h.width);
        let bytes = &mut buffer[..count * h.width];
        r.read_exact(bytes)?;
        for chunk in bytes.chunks_exact(h.width) {
            match &mut samples {
                Samples::U8(v) => v.push(chunk[0]),
                Samples::I8(v) => v.push(chunk[0] as i8),
                Samples::U16(v) => v.push(if h.big {
                    u16::from_be_bytes(chunk.try_into().unwrap())
                } else {
                    u16::from_le_bytes(chunk.try_into().unwrap())
                }),
                Samples::I16(v) => v.push(if h.big {
                    i16::from_be_bytes(chunk.try_into().unwrap())
                } else {
                    i16::from_le_bytes(chunk.try_into().unwrap())
                }),
                Samples::F64(v) => {
                    let value = if h.width == 4 {
                        if h.big {
                            f32::from_be_bytes(chunk.try_into().unwrap()) as f64
                        } else {
                            f32::from_le_bytes(chunk.try_into().unwrap()) as f64
                        }
                    } else if h.big {
                        f64::from_be_bytes(chunk.try_into().unwrap())
                    } else {
                        f64::from_le_bytes(chunk.try_into().unwrap())
                    };
                    if !value.is_finite() {
                        return Err(DataError::Invalid("nonfinite NPY sample".into()));
                    }
                    v.push(value);
                }
            }
        }
        done += count;
    }
    finish(r)?;
    Ok((samples, h.dtype))
}
pub(crate) fn read_strings(
    r: &mut impl Read,
    limits: &ImportLimits,
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<String>, DataError> {
    check(cancelled)?;
    let h = header(r, limits)?;
    if h.count > 64 || !matches!(h.dtype.as_bytes()[1], b'U' | b'S') {
        return Err(DataError::Unsupported(
            "channel_names must be at most 64 Unicode or byte strings".into(),
        ));
    }
    let mut result = vec![];
    let mut item = vec![0; h.width];
    for _ in 0..h.count {
        check(cancelled)?;
        r.read_exact(&mut item)?;
        let s = if h.dtype.as_bytes()[1] == b'S' {
            let end = item.iter().position(|&b| b == 0).unwrap_or(item.len());
            std::str::from_utf8(&item[..end])
                .map_err(|_| DataError::Invalid("invalid UTF-8 channel name".into()))?
                .to_string()
        } else {
            let mut s = String::new();
            for part in item.chunks_exact(4) {
                let cp = if h.big {
                    u32::from_be_bytes(part.try_into().unwrap())
                } else {
                    u32::from_le_bytes(part.try_into().unwrap())
                };
                if cp == 0 {
                    break;
                }
                s.push(
                    char::from_u32(cp)
                        .ok_or_else(|| DataError::Invalid("invalid Unicode channel name".into()))?,
                );
            }
            s
        };
        if s.trim().is_empty() || s.len() > 128 {
            return Err(DataError::Invalid("invalid channel name".into()));
        }
        result.push(s);
    }
    finish(r)?;
    Ok(result)
}
fn write_header(w: &mut impl Write, dtype: &str, count: usize) -> Result<(), DataError> {
    let base = format!("{{'descr': '{dtype}', 'fortran_order': False, 'shape': ({count},), }}");
    let padding = (64 - (10 + base.len() + 1) % 64) % 64;
    let h = format!("{base}{}\n", " ".repeat(padding));
    let len =
        u16::try_from(h.len()).map_err(|_| DataError::Limit("NPY header too large".into()))?;
    w.write_all(b"\x93NUMPY\x01\x00")?;
    w.write_all(&len.to_le_bytes())?;
    w.write_all(h.as_bytes())?;
    Ok(())
}
pub(crate) fn write_numeric(
    w: &mut impl Write,
    samples: &Samples,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), DataError> {
    let dtype = match samples {
        Samples::U8(_) => "|u1",
        Samples::I8(_) => "|i1",
        Samples::U16(_) => "<u2",
        Samples::I16(_) => "<i2",
        Samples::F64(_) => "<f8",
    };
    write_header(w, dtype, samples.len())?;
    let mut buffer = Vec::with_capacity(16384);
    for i in 0..samples.len() {
        if i % 2048 == 0 {
            check(cancelled)?;
        }
        match samples {
            Samples::U8(v) => buffer.push(v[i]),
            Samples::I8(v) => buffer.push(v[i] as u8),
            Samples::U16(v) => buffer.extend(v[i].to_le_bytes()),
            Samples::I16(v) => buffer.extend(v[i].to_le_bytes()),
            Samples::F64(v) => buffer.extend(v[i].to_le_bytes()),
        }
        if buffer.len() >= 16384 {
            w.write_all(&buffer)?;
            buffer.clear();
        }
    }
    w.write_all(&buffer)?;
    Ok(())
}
pub(crate) fn write_values(
    w: &mut impl Write,
    count: usize,
    values: impl Iterator<Item = f64>,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), DataError> {
    write_header(w, "<f8", count)?;
    let mut buffer = Vec::with_capacity(16384);
    let mut seen = 0;
    for value in values {
        if seen % 2048 == 0 {
            check(cancelled)?;
        }
        if !value.is_finite() {
            return Err(DataError::Invalid("nonfinite export value".into()));
        }
        buffer.extend(value.to_le_bytes());
        seen += 1;
        if buffer.len() >= 16384 {
            w.write_all(&buffer)?;
            buffer.clear();
        }
    }
    if seen != count {
        return Err(DataError::Invalid("export length changed".into()));
    }
    w.write_all(&buffer)?;
    Ok(())
}
pub(crate) fn write_strings(w: &mut impl Write, strings: &[String]) -> Result<(), DataError> {
    let width = strings
        .iter()
        .map(|s| s.chars().count())
        .max()
        .unwrap_or(1)
        .max(1);
    write_header(w, &format!("<U{width}"), strings.len())?;
    for s in strings {
        let chars: Vec<_> = s.chars().collect();
        for i in 0..width {
            w.write_all(&(chars.get(i).copied().unwrap_or('\0') as u32).to_le_bytes())?;
        }
    }
    Ok(())
}
fn hex(hash: ring::digest::Digest) -> String {
    hash.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}
struct HashWriter<'a, W: Write> {
    inner: &'a mut W,
    context: ring::digest::Context,
    count: u64,
}
impl<W: Write> Write for HashWriter<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(bytes)?;
        self.context.update(&bytes[..n]);
        self.count += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}
pub(crate) fn write_evidence(
    w: &mut impl Write,
    samples: &Samples,
    cancelled: &dyn Fn() -> bool,
) -> Result<(String, u64), DataError> {
    let mut writer = HashWriter {
        inner: w,
        context: ring::digest::Context::new(&ring::digest::SHA256),
        count: 0,
    };
    write_numeric(&mut writer, samples, cancelled)?;
    Ok((hex(writer.context.finish()), writer.count))
}
struct HashReader<'a, R: Read> {
    inner: &'a mut R,
    context: ring::digest::Context,
    count: u64,
}
impl<R: Read> Read for HashReader<'_, R> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(bytes)?;
        self.context.update(&bytes[..n]);
        self.count += n as u64;
        Ok(n)
    }
}
pub(crate) fn read_evidence(
    r: &mut impl Read,
    limits: &ImportLimits,
    cancelled: &dyn Fn() -> bool,
) -> Result<(Samples, String, String, u64), DataError> {
    let mut reader = HashReader {
        inner: r,
        context: ring::digest::Context::new(&ring::digest::SHA256),
        count: 0,
    };
    let (samples, dtype) = read_numeric(&mut reader, limits, cancelled)?;
    Ok((samples, dtype, hex(reader.context.finish()), reader.count))
}
/// Streams one selected raw payload with bounded working memory and checks its complete hash.
pub(crate) fn read_viewport(
    r: &mut impl Read,
    limits: &ImportLimits,
    calibration: &Calibration,
    roi: SampleRange,
    max_points: usize,
    cancelled: &dyn Fn() -> bool,
) -> Result<(Vec<PlotPoint>, String, String, u64, usize), DataError> {
    check(cancelled)?;
    calibration.validate()?;
    let mut reader = HashReader {
        inner: r,
        context: ring::digest::Context::new(&ring::digest::SHA256),
        count: 0,
    };
    let h = header(&mut reader, limits)?;
    let kind = h.dtype.as_bytes()[1];
    if !matches!(
        (kind, h.width),
        (b'u', 1) | (b'i', 1) | (b'u', 2) | (b'i', 2) | (b'f', 4) | (b'f', 8)
    ) {
        return Err(DataError::Unsupported(
            "unsupported numeric viewport dtype".into(),
        ));
    }
    if !calibration.time_s(h.count - 1).is_finite() {
        return Err(DataError::Invalid(
            "viewport time calibration overflows".into(),
        ));
    }
    let mut envelope = crate::reduce::Envelope::new(roi, max_points, h.count)?;
    let mut buffer = [0u8; 16384];
    let mut done = 0;
    while done < h.count {
        check(cancelled)?;
        let count = (h.count - done).min(buffer.len() / h.width);
        let bytes = &mut buffer[..count * h.width];
        reader.read_exact(bytes)?;
        for (offset, chunk) in bytes.chunks_exact(h.width).enumerate() {
            let value = match (kind, h.width) {
                (b'u', 1) => chunk[0] as f64,
                (b'i', 1) => (chunk[0] as i8) as f64,
                (b'u', 2) => {
                    if h.big {
                        u16::from_be_bytes(chunk.try_into().unwrap()) as f64
                    } else {
                        u16::from_le_bytes(chunk.try_into().unwrap()) as f64
                    }
                }
                (b'i', 2) => {
                    if h.big {
                        i16::from_be_bytes(chunk.try_into().unwrap()) as f64
                    } else {
                        i16::from_le_bytes(chunk.try_into().unwrap()) as f64
                    }
                }
                (b'f', 4) => {
                    if h.big {
                        f32::from_be_bytes(chunk.try_into().unwrap()) as f64
                    } else {
                        f32::from_le_bytes(chunk.try_into().unwrap()) as f64
                    }
                }
                (b'f', 8) => {
                    if h.big {
                        f64::from_be_bytes(chunk.try_into().unwrap())
                    } else {
                        f64::from_le_bytes(chunk.try_into().unwrap())
                    }
                }
                _ => unreachable!(),
            };
            envelope.push(done + offset, calibration.voltage_v(value))?;
        }
        done += count;
    }
    finish(&mut reader)?;
    check(cancelled)?;
    Ok((
        envelope.finish()?,
        h.dtype,
        hex(reader.context.finish()),
        reader.count,
        h.count,
    ))
}
