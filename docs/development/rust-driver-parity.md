# Native driver parity

Implementation evidence and physical validation are separate. All evidence below
is offline; no new native driver has been qualified against connected hardware.
The candidate App/Host now uses only the native worker (Task 11 onward). The
installed App has not been replaced. Other driver adapters are not admitted by
the candidate worker until their typed integration task is complete.

## AQ6370

Every public entry point in `Code/Utils/osa.py` and `osa_trace.py` is accounted for.
Rust exposes typed operations, not arbitrary SCPI or panel-setting controls.

| Existing API / behavior | Native target | Regression / state |
| --- | --- | --- |
| `AQ6370(...)`: borrowed manager, resource, timeout, cleanup budget | `Osa::new`, native manager lease, bounded options | implemented; Task 8 lifecycle tests |
| `connect`, context entry | `Osa::connect` | identity gating, read-only startup; Task 8 |
| `probe_identity` | `Osa::probe_identity` | immutable identity + release evidence; Task 8 |
| `read_trace` | `Osa::read_trace` | `read_trace_never_changes_panel`, `changed_context_rejects_capture`; Task 8 |
| `acquire_trace` | `Osa::acquire_trace` | explicitly owned SINGLE/AUTO sweep, no REPEAT-mode initiation; Task 8 |
| `acquire` | `Osa::acquire` / `TraceCapture::spectrum` | exact native capture first; finite dBm compatibility only; Tasks 7–8 |
| `close`, context exit | explicit `Osa::close`, retained transport on `Drop` | panel sweep untouched, owned-sweep cleanup, retryable release; Task 8 |
| `has_resource_responsibility`, `state`, `identity`, `cleanup_error` | read-only native getters | retained native handles and generation cancellation; Task 8 |
| Python helper threads, lifecycle locks, cancellation generation | worker execution slots + native transaction boundary + metadata stop handle | Task 5 scheduler tests; OSA coverage Task 8 |
| native point/chunk/reply limits | 200001 / 1024 / 65536 | `trace_limits_are_exact` |
| `decode_trace_reply` ASCII / LE REAL,32 / LE REAL,64 | same function, `TransferFormat` | `ascii_real32_real64_exact_values`, `truncated_block_is_rejected` |
| malformed/nonfinite/partial/extra trace replies | strict decimal and definite-block validation | `nonfinite_and_density_are_rejected` |
| `TraceContext`, `native_unit` | validated immutable `TraceContext` / `NativeUnit` | density/CALC/contradictory scale/frequency rejected |
| before/after context fields | same 11 wire fields | format, count, spacing, units, attribute, active trace, center/span/resolution, sweep mode |
| `TraceCapture` and all fields | immutable owned arrays + typed context + read timing | `capture_preserves_native_watts_and_rejects_bad_axis_or_context` |
| `TraceCapture.power_dbm` | compatibility getter; never overwrites native W values | positive W conversion; zero W yields no finite dBm representation |
| `Spectrum`, `Spectrum.create` and five fields | validated owned `Spectrum`, `create`, getters | finite increasing wavelength/sample shape; same source/trace semantics |
| read UTC interval and monotonic elapsed | `ReadTiming` | wall-clock adjustment does not alter elapsed; not measurement time |
| consistency | immutable `unproven` | sequential reads never claim atomicity |

Read-only query order is frozen from the reviewed Python implementation:
`:TRACe:ACTive?`, `:UNIT:X?`, `:FORMat:DATA?`, trace sample count,
Y1 spacing/unit, unnamed trace attribute, center/span/resolution and sweep mode;
then alternating bounded X/Y ranges and the same full context again.
Only explicit acquisition may initiate or abort a software-owned sweep.

Task 8 session evidence: `osa_read` (7 tests) and `osa_lifecycle` (8 tests)
cover all public entry points above, literal fragmented VISA replies, the full
200001-point trace, model/probe/close behavior, nonactive/density/CALC/frequency
gates, SINGLE/AUTO versus REPEAT, native OPC timeout, cancellation, failed-open
live handles, and retryable bounded close. Lifecycle rows are implemented,
but physical validation is still pending.

Deliberate migration behavior: one explicit acquisition commands one sweep;
Python's automatic sweep retries are not carried over. A failed/uncertain read
or sweep keeps the session fenced until explicit close. Close returns immutable
partial cleanup evidence when native release is unconfirmed; a later explicit
close reaps the same unfinished native helper instead of launching another one.
No failure causes an implicit replacement measurement. Transport/local VISA
attributes belong to newly opened sessions, never borrowed instrument sessions.

