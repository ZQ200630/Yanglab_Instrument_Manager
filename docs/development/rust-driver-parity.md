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

## Gain Chip Driver (Task 14)

The READY protocol has no instrument model/serial query. CP210x USB discovery
is adapter evidence, not an invented instrument identity. Driver connection is
read-only unless a confirmed unsafe current-on/TEC-off invariant or a failure
triggers the separately disclosed normal-driver shutdown policy. The explicit
read-only probe never writes output commands, including on failure.

| Existing public API / behavior | Rust equivalent | Offline regression / evidence |
| --- | --- | --- |
| constructor, explicit port / CP210x preferred USB serial with unique VID/PID fallback | `GainConfig`, `GainDriver::new` / `with_backend` | pure validated one-second watchdog interval, finite budgets, shared canonical reservation; `invalid_config_opens_nothing` |
| `connect`, context entry | `connect` | RDTA/RDEA/RDRA/RDCA/RDQA only on normal startup; typed returned snapshot; active current-without-TEC trips Q=0 then D=0 |
| read-only identity diagnostic | `probe_identity` | `read_only_probe_never_runs_gain_shutdown`, `failed_readonly_probe_and_wrong_enable_ack_never_claim_success` |
| status/state/cleanup/fault/resource responsibility | immutable `GainStatus`, cached getters / `read_status` | `received_at` is actual temperature-reply receipt, not later-query completion; stale cache rejects; `later_status_queries_cannot_refresh_an_older_temperature` |
| all numeric / boolean typed reads | `read_temperature`, `read_target`, `read_current`, `read_tec_enabled`, `read_current_enabled` | exact expected fields T/E/C/R/Q; `all_typed_reads_use_confirmed_values` |
| target / current setters | `set_temperature`, `set_current` | 15–40 degC / 0–200 mA validation before bytes; return acknowledged values, never requested values as facts; `limits_reject_before_write` |
| enable/disable TEC and current | `enable_tec`, `disable_tec`, `disable_current` | boolean ACK is exactly 0/1; TEC disable commands current off first; no interleaved ordinary job |
| thermal stability wait / current enable | `wait_stable`, `enable_current`, internal `ThermalInterlock` | six samples spanning >=5 s; adjacent samples 1–1.5 s; latest evidence <=1.5 s; target/TEC transitions, gap/deviation/failure invalidate; `five_seconds_requires_fresh_continuous_samples`, `rapid_reads_cannot_fabricate_stability`, `target_change_invalidates_the_entire_sequence` |
| enable-time live recheck and controller 3 mA reset | `enable_current` | full fresh status, unchanged epoch, then Q=1 and actual RDCA; `enable_reads_actual_reset_current_and_ramp_is_bounded`; already-on observation never reissues Q=1: `repeated_enable_never_reissues_q1_or_resets_running_current` |
| bounded current ramp and final readback | `ramp_current` | <=1 mA quantized steps, >=50 ms interval, final current/enable/TEC/temp/target reads; metadata cancellation in <=50 ms wait quanta; `long_ramp_waits_are_interruptible_at_fifty_ms` |
| PID reads, set, reset, integral clear | `read_pid`, `set_pid`, `reset_pid`, `clear_integral` | RDPA/RDIA/RDDA, STPA/STIA/STDA, RST + typed readback, CLR; all coefficients finite 0–999.999; `pid_functions_retain_reviewed_protocol` |
| format / build / READY reply and generic ACK helpers | `gain::codec` | CRLF, ASCII, six milli-unit digits, ties-to-even, exact expected field; `gain_fixed_commands_and_ready_fields_match` |
| temperature watchdog | serialized actor + `monitor` / `interlock` | >1 degC for three consecutive samples disables current; >3 degC shuts current then TEC; `moderate_and_severe_thresholds_preserve_shutdown_order` |
| native/reply failures | fenced actor and transport | first failed monitor exchange trips, unlike legacy three failed reads; no nonzero replay / no late partial reply used as next ACK; `tec_off_or_monitor_failure_disables_current`, `each_packet_respects_io_timeout_not_the_whole_compound_budget` |
| close/context exit/drop, late completion/retry | `close`, `DriverLifecycle`, retained owned actor | immutable pending receipt, only actual close releases reservation, failed native close retries release only; `shutdown_orders_current_before_tec`, `pending_poll_close_preserves_immutable_receipt_and_resource`, `failed_shutdown_cannot_reuse_an_earlier_off_ack` |
| close/stop races with enable | epoch-scoped metadata `StopHandle` | `stop_during_enable_read_cannot_send_q1`; no caller timeout force-kills an unfinished native read |

All rows are implemented/offline tested; physical validation is pending. This
native monitor reads the full five-field status at each one-second tick, whereas
the legacy monitor queried only temperature. The first invalid monitor exchange
faults instead of allowing three failures. These are deliberate conservative
changes, not claims that failed shutdown writes physically reached the device.
Release, current-off acknowledgement and TEC-off acknowledgement remain separate.

## PM400 session and measurement families (Task 15)

