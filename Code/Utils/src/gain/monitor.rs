use std::time::Duration;
#[derive(Default)]
pub(super) struct Deviation {
    last: Option<Duration>,
    count: usize,
}
impl Deviation {
    pub fn observe(&mut self, deviation: f64, at: Duration) -> bool {
        if deviation <= 1. {
            self.last = None;
            self.count = 0;
            return false;
        }
        if self
            .last
            .is_none_or(|last| at < last || at.saturating_sub(last) > Duration::from_millis(1500))
        {
            self.count = 1;
            self.last = Some(at);
        } else if at.saturating_sub(self.last.unwrap()) >= Duration::from_secs(1) {
            self.count += 1;
            self.last = Some(at);
        } else {
            // Faster telemetry does not advance the one-second safety window
            // or repeat the third observation's current-off transition.
            return false;
        }
        self.count == 3
    }
}
