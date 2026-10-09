# Gain console redesign

The operator wants a clear Gain page with a rolling temperature plot, an overall
status panel, and two balanced control columns: Temperature Control and Current
Control. Normal values must not carry repetitive Fresh / seconds-ago labels.
Targets, PID, output switches and current ramp settings must be practical to use;
stable waits should be automatic without inventing safety or completion evidence.

This extends the existing native driver/Host/Worker flow; it adds no selectable
backend, dependency, hardware diagnostic or new resource owner.

## Presentation

The upper row contains a wide live temperature plot and a compact overall status
card. The lower row contains equal Temperature and Current cards. Small screens
stack the cards. Use the existing light/teal palette, clear numeric hierarchy,
subtle grid lines and restrained status colors. Routine protocol details live in
Diagnostics; fault, disconnected/historical and uncertain states remain visible.

The temperature plot uses observation timestamps, not sample index. It supports
1 / 5 / 15 minute windows, default 5 minutes, and contains only accepted actual
observations. History remains bounded (4801 points, supporting the sixteen-minute
retention buffer at the 200 ms / 5 Hz sampling and cache publication cadence), deduplicates field revisions,
continues collecting when another page is selected, resets on Host boot / Worker
session / connection changes, and breaks lines across missing or invalid samples.
Scale includes all visible finite temperatures and target values, including a
literal zero readback; it must not clip zero into a plausible 15 degC reading.
The target is visually distinct from temperature. No chart timer creates samples.

Temperature controls expose a draft 15–40 degC target (Apply or Enter), a single
state-derived TEC switch, and PID P / I / D in an expandable section. PID fields
start unknown until an actual read_pid / set_pid readback exists for the current
connection. PID getter failure never installs assumed coefficients.

Current controls expose a draft 0–200 mA target, Apply or Enter, a single
state-derived output switch, soft-start and smooth-change options (both default
on), plus step and interval settings (default 1 mA / 100 ms). Start timeout is the
total compound-operation budget, default 30 s, maximum 180 s. Drafts, focus,
selection and scroll survive observations and waits. Edits never send commands.

## Native contracts and automation

Expose read_pid {}, set_pid {p,i,d}, ramp_current {current_ma,step_ma,interval_s}
and start_current {current_ma,soft_start?,step_ma?,interval_s?,timeout_s?} through
the existing catalog, typed Host validator, Worker schema and native adapter.
PID values are 0–999.999. Ramp step is 0.001–1 mA and interval 0.05–180 s; driver
limits and confirmed acknowledgements remain authoritative. Overall operations
are bounded and impossible plans fail before enabling output.

start_current requires TEC already enabled. One native operation generation
covers confirmed TEC -> stable temperature -> current enable -> optional ramp ->
final verification. Stability remains +/-0.2 degC for five continuous seconds.
Q=1 resets the controller current setting to 3 mA; soft-start uses confirmed
enabled current rather than promising a zero-current initial enable. Existing
watchdog limits and current-off-before-TEC-off behavior remain unchanged.

The native Gain monitor targets one serialized five-field snapshot every 200 ms
(5 Hz), with no overlapping reads or catch-up bursts after a slow response.
Already queued output-off work takes priority over the next scheduled poll.
The Worker publishes Gain's pure cached observations at 200 ms. The Host also
queries worker metadata and publishes observations at a 200 ms target while a
known Gain connection is healthy and owned; its active cache TTL is 200 ms.
Due fast publications refresh metadata rather than beating against a separate
cache clock. Completion wakes the idle publisher, and missed ticks are skipped.
These are metadata-only queries; other devices keep their existing cadence.
Independent scheduler phases may omit intermediate revisions; history retains
only actual received observations and never invents samples.
Faster reads do not shorten the five-second
thermal interlock. Moderate-deviation counting keeps its one-second spacing,
and only the threshold crossing triggers a current-off command for that
deviation sequence. Packet deadlines, stale evidence and shutdown limits remain
unchanged. The polling target does not claim a controller sensor conversion rate
or a measured hardware throughput result.

Output-off commands can preempt pending normal work without replacing the
original normal operation's identity. Cancel/timeout/unknown does not replay a
command. Only actual typed shutdown evidence establishes Off. Cached observations
must continue during waits and ramps without contending on the action mutex or
issuing extra I/O. No observation may grant authority or complete a pending call.

After a confirmed Off, the next explicit normal operator command may automatically
resume the same owned, healthy, quiescent STOP_HELD context through the existing
metadata-only resume operation. It then rechecks lease, boot, connection and fresh
readback before submitting the captured command once. It never resumes a failed,
unknown or active attempt, restores output automatically, or replays canceled work.
Stronger TEC Off requested during current Off retains its own intent and waits for
confirmed prior completion; an unknown result blocks that queued request.

PID telemetry is null or {values:[p,i,d],connection_id,revision,quality,
observed_age_s,error}; its getter timestamp is independent of thermal polling.
Current operation telemetry is null or {kind,phase,active,target_ma,current_ma,
steps_completed,steps_total,elapsed_s,error}. Phases include checking_tec,
waiting_stable, enabling, ramping, verifying, completed, failed and canceled.
Percent progress is used only for counted acknowledged steps; waits are
indeterminate. The C field is a controller current setting, not an independent
measurement of output current.

## Verification and delivery

Behavioral RED/GREEN tests cover schema/limits, PID readback, generation races,
deadline, stability gating, soft-start and safety preemption. Frontend coverage
includes irregular sample timing, gaps/connection reset, retained drafts and
pending/unknown outcomes. Mount the real console in a finite headless-browser
fixture and inspect layouts at 800 / 960 / 1440 px and output-on/off/wait/fault
states. Run the native offline regression and closed package checks, then build
and qualify an exact portable Rust candidate. Switch only after normal operator
release and App exit; actual hardware acceptance is distinct from offline checks.