| Existing public surface | Native equivalent | Evidence / conditionality |
| --- | --- | --- |
| construction, connect, identify, instrument/sensor info | `Pm400::new/with_options/connect/identify/instrument_info/sensor_info` | `connect_close_preserve_measurement_settings`, strict CSV including quoted commas, four-field Thorlabs/PM400 identity; normal connect only identity/sensor reads |
| read-only identity probe, cleanup and responsibility | `probe_identity/close/has_resource_responsibility/cleanup_error/cleanup_report` | `readonly_probe_and_wrong_identity_do_not_change_settings`, `late_native_close_retains_immutable_attempt_and_reservation`; shared canonical resource stays reserved until native close completes |
| all nine scalar kinds + root convenience methods | typed `MeasurementKind`, `measure`, nine `measure_*` methods | `nine_measurement_kinds_keep_units`, exact suffixes; watts/DBM read from `SENSe:POWer:DC:UNIT?`, other units W/cm²/J/cm²/A/V/J/Hz/ohm/degC retained |
| configure, get configuration, fetch, read (optional kind) | `configure/get_configuration/fetch/read/fetch_configured/read_configured` | explicit mode-changing operations only, supported full/short suffix forms; `fetch_read_configuration_and_actions_are_explicit` |
| initiate/abort, cancellation and active status | `initiate/abort/cancel_measurement/stop_handle/state` | `cancel_and_late_measurement_keep_ownership`; metadata-only cancellation quarantines uncertain exchange and requires explicit close/reconnect, never replay or forced handle destruction |
| sensor capability flags | typed `SensorInfo/SensorCapabilities` | `unsupported_sensor_never_receives_command`: power, energy, temperature individually gated before any measurement/configuration write; unknown flags preserved, zero flags report unavailable functions |
| scalar parsing/timeouts | finite/sentinel-checked `Measurement`, per-packet and caller deadlines | `malformed_or_trailing_values_never_become_measurement`, `native_deadline_and_primary_protocol_error_remain_truthful`, `query_packets_never_inflate_configured_timeout` |
| standard-event/status-byte/OPC + status event/condition queries | typed root methods and `status()` | `strict_csv_identity_sensor_and_register_evidence`; event and error-queue reads are consuming, not labeled nondestructive |
| root and facade settings/maintenance | Task 16 below | no generic public SCPI entry point; firmware and sensor physical qualification remains pending |

Native I/O runs within the scheduler's device domain, not the UI thread. A vendor
call that ignores its timeout retains its owning driver/session until completion;
metadata stop does not concurrently destroy the native handle. Closing a normal
PM400 session never resets, zeros, changes configuration, or sends ABORt. Error
queue attribution drains are explicit action side effects, distinct from identity
queries. Physical probe coverage and sensor qualification remain unperformed.

## PM400 settings and maintenance (Task 16)

All paired entries retain their `set_*`/`get_*` snake-case methods; `sense()`,
`input()`, `system()`, `display()`, `calibration()` and `status()` borrow the one
session owner, so a compound set/readback cannot interleave another transaction.

| Facade / existing public families | Native validation and evidence |
| --- | --- |
| Sense: average count, loss_db, beam_diameter_mm, wavelength_nm | positive typed count; sensor-reported finite MIN/MAX and property-specific DEFAULT support; wavelength setter requires wavelength flag |
| Sense: photodiode_response_a_per_w, thermopile_response_v_per_w, pyro_response_v_per_j | explicit confirmation + response-settable + power/energy flags before any I/O; exact typed readback, no copied requested value |
| Sense: current_auto_range/current_range_a/current_reference_a/current_delta_enabled | power flag, literal bool ACK, reviewed current SCPI headers and MIN/MAX vs reference DEFAULT selectors |
| Sense: energy_range_j/energy_reference_j/energy_delta_enabled/peak_threshold_percent | energy flag, finite device bounds and exact property-specific selectors |
| Sense: power_auto_range/power_range_w/power_reference_w/power_delta_enabled/power_unit | power flag; exact `W`/`DBM` unit tokens and `:DC:` headers |
| Sense: voltage_auto_range/voltage_range_v/voltage_reference_v/voltage_delta_enabled | power flag, finite device limits, strict bools |
| Sense: get_frequency_upper_hz/get_frequency_lower_hz | read-only finite numeric frequency bounds |
| Sense: start_zero_collection/abort_zero_collection/get_zero_state/get_zero_magnitude | explicit confirmation for start, optical capability before start/abort writes; reported zero state/magnitude, never physical calibration-success claim |
| Input: photodiode_lowpass_enabled, thermopile_accelerator_enabled/auto, thermopile_tau_s | power capability; tau setter additionally requires tau-settable; all exact property readbacks |
| Input: adapter type | typed Photodiode/Thermal/Pyro, explicit confirmation; strict documented uppercase short/full replies, no free-form tokens |
| System: beep/beeper, next_error/drain_errors, SCPI version, sensor info | error reads/drains are consuming; finite 1..128 drain count; action errors attributed only after recording pre-existing queue entries |
| System: date/time/line_frequency_hz | private valid Gregorian/date and timezone-free microsecond time types; 50 or 60 Hz only; exact readback |
| Display: brightness/contrast; Calibration: get_string | finite display values with reviewed 1e-12 comparison, no invented numeric bounds; calibration string query only |
| Status: read_event/condition, positive/negative transition, enable, preset | all four groups; 0..65535 masks, events clear-on-read; preset requires explicit confirmation |
| Root: clear_status, standard/service enable getters/setters, standard event, status byte, mark/wait OPC, reset, self_test, wait_to_continue | 0..255 masks; reset requires confirmation; event/status effects remain distinct from measurement settings |