Fixture `Code/Utils/tests/fixtures/osa/samples.json` records reviewed literal
ASCII/little-endian vectors; it is not a hardware capture or Python oracle run.

## Other devices

Voltage, Gain, PM400, MDT693B and Fiber Coupling parity tables are added before
their respective port tasks. They are not native-qualified or silently delegated
to Python by a future partial native-worker milestone.

## Eight-channel Voltage Source (Task 13)

All physical validation below is pending. The telemetry protocol has no model,
serial, measurement ID or timestamp; a decoded frame is weak protocol evidence,
not an invented instrument identity or independent voltage measurement.

| Existing public API / behavior | Exact Rust method / type | Safety / transcript / offline test |
| --- | --- | --- |
| constructor and limit/ramp/finite timing configuration | `VoltageSource::new(VoltageConfig,ResourceBook,Arc<dyn Clock>) -> DriverResult<Self>` | pure validation; native 115200 8N1, no automatic adapter selection |
| `connect`, context entry | `connect(&mut self) -> DriverResult<ProbeReport>` | first telemetry, immediate eight-zero command, post-write host-observed zero before READY; voltage/shutdown suites |
| separate read-only probe | `probe_identity(&mut self) -> DriverResult<ProbeReport>` | opens stream only, no commands; `read_only_probe_never_zeroes_or_sets_ready` |
| `encode_voltages`, `decode_telemetry` | typed `[f64;8]` / `VoltageStatus` codecs | 16 big-endian DAC bytes + CRLF, board scale 28 V; 32 ADC bytes + CRLF, voltage 0.0016 V/code/current signed 0.0025 mA/code; `encode_eight_channels_matches_wire` |
| reader/recovery/freshness | bounded stream decoder + continuous native reader | fragmentation/resync and stale rejection; `telemetry_resynchronizes_without_false_success`, `stale_telemetry_and_zero_evidence_are_unusable` |
| `read_status(max_age)` | `read_status(&self,Duration) -> DriverResult<VoltageStatus>` | cached fresh telemetry only |
| `wait_for_status(timeout)` | `wait_for_status(&self,Deadline) -> DriverResult<VoltageStatus>` | newer sequence, lifecycle cancellation |
| `wait_for_channel(channel,target,timeout,tolerance)` | `wait_for_channel(&self,u8,f64,f64,Deadline) -> DriverResult<VoltageStatus>` | one-based 1–8, post-command boundary; `fresh_wait_and_channel_confirmations_are_post_command` |
| `set_channel` | `set_channel(&mut self,u8,f64) -> DriverResult<()>` | preserve other commanded targets, validate before write; `invalid_channel_or_voltage_writes_nothing` |
| `set_all`, `ramp_to` | respective `(&mut self,[f64;8]) -> DriverResult<()>` | normal DAC steps <=0.1 V, >=50 ms including separate operations; `ramp_is_at_most_point_one_every_fifty_ms` |
| `zero(emergency)` | `zero(&mut self,bool) -> DriverResult<()>` | false ramps, true immediate all-zero frame with cancellation generation; `emergency_zero_is_immediate_and_new_normal_work_is_explicit` |
| fault / reader or writer failure | fault state, independent safety-zero attempt | no nonzero replay; `fault_zeroes_all_channels` |
| `close`, context exit | `close(&mut self) -> DriverResult<CleanupReport>` | immediate zero, bounded post-write telemetry, settle reader before transport close, retry retained attempts; `incomplete_close_keeps_port_until_reader_finishes` |
| zero evidence / cleanup error / state / resource release | immutable getters and cleanup attempt | `unknown`, `command_sent`, `measured_zero` remain separate from resource release and physical measurement; `late_zero_evidence_does_not_mutate_old_report` |
| context scope / object drop | explicit close + retained native responsibility | `drop_retains_pending_reader_until_zero_and_actual_release`, `metadata_stop_fences_queued_nonzero_while_read_is_pending` |
| recovery polling interval | interruptible finite recovery backoff | `recovery_backoff_does_not_spin_and_close_preempts_it`; safety and close intents do not wait for the configured retry interval |

All listed rows are implemented and tested offline; physical validation remains
pending. Normal setters confirm delivery, not measured voltage. Explicit target
waits return fresh host telemetry. A failed native close can be retried for release
only; it cannot reuse old telemetry to upgrade unknown-zero evidence. Reader
spawn/panic recovery keeps an owned session slot, never a detached raw handle.
