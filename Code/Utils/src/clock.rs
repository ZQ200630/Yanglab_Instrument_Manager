use std::{
    sync::Mutex,
    time::{Duration, Instant},
};
pub trait Clock: Send + Sync {
    fn now(&self) -> Duration;
    fn wait(&self, duration: Duration);
}
pub struct SystemClock(Instant);
impl Default for SystemClock {
    fn default() -> Self {
        Self(Instant::now())
    }
}
impl Clock for SystemClock {
    fn now(&self) -> Duration {
        self.0.elapsed()
    }
    fn wait(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}
#[derive(Default)]
pub struct ManualClock(Mutex<Duration>);
impl Clock for ManualClock {
    fn now(&self) -> Duration {
        *self.0.lock().unwrap()
    }
    fn wait(&self, duration: Duration) {
        let mut current = self.0.lock().unwrap();
        *current = current.saturating_add(duration);
    }
}
