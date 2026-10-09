use super::*;

fn validate_ramp(target: f64, step: f64, interval: Duration) -> DriverResult<()> {
    codec::format_fixed(target, 0., 200.)?;
    if !step.is_finite() || !(0.001..=1.).contains(&step)
        || !(Duration::from_millis(50)..=Duration::from_secs(180)).contains(&interval)
    {
        return Err(DriverError::Invalid("Gain ramp requires 0.001–1 mA steps and 50 ms–180 s intervals".into()));
    }
    Ok(())
}

impl GainDriver {
    /// The scheduler captures this generation before releasing its dispatch lock.
    /// Normal calls in the intent must never adopt a later safety generation.
    pub fn with_operation_fence<T>(&mut self, generation:u64, operation:impl FnOnce(&mut Self)->DriverResult<T>)->DriverResult<T> {
        guard(&self.shared,generation)?;
        let previous=self.operation_generation.replace(generation);
        let result=operation(self);
        self.operation_generation=previous;
        result
    }
    fn validate_plan(&self, start: f64, target: f64, step: f64, interval: Duration, budget: Duration) -> DriverResult<()> {
        validate_ramp(target, step, interval)?;
        let start = (start * 1000.).round_ties_even() as i64;
        let end = (target * 1000.).round_ties_even() as i64;
        let count = start.abs_diff(end).div_ceil((step * 1000.).floor() as u64);
        if interval.as_secs_f64() * count as f64 >= budget.as_secs_f64() {
            return Err(DriverError::Invalid("Gain ramp cannot complete within the total operation budget".into()));
        }
        Ok(())
    }
    fn begin_operation(&self, kind: &'static str, target: f64, generation: u64) {
        let mut state = self.shared.state.lock().unwrap();
        state.current_operation = Some(CurrentOperation {
            kind, phase: "checking_tec", active: true, target_ma: target,
            current_ma: state.status.as_ref().map(|s| s.current_ma),
            steps_completed: 0, steps_total: 0, started_at: self.shared.clock.now(),
            finished_at: None, error: None, generation,
        });
    }
    pub(super) fn progress(&self, phase: &'static str, current: Option<f64>, completed: u64, total: u64) {
        if let Some(op) = &mut self.shared.state.lock().unwrap().current_operation {
            if op.generation != self.shared.generation.load(Ordering::Acquire) { return; }
            op.phase = phase;
            op.current_ma = current;
            op.steps_completed = completed;
            op.steps_total = total;
        }
    }
    pub(super) fn progress_phase(&self, phase: &'static str) {
        if let Some(op) = &mut self.shared.state.lock().unwrap().current_operation {
            if op.generation != self.shared.generation.load(Ordering::Acquire) { return; }
            op.phase = phase;
        }
    }
    fn finish_operation(&self, result: &DriverResult<f64>) {
        if let Some(op) = &mut self.shared.state.lock().unwrap().current_operation {
            op.active = false;
            op.finished_at = Some(self.shared.clock.now());
            op.phase = match result { Ok(_) => "completed", Err(DriverError::Canceled) => "canceled", Err(_) => "failed" };
            op.error = result.as_ref().err().map(ToString::to_string);
            if let Ok(current) = result { op.current_ma = Some(*current); }
        }
        if result.as_ref().is_err_and(|e| !matches!(e, DriverError::Invalid(_) | DriverError::Canceled)) {
            self.stop_handle().request_stop();
        }
    }
    pub(super) fn check_budget(&self, generation: u64, deadline: Deadline, clock_end: Duration) -> DriverResult<()> {
        guard(&self.shared, generation)?;
        deadline.remaining_millis()?;
        if self.shared.clock.now() >= clock_end { return Err(timeout("Gain total operation budget")); }
        Ok(())
    }
    pub(super) fn read_number_for(&self, field: u8, generation: u64, deadline: Deadline) -> DriverResult<f64> {
        match self.call_for(Op::Read(field), generation, deadline)? { Reply::Number(v) => Ok(v), _ => unreachable!() }
    }
    pub(super) fn read_flag_for(&self, field: u8, generation: u64, deadline: Deadline) -> DriverResult<bool> {
        match self.call_for(Op::Read(field), generation, deadline)? { Reply::Flag(v) => Ok(v), _ => unreachable!() }
    }
    pub(super) fn set_current_for(&self, value: f64, generation: u64, deadline: Deadline) -> DriverResult<f64> {
        match self.call_for(Op::Set(b'C', value), generation, deadline)? { Reply::Number(v) => Ok(v), _ => unreachable!() }
    }
    pub fn ramp_current(&mut self, target: f64, step: f64, interval: Duration) -> DriverResult<f64> {
        let generation = self.operation_generation.unwrap_or_else(||self.shared.generation.load(Ordering::Acquire));
        let initial = self.read_status()?;
        guard(&self.shared,generation)?;
        self.validate_plan(initial.current_ma, target, step, interval, Duration::from_secs(180))?;
        let deadline = Deadline::after(Duration::from_secs(180));
        let clock_end = self.shared.clock.now() + Duration::from_secs(180);
        self.begin_operation("ramp_current", target, generation);
        let result = self.ramp_current_for(target, step, interval, generation, deadline, clock_end);
        self.finish_operation(&result);
        result
    }
    /// One fenced intent. TEC is never automatically enabled, and timeout is total.
    pub fn start_current(&mut self, target: f64, soft_start: bool, step: f64, interval: Duration, timeout: Duration) -> DriverResult<f64> {
        let generation = self.operation_generation.unwrap_or_else(||self.shared.generation.load(Ordering::Acquire));
        codec::format_fixed(target, 0., 200.)?;
        validate_ramp(target, step, interval)?;
        if !(Duration::from_millis(50)..=Duration::from_secs(180)).contains(&timeout) {
            return Err(DriverError::Invalid("Gain total startup timeout requires 50 ms–180 s".into()));
        }
        let initial = self.read_status()?;
        guard(&self.shared,generation)?;
        if soft_start {
            // Q=1 resets the controller setpoint to 3 mA; the actual value is
            // nevertheless read back after enable, and the remaining plan rechecked.
            self.validate_plan(if initial.current_enabled { initial.current_ma } else { 3. }, target, step, interval, timeout)?;
        }
        let deadline = Deadline::after(timeout);
        let clock_end = self.shared.clock.now() + timeout;
        self.begin_operation("start_current", target, generation);
        let result = (|| {
            if !self.read_flag_for(b'R', generation, deadline)? {
                return Err(DriverError::Invalid("Gain current startup requires TEC already enabled".into()));
            }
            self.progress_phase("waiting_stable");
            self.wait_stable_for(deadline, generation, Some(clock_end))?;
            self.check_budget(generation, deadline, clock_end)?;
            self.progress_phase("enabling");
            self.call_for(Op::EnableCurrent, generation, deadline)?;
            self.check_budget(generation, deadline, clock_end)?;
            let current = self.read_status()?.current_ma;
            if soft_start {
                self.validate_plan(current, target, step, interval, clock_end.saturating_sub(self.shared.clock.now()))
                    .map_err(|error|blocked(&format!("Gain enabled output cannot finish the remaining ramp: {error}")))?;
                self.ramp_current_for(target, step, interval, generation, deadline, clock_end)?;
            } else {
                self.progress("ramping", Some(current), 0, 1);
                let applied = self.set_current_for(target, generation, deadline)?;
                if (applied - (target * 1000.).round_ties_even() / 1000.).abs() > 0.0005 {
                    return Err(DriverError::Protocol("Gain current acknowledgement differs from requested target".into()));
                }
                self.progress("ramping", Some(applied), 1, 1);
            }
            self.progress_phase("verifying");
            self.check_budget(generation, deadline, clock_end)?;
            self.call_for(Op::Snapshot, generation, deadline)?;
            let final_status = self.read_status()?;
            if !final_status.current_enabled || !final_status.tec_enabled
                || (final_status.temperature_c - final_status.target_c).abs() > 0.2
                || (final_status.current_ma - (target * 1000.).round_ties_even() / 1000.).abs() > 0.0005
            { return Err(blocked("Gain startup final readback/safety mismatch")); }
            self.check_budget(generation, deadline, clock_end)?;
            Ok(final_status.current_ma)
        })();
        self.finish_operation(&result);
        result
    }
}
