use super::{finite, properties::close, Pm400};
use crate::{DriverError, DriverResult};
pub struct Display<'a>(pub(crate) &'a mut Pm400);
impl Pm400 {
    pub fn display(&mut self) -> Display<'_> {
        Display(self)
    }
}
impl Display<'_> {
    fn set(&mut self, h: &str, v: f64) -> DriverResult<f64> {
        if !v.is_finite() {
            return Err(DriverError::Invalid(
                "display setting must be finite".into(),
            ));
        }
        self.0
            .set_confirm(h, &v.to_string(), v, finite, |a, b| close(*a, *b, 1e-12))
    }
    fn get(&mut self, h: &str) -> DriverResult<f64> {
        finite(&self.0.query(&format!("{h}?"), self.0.deadline())?)
    }
    pub fn set_brightness(&mut self, v: f64) -> DriverResult<f64> {
        self.set("DISPlay:BRIGhtness", v)
    }
    pub fn get_brightness(&mut self) -> DriverResult<f64> {
        self.get("DISPlay:BRIGhtness")
    }
    pub fn set_contrast(&mut self, v: f64) -> DriverResult<f64> {
        self.set("DISPlay:CONTrast", v)
    }
    pub fn get_contrast(&mut self) -> DriverResult<f64> {
        self.get("DISPlay:CONTrast")
    }
}
