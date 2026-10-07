use crate::{
    transport::{serial::SerialSession, ByteTransport, Deadline},
    DriverError, DriverResult,
};
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedReply {
    pub lines: Vec<String>,
    pub echo_enabled: bool,
}
/// Pure bounded parser: provides no transport/raw-command escape hatch.
pub fn parse_reply(command: &str, bytes: &[u8], echo: Option<bool>) -> DriverResult<ParsedReply> {
    if bytes.len() > 4096 || !bytes.is_ascii() {
        return Err(DriverError::Protocol(
            "MDT non-ASCII/oversized reply".into(),
        ));
    }
    let mut lines = vec![];
    let mut line = vec![];
    let mut previous_cr = false;
    let mut prompt = None;
    for (index, &b) in bytes.iter().enumerate() {
        if previous_cr && b == b'\n' {
            previous_cr = false;
            continue;
        }
        previous_cr = false;
        if b == b'\r' || b == b'\n' {
            let text = String::from_utf8(std::mem::take(&mut line)).unwrap();
            if text == "*" || text == "!" {
                prompt = Some((text, index, b));
                break;
            }
            lines.push(text);
            previous_cr = b == b'\r';
        } else {
            line.push(b);
        }
    }
    let (p, index, b) =
        prompt.ok_or_else(|| DriverError::Protocol("MDT missing terminated prompt".into()))?;
    let tail = &bytes[index + 1..];
    if !tail.is_empty() && !(b == b'\r' && tail == b"\n") {
        return Err(DriverError::Protocol("MDT data after prompt".into()));
    }
    if p == "!" {
        return Err(DriverError::Protocol("MDT command rejected".into()));
    }
    let found = lines.first().is_some_and(|s| s == command);
    match echo {
        Some(true) if !found => {
            return Err(DriverError::Protocol("MDT expected echo absent".into()))
        }
        Some(false) if found && command != "?" => {
            return Err(DriverError::Protocol("MDT unexpected echo".into()))
        }
        _ => {}
    }
    let enabled = echo.unwrap_or(found);
    if enabled && found {
        lines.remove(0);
    }
    Ok(ParsedReply {
        lines,
        echo_enabled: enabled,
    })
}
pub(crate) struct Protocol {
    pub session: SerialSession,
    pub echo: Option<bool>,
    pending_lf: bool,
    last_size: usize,
    pub broken: bool,
}
impl Protocol {
    pub fn new(session: SerialSession) -> Self {
        Self {
            session,
            echo: None,
            pending_lf: false,
            last_size: 0,
            broken: false,
        }
    }
    fn boundary(&mut self, d: Deadline) -> DriverResult<()> {
        if self.session.available()? != 0 {
            let byte = self.session.read_bounded(1, d)?;
            if self.pending_lf && byte == b"\n" {
                self.last_size += 1;
                if self.last_size > 4096 {
                    return Err(DriverError::Protocol(
                        "MDT optional LF exceeded bound".into(),
                    ));
                }
                self.pending_lf = false;
                self.last_size = 0;
            } else {
                return Err(DriverError::Protocol(
                    "MDT trailing/stale response bytes".into(),
                ));
            }
            if self.session.available()? != 0 {
                return Err(DriverError::Protocol("MDT data after prompt".into()));
            }
        }
        Ok(())
    }
    pub fn exchange(&mut self, command: &str, d: Deadline) -> DriverResult<Vec<String>> {
        if command.is_empty()
            || command.len() > 128
            || !command.is_ascii()
            || command.contains(['\n', '\r'])
        {
            return Err(DriverError::Invalid("invalid internal MDT frame".into()));
        }
        let r = (|| {
            self.boundary(d)?;
            self.session
                .write_all(format!("{command}\r\n").as_bytes(), d)?;
            let mut bytes = vec![];
            let mut line = vec![];
            let mut previous_cr = false;
            loop {
                let b = self.session.read_bounded(1, d)?;
                d.remaining_millis()?;
                if b.is_empty() {
                    return Err(DriverError::Timeout {
                        operation: "MDT prompt".into(),
                        transferred: bytes.len(),
                    });
                }
                let b = b[0];
                if self.pending_lf {
                    self.pending_lf = false;
                    if b == b'\n' {
                        self.last_size += 1;
                        if self.last_size > 4096 {
                            return Err(DriverError::Protocol(
                                "MDT deferred LF exceeded previous reply bound".into(),
                            ));
                        }
                        self.last_size = 0;
                        continue;
                    }
                    self.last_size = 0;
                }
                bytes.push(b);
                if bytes.len() > 4096 || b > 127 {
                    return Err(DriverError::Protocol(
                        "MDT reply exceeded bound or ASCII".into(),
                    ));
                }
                if previous_cr && b == b'\n' {
                    previous_cr = false;
                    continue;
                }
                previous_cr = false;
                if b == b'\r' || b == b'\n' {
                    if line == b"*" || line == b"!" {
                        self.pending_lf = b == b'\r';
                        self.last_size = bytes.len();
                        self.boundary(d)?;
                        let parsed = parse_reply(command, &bytes, self.echo)?;
                        self.echo = Some(parsed.echo_enabled);
                        return Ok(parsed.lines);
                    }
                    line.clear();
                    previous_cr = b == b'\r';
                } else {
                    line.push(b);
                }
            }
        })();
        if let Err(e) = &r {
            if !matches!(e,DriverError::Protocol(s)if s=="MDT command rejected") {
                self.broken = true;
                let _ = self.session.fence_protocol();
            }
        }
        r
    }
}
pub(crate) fn single(lines: &[String]) -> DriverResult<String> {
    if lines.len() != 1 {
        return Err(DriverError::Protocol("MDT expected one result line".into()));
    }
    Ok(lines[0].clone())
}
pub(crate) fn number(lines: &[String]) -> DriverResult<f64> {
    let text = single(lines)?;
    let text = text.trim();
    for (index, c) in text.char_indices() {
        if c.is_ascii_digit() || c == '+' || c == '-' || c == '.' {
            if let Ok(v) = text[index..].trim().parse::<f64>() {
                if v.is_finite() {
                    return Ok(v);
                }
                return Err(DriverError::Protocol("MDT nonfinite number".into()));
            }
        }
    }
    Err(DriverError::Protocol("MDT expected numeric suffix".into()))
}
pub(crate) fn integer(lines: &[String], min: i64, max: i64) -> DriverResult<i64> {
    let v = number(lines)?;
    if v.fract() != 0. || v < (min as f64) || v > (max as f64) {
        return Err(DriverError::Protocol("MDT integer outside bounds".into()));
    }
    Ok(v as i64)
}
pub(crate) fn boolean(lines: &[String]) -> DriverResult<bool> {
    let text = single(lines)?;
    let text = text
        .rsplit([':', '='])
        .next()
        .unwrap()
        .trim()
        .to_ascii_lowercase();
    match text.as_str() {
        "1" | "on" | "true" | "enabled" => Ok(true),
        "0" | "off" | "false" | "disabled" => Ok(false),
        _ => Err(DriverError::Protocol("MDT expected boolean".into())),
    }
}
