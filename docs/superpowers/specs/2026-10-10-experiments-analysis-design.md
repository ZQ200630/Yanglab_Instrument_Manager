# Native waveform, experiments and analysis design

## Authority and intended outcome

The user approved the architecture in this conversation and explicitly requested a detailed implementation specification followed by autonomous execution without additional design/plan approval. This supersedes workflow approval pauses, not hardware safety or permissions. Preserve all existing uncommitted work. Do not push, merge, install over a running App, or actuate hardware as part of offline implementation.

Recreate the operations of `Rigol_Osc_Control_ReconstructNew (3).zip` in the existing real-only, Rust-backed Yang LAB INSTRUMENT CONSOLE. The reviewed source copy is `Result/rigol-review-20261010/source`; the original ZIP and `reference_code` remain read-only. Product/runtime/build must not require Python or Anaconda.

Success is usable instrument pages and model-independent waveform browsing, persistence, export, Q/FSR analysis, plus parameterized multi-instrument experiment templates. A library-only implementation, UI-only mock, or unreachable API is not completion.

## Product boundaries

- Navigation: Overview, Device Setup, Experiments, Analysis, Settings. Existing dynamic instrument pages remain.
- Choose a concrete model in Device Setup. Preserve the simplified discovered-resource picker, automatic names, hidden fixed backend, collapsed timeout and manual fallback.
- Stable family core + explicit optional capabilities + typed model extensions. A supported driver must implement its core, not return fabricated samples or no-op success.
- Initial scope models: Rigol DS7054 and MHO5054. Initial generators: Rigol DG4162, DG4202, DG922Pro. Each has reviewed VISA profiles; valid GPIB, USB and TCPIP resources are separately admitted, never guessed as identity.
- Scope core: identity/capabilities, channel display read/write, acquisition status/run/stop, supported memory depths/read/write, bounded Screen/Memory waveform acquisition. Raw transport dialect/representation/scaling belongs to drivers.
- Generator core: channel signal readback, waveform/frequency/Vpp/offset configuration, output enable/readback. Phase, load and polarity remain available through capability-gated typed operations. Triangle must map to verified waveform/symmetry commands, not an assumed RAMP string.
- Laser role uses the existing native TLB controller interface. No laser emission is automatically enabled by a template.
- No general-purpose node editor, arbitrary user SCPI, selectable simulation mode, or Python plugins.

## Code boundaries

- `Code/Utils/src/scope` and `Code/Utils/src/generator`: native family interfaces, per-model protocol adapters, bounded decode, validation and lifecycle.
- `Code/Data`: new `yang-data` crate. Typed immutable samples, calibrated views, Record/Frame organization, viewport reduction, persistence/import/export and pure analysis. No VISA, serial, Tauri or driver dependency.
- `Code/Experiments`: new native `yang-experiments` crate with templates under short case directories. Runner depends on injected family-facing ports, never serial/VISA or Tauri. Keep existing Python experiments unchanged as references.
- Existing Worker/Host: driver factory, admission, ownership, operation journal, bounded task execution, capture transfer and native data/experiment services.
- `App/web`: English, light-theme direct-control pages and data workbench. Large processing occurs in Rust; browser receives bounded viewport data, never the complete deep-memory array as JSON.

## Waveform contract

Use schema version 1. `Samples` preserves U8, U16 or F64 actual values; signed legacy values remain explicitly represented, never reinterpreted from erroneous metadata. `Calibration` contains `x_increment_s`, `x_origin_s`, `x_reference`, `y_increment_v`, `y_origin`, `y_reference`. Each `ChannelWaveform` owns channel ID, samples and calibration. `WaveformFrame` owns channels, actual capture time and optional nominal offset. `WaveformRecord` owns stable ID, editable name/comment, provenance and frames. Do not use editable names as paths/identity. Keep original preamble/model/conversion convention in provenance.

Time is `(absolute_sample_index - x_reference) * x_increment_s + x_origin_s`. The per-model driver determines the Y-origin convention and supplies the normalized calibration. Each channel retains its own axis; export must not assume equal lengths/timing without checking.

Public crate contracts: `Samples`, `Calibration`, `ChannelWaveform`, `WaveformFrame`, `WaveformRecord`, `AnalysisRequest`, `AnalysisResult`, `DataStore`. Their serialized field names are snake_case and schemas reject unknown incompatible fields. The exact Rust constructors/fields are frozen in Task 1 before dependent code is implemented.

Support 1 through 9999 requested frames, model-supported depths up to 250M points, and bounded transport chunks. Admission checks declared byte counts and configured resource budgets before allocating. Store raw arrays in binary files; process channels/frames incrementally and provide cancellation checks between chunks. Configured limits and insufficient-space errors must be visible, not silent truncation to 1000 points.

## Data workbench and compatibility

