# Native driver parity

Implementation evidence and physical validation are separate. All evidence below
is offline; no new native driver has been qualified against connected hardware.
The existing App still uses its existing backend until the explicit cutover tasks.

## AQ6370

Every public entry point in `Code/Utils/osa.py` and `osa_trace.py` is accounted for.
Rust exposes typed operations, not arbitrary SCPI or panel-setting controls.

| Existing API / behavior | Native target | Regression / state |
| --- | --- | --- |
| `AQ6370(...)`: borrowed manager, resource, timeout, cleanup budget | `Osa::new`, native manager lease, bounded options | Task 8, pending |
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

Fixture `Code/Utils/tests/fixtures/osa/samples.json` records reviewed literal
ASCII/little-endian vectors; it is not a hardware capture or Python oracle run.

## Other devices

Voltage, Gain, PM400, MDT693B and Fiber Coupling parity tables are added before
their respective port tasks. They are not native-qualified or silently delegated
to Python by a future partial native-worker milestone.
