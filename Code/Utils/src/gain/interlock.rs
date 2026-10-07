use std::{collections::VecDeque, time::Duration};
#[derive(Default)]
pub struct ThermalInterlock {
    samples: VecDeque<Duration>,
    latest: Option<Duration>,
    target: Option<f64>,
    tec: Option<bool>,
    epoch: u64,
}
impl ThermalInterlock {
    pub fn invalidate(&mut self) {
        self.samples.clear();
        self.latest = None;
        self.epoch = self.epoch.saturating_add(1);
    }
    pub fn epoch(&self) -> u64 {
        self.epoch
    }
    pub fn observe(&mut self, temperature: f64, target: f64, tec: bool, at: Duration) {
        if self.target != Some(target) || self.tec != Some(tec) {
            self.invalidate();
        }
        self.target = Some(target);
        self.tec = Some(tec);
        if !temperature.is_finite()
            || !(15. ..=40.).contains(&target)
            || !tec
            || (temperature - target).abs() > 0.2
        {
            self.invalidate();
            return;
        }
        if self.latest.is_some_and(|last| at < last)
            || self
                .samples
                .back()
                .is_some_and(|last| at.saturating_sub(*last) > Duration::from_millis(1500))
        {
            self.invalidate();
        }
        self.latest = Some(at);
        if self
            .samples
            .back()
            .is_none_or(|last| at.saturating_sub(*last) >= Duration::from_secs(1))
        {
            self.samples.push_back(at);
            if self.samples.len() > 6 {
                self.samples.pop_front();
            }
        }
    }
    pub fn ready(&self, now: Duration) -> bool {
        self.samples.len() == 6
            && self
                .latest
                .is_some_and(|at| now >= at && now - at <= Duration::from_millis(1500))
            && self
                .samples
                .back()
                .is_some_and(|at| now >= *at && now - *at <= Duration::from_millis(1500))
            && self
                .samples
                .back()
                .unwrap()
                .saturating_sub(*self.samples.front().unwrap())
                >= Duration::from_secs(5)
            && self
                .samples
                .iter()
                .zip(self.samples.iter().skip(1))
                .all(|(a, b)| {
                    *b >= *a
                        && (Duration::from_secs(1)..=Duration::from_millis(1500))
                            .contains(&(*b - *a))
                })
    }
}
