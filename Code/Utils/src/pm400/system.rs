use super::{csv, properties::boolean, Pm400, SensorInfo, SystemError};
use crate::{DriverError, DriverResult};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Date {
    year: u16,
    month: u8,
    day: u8,
}
impl Date {
    pub fn new(year: u16, month: u8, day: u8) -> DriverResult<Self> {
        let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
        let days = match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 => {
                if leap {
                    29
                } else {
                    28
                }
            }
            _ => 0,
        };
        if year == 0 || year > 9999 || day == 0 || day > days {
            return Err(DriverError::Invalid("invalid Gregorian date".into()));
        }
        Ok(Self { year, month, day })
    }
    pub fn year(self) -> u16 {
        self.year
    }
    pub fn month(self) -> u8 {
        self.month
    }
    pub fn day(self) -> u8 {
        self.day
    }
    fn token(self) -> String {
        format!("{},{},{}", self.year, self.month, self.day)
    }
    fn parse(t: &str) -> DriverResult<Self> {
        let f = csv(t, 3)?;
        if f.iter().any(|s| !s.bytes().all(|b| b.is_ascii_digit())) {
            return Err(DriverError::Protocol("date requires integer fields".into()));
        }
        let y = f[0]
            .parse()
            .map_err(|_| DriverError::Protocol("date year".into()))?;
        let m = f[1]
            .parse()
            .map_err(|_| DriverError::Protocol("date month".into()))?;
        let d = f[2]
            .parse()
            .map_err(|_| DriverError::Protocol("date day".into()))?;
        Self::new(y, m, d).map_err(|_| DriverError::Protocol("invalid device date".into()))
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Time {
    hour: u8,
    minute: u8,
    second: u8,
    microsecond: u32,
}
impl Time {
    pub fn new(hour: u8, minute: u8, second: u8, microsecond: u32) -> DriverResult<Self> {
        if hour > 23 || minute > 59 || second > 59 || microsecond > 999999 {
            return Err(DriverError::Invalid("invalid timezone-free time".into()));
        }
        Ok(Self {
            hour,
            minute,
            second,
            microsecond,
        })
    }
    pub fn hour(self) -> u8 {
        self.hour
    }
    pub fn minute(self) -> u8 {
        self.minute
    }
    pub fn second(self) -> u8 {
        self.second
    }
    pub fn microsecond(self) -> u32 {
        self.microsecond
    }
    fn token(self) -> String {
        let s = if self.microsecond == 0 {
            self.second.to_string()
        } else {
            format!("{}.{:06}", self.second, self.microsecond)
                .trim_end_matches('0')
                .into()
        };
        format!("{},{},{s}", self.hour, self.minute)
    }
    fn parse(t: &str) -> DriverResult<Self> {
        let f = csv(t, 3)?;
        let (secs, fraction) = f[2]
            .split_once('.')
            .map_or((f[2].as_str(), None), |(s, f)| (s, Some(f)));
        if [&f[0], &f[1], secs]
            .iter()
            .any(|s| s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()))
            || fraction.is_some_and(|f| {
                f.is_empty() || f.len() > 6 || !f.bytes().all(|b| b.is_ascii_digit())
            })
        {
            return Err(DriverError::Protocol("invalid time fields".into()));
        }
        let parse = |s: &str| {
            s.parse::<u8>()
                .map_err(|_| DriverError::Protocol("time integer".into()))
        };
        let micros = if let Some(f) = fraction {
            f.parse::<u32>().unwrap() * 10u32.pow(6 - f.len() as u32)
        } else {
            0
        };
        Self::new(parse(&f[0])?, parse(&f[1])?, parse(secs)?, micros)
            .map_err(|_| DriverError::Protocol("invalid device time".into()))
    }
}
pub struct System<'a>(pub(crate) &'a mut Pm400);
impl Pm400 {
    pub fn system(&mut self) -> System<'_> {
        System(self)
    }
}
impl System<'_> {
    pub fn beep(&mut self) -> DriverResult<()> {
        self.0.action("SYSTem:BEEPer", self.0.deadline())
    }
    pub fn set_beeper_enabled(&mut self, v: bool) -> DriverResult<bool> {
        self.0.set_boolean("SYSTem:BEEPer:STATe", v)
    }
    pub fn get_beeper_enabled(&mut self) -> DriverResult<bool> {
        boolean(&self.0.query("SYSTem:BEEPer:STATe?", self.0.deadline())?)
    }
    /// Consumes one queued system error.
    pub fn next_error(&mut self) -> DriverResult<SystemError> {
        SystemError::parse(&self.0.query("SYSTem:ERRor?", self.0.deadline())?)
    }
    /// Consumes a bounded error queue; never part of normal connection.
    pub fn drain_errors(&mut self, max: usize) -> DriverResult<Vec<SystemError>> {
        if max == 0 || max > 128 {
            return Err(DriverError::Invalid("maximum errors must be 1..128".into()));
        }
        let deadline = self.0.deadline();
        let mut result = vec![];
        for _ in 0..max {
            let e = SystemError::parse(&self.0.query("SYSTem:ERRor?", deadline)?)?;
            if e.code == 0 {
                return Ok(result);
            }
            result.push(e);
        }
        Err(DriverError::Protocol(
            "system error queue did not terminate".into(),
        ))
    }
    pub fn get_scpi_version(&mut self) -> DriverResult<String> {
        self.0.query("SYSTem:VERSion?", self.0.deadline())
    }
    pub fn set_date(&mut self, v: Date) -> DriverResult<Date> {
        self.0
            .set_confirm("SYSTem:DATE", &v.token(), v, Date::parse, |a, b| a == b)
    }
    pub fn get_date(&mut self) -> DriverResult<Date> {
        Date::parse(&self.0.query("SYSTem:DATE?", self.0.deadline())?)
    }
    pub fn set_time(&mut self, v: Time) -> DriverResult<Time> {
        self.0
            .set_confirm("SYSTem:TIME", &v.token(), v, Time::parse, |a, b| a == b)
    }
    pub fn get_time(&mut self) -> DriverResult<Time> {
        Time::parse(&self.0.query("SYSTem:TIME?", self.0.deadline())?)
    }
    pub fn set_line_frequency_hz(&mut self, v: u32) -> DriverResult<u32> {
        if v != 50 && v != 60 {
            return Err(DriverError::Invalid(
                "line frequency must be 50 or 60".into(),
            ));
        }
        self.0.set_integer("SYSTem:LFRequency", v, 50, 60)
    }
    pub fn get_line_frequency_hz(&mut self) -> DriverResult<u32> {
        let v = self.0.get_integer("SYSTem:LFRequency", 50, 60)?;
        if v != 50 && v != 60 {
            return Err(DriverError::Protocol(
                "device line frequency must be 50 or 60".into(),
            ));
        }
        Ok(v)
    }
    pub fn get_sensor_info(&mut self) -> DriverResult<SensorInfo> {
        SensorInfo::parse(&self.0.query("SYSTem:SENSor:IDN?", self.0.deadline())?)
    }
}