`all_property_table_rows_have_typed_rust_methods` executes the reviewed literal
method/command matrix; `maintenance_requires_confirmation_before_write`,
`capability_checks_precede_any_write`, `range_selectors_and_readbacks_match`,
`date_time_masks_and_adapter_limits_are_strict` and
`strict_units_and_consuming_error_attribution` cover refusal, readback and parsing.
Sensors and firmware still need separately authorized physical qualification.

## MDT693B protocol and read-only lifecycle (Task 17)

| Existing public surface | Native equivalent and evidence |
| --- | --- |
| port/limits/timing construction | `MdtConfig`, pure `new/with_backend`, lowered per-axis limits only; 115200/8N1, no flow control or inherited DTR/RTS; no automatic discovery |
| connect / identity probe | only the reviewed 22 queries including `?`, `id?`, `serial?`; firmware command list authoritative for `xmin?`; exact MDT693B and nonempty serial; no setter, zero, reset, arrow, or limit change |
| read-only recover | full query snapshot, always `axis_command_known=false`; clean-stream recovery only, ambiguous transport requires explicit close/reconnect |
| supported commands/product/serial/XYZ/axis state | typed `get_supported_commands/get_product_information/get_serial_number/get_axis_voltage/get_all_voltages/get_axis_state`; serialized complete snapshot; externally present 90 V reported as 90 V, not silently clamped |
| immutable cached status / fault evidence | `MdtStatus`, three `AxisState` rows, hardware limit/control settings; restricted latch survives ordinary reads until explicit recovery |
| monitor | one owning bounded actor, default XYZ polling every 500 ms; three complete-frame failures fault, ambiguous framing faults immediately; no output writes |
| close / responsibility / Drop retention | `close/has_resource_responsibility/cleanup_error/cleanup_report/stop_handle`, retained owner and release-only retries; stop-and-hold, never automatic zero |
| prompt/echo protocol | pure `parse_reply`, private native transport framing; CR/LF/CRLF, unknown/on/off echo, deferred LF and 4096-byte bound; stale/trailing data rejected before next command |

Named protocol/lifecycle tests cover all rows, including canceled in-flight reads
and immutable unsuccessful close evidence. Receive-count inspection uses the
native serial buffer count; there is no post-prompt blocking read or input purge.
All evidence is offline; actual MDT firmware, physical outputs and stage motion
have not been qualified by this migration. Settings and motion authority follow
in Task 18, not from a read-only snapshot.

## MDT693B complete settings and motion (Task 18)

| Existing family | Native API and safety/evidence |
| --- | --- |
| friendly name / echo / display intensity | paired typed getters/setters; bounded ASCII name, echo-transition parser, intensity 0..15; setter ACK and actual readback |
| axis min/max / DAC step / compatibility / rotary / push-to-adjust | paired typed methods; fresh bounds cannot exclude actual output, raise 75 V project ceiling or invert min/max; DAC 1..1000, typed modes/bools |
| absolute axis / all-axis voltage | `set_axis_voltage`, confirmed `set_all_voltages`; requires current baseline authority, fresh Master/XYZ/hardware/axis limits, <=0.1 V steps and >=50 ms spacing across operations; unequal bases converge individually before simultaneous final command |
| Master Scan | paired getters plus explicitly confirmed enable/voltage setters; enable toggles only with freshly read zero contribution, voltage ramps only while enabled; actual XYZ and programmed property checked after every write |
| baseline adoption | `baseline_attestation(true)` produces private connection/generation-bound evidence; `adopt_current_axis_baseline` reads fresh state, requires Master off/zero and safe axes; operator assumption is labeled, not software proof of no external contribution |
| explicit zero | `ramp_to_zero` requires established authority; `emergency_zero(true)` sends exactly Master=0, all axes=0, Master disable, then verifies complete fresh Master/XYZ/limits; partial ACK or residual never arms motion |
| channel selection / increment / decrement | fixed private ANSI candidates without CR/LF; firmware help gates, rejected candidate latched unsupported; selections must not move voltage, increments use freshly verified hardware/DAC scale <=0.1 V and conservative bounds for every selectable axis |
| factory defaults | `restore_factory_defaults(true)` only; complete readback, truthful restriction/fault, always disarmed even if all outputs happen to read zero |
| external/manual change, cancel and close | any unexplained XYZ, Master, identity or generation change invalidates authority; fault always holds without rollback/zero; urgent bounded queue fences older work, stale queued jobs cannot revoke newer verified zero; late publication cannot rearm or overwrite closing |

17 motion/settings integration tests plus one publication regression cover these
rows. Motion remains open-loop electrical control: confirmed software readback is
not measured stage displacement or physical zero. All evidence remains offline.
