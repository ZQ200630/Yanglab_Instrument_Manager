use super::Pm400;
use crate::DriverResult;
pub struct Calibration<'a>(pub(crate) &'a mut Pm400);
impl Pm400 {
    pub fn calibration(&mut self) -> Calibration<'_> {
        Calibration(self)
    }
}
impl Calibration<'_> {
    pub fn get_string(&mut self) -> DriverResult<String> {
        self.0.query("CALibration:STRing?", self.0.deadline())
    }
}