- Record -> Frame -> Channel browser, editable name/comment, frame/channel selection, deletion with confirmation, clear, save/load/append, selected-frame/record/export-all.
- Native schema-versioned storage uses immutable raw payloads and atomic metadata commits. Changes to labels do not mutate sample evidence. Interrupted saves cannot overwrite the last valid file.
- Import legacy `.db` ZIP (`database.json` plus NPY), unpacked equivalent, CSV and NPZ. The actual NPY dtype is authoritative; preserve/report conflicts with legacy metadata. Imported history has no live hardware authority.
- Path confinement, ZIP member/count/uncompressed-size bounds, finite arrays and shape validation are required. No extractall and no pickle/object array execution. Reject ambiguous malformed imports with actionable errors.
- Preserve legacy CSV columns `Time (us),CH1 (V),...` and NPZ `time_us`, `channel_names`, per-channel calibrated voltages. Exports include metadata/analysis sidecars, collision-safe filenames and per-file batch outcomes. Preserve a raw round-trip save format.
- Existing OSA archive data remains valid. Add a typed waveform capture kind rather than applying OSA wavelength/power validation or 200001-point bounds to it. Reuse existing chunking, ownership, immutable origin and durability mechanisms where appropriate.

## Display and analysis

- Preserve multichannel overlays, legends, horizontal ROI zoom, reset, and min/max rendering reduction. Retain sample-index/raw-code inspection and provide calibrated time/voltage units.
- Viewport reduction is display-only; preserve true extremum indices and tail samples. Analysis has its own explicit, versioned reduction policy, never consumes mutable rendered curves.
- Single Drop: inverted Lorentzian, fitted curve, positive FWHM and Q; scan mapping uses calibration `2 * ramp_hz * excursion_pm`. Save requested calibration, ROI, parameters, algorithm version, fit status, residual statistics and source references.
- FSR: dip detection, prominence/distance controls with sensible legacy defaults, regular contiguous-subset regression, dip markers, spacing in nm using supplied scan speed. Retain a documented legacy-comparison algorithm where necessary; scientific corrections must carry an algorithm version, not silently claim numerical equality with SciPy.
- Validate nonfinite input, empty/too-short ROI, zero scan calibration, flat data, missing channels, degenerate regression and nonconvergence. Do not display NaN or a made-up successful fit.
- Analysis works offline on saved/imported data and can be rerun without changing original samples. No FFT or other unimplemented legacy claims are required in this increment.

## Experiments and execution

Template is versioned case logic; Experiment is a saved configuration; Run is one immutable parameter/device snapshot plus progress, operation references, raw captures and derived outputs.

Initial templates: waveform capture, Q Measurement and FSR Measurement. Bind explicit `(host_id, domain_id, channel)` roles, never COM order/model string guessing. Q/FSR roles: Laser, Scan Signal and Detector. Parameters include optional coarse wavelength, selected channels, capture mode/depth/frame count, triangle frequency/Vpp/offset, analysis calibration and ROI. Defaults from the example are 50 Hz and 2 Vpp; no implicit output change on page open or configuration save.

Preflight verifies all roles, identities, required capabilities, parameters and ownership before any actuation. Acquire reservations in deterministic order; release partial reservations if preflight fails. Manual and experiment control share Host authority. No direct driver bypass. A run advances explicit steps: prepare, optional coarse tune/settle, configure scan, arm/acquire, persist, analyze, cleanup.

Instrument hardware generates timing-critical waveforms and triggers; network/UI scheduling is not deterministic sampling. Real capture/readback is required before claiming success. Multi-host preparation is coordination, not an atomic hardware transaction. Cross-host roles must either execute through an authenticated peer adapter with explicit loss handling or be rejected before output changes; never quietly ignore a remote role.

GUI closure and navigation must not strand hardware ownership. The owning Host executes runs; explicit stop fences future steps and invokes each driver's defined cleanup. Generator outputs changed by a run are disabled on run cleanup by the declared policy; no blanket zero/rollback for lasers or piezos. Preserve partial data and uncertain command outcomes. Never automatically replay a command after timeout/restart.

## Responsiveness, verification and delivery

Clicks acknowledge immediately. Background jobs expose queued/running/completed/failed/cancelled/uncertain status, stage and elapsed time; percentages only from measured frames/bytes. Bound work queues, file operations and plot payloads. Keep navigation/history usable while collecting or fitting.

TDD for new behavior; focused native/JS tests then full `App/scripts/test-native.ps1 -Offline`. Use delayed, cancelled, malformed, disconnected, multi-device contention and partial-transfer cases. Use explicit transport doubles only in tests; no product simulation devices. Add native preview/read-only diagnostics under `Code/Debugs` without running them against hardware here.

Final acceptance includes native build/package, real rendered UI checks without connecting instruments, and requirement-by-requirement evidence. Real hardware correctness remains separately staged and must not be claimed from offline tests. Preserve a progress ledger and explicitly identify any unverified hardware stage or incomplete deliverable; do not shrink the requested goal to passing libraries.
