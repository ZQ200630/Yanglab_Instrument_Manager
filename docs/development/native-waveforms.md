# Native waveform workbench

The scope reader, experiment runner and analysis workbench use Rust throughout. Instrument-specific protocols stay in drivers; saved waveform data, analysis and exports remain usable without an instrument connection or Python installation.

## Layers

| Layer | Responsibility | Location |
|---|---|---|
| Family driver | Identity, capabilities, reviewed model commands, calibration, bounds and resource lifecycle | `Code/Utils/src/scope`, `Code/Utils/src/generator` |
| Data | Record → Frame → Channel, immutable typed raw samples, atomic metadata, legacy import/export, versioned Q/FSR | `Code/Data` |
| Experiment | Versioned templates, explicit roles, preflight, steps, partial results and cleanup policy | `Code/Experiments` |
| Worker and Host | Native sessions, control ownership, operation journal, capture transfer, durable frame commit, bounded jobs | `App/worker-rs`, `App/src-tauri/src/host` |
| App | Direct instrument controls, experiment configuration, history, ROI and background-job feedback | `App/web` |

Add a new model through a reviewed family implementation and capability declaration, not a new analysis pipeline. Model-specific direct controls may differ. An experiment calls the family core through the Host adapter and cannot open serial/VISA resources itself.

## Supported instruments

Scopes: Rigol DS7054 and MHO5054. Generators: Rigol DG4162, DG4202 and DG922 Pro. Device Setup selects the concrete model and an explicit compatible system-VISA resource. Enumeration is not identity verification. Unknown instruments remain unsupported rather than receiving guessed commands.

Scope core includes identity/status, channel display, Run/Stop, memory-depth choices and Screen/Memory transfer. DS7054 preserves unsigned BYTE samples; MHO5054 preserves unsigned little-endian WORD samples. Channel calibration supplies its own time and voltage axes. Optional physical memory licenses/channel topology can restrict what a real instrument accepts.

Generator core includes two-channel signal/output readback, waveform/frequency/Vpp/offset configuration and explicit output enable. Phase, load, polarity and amplitude-unit operations are capability-gated. Triangle uses the reviewed ramp command with 50% symmetry. Voltage peaks, load divider and frequency-dependent limits are validated together; enabling output is separate from configuring the signal.

Identity connection and normal close do not reset scope/generator settings or turn outputs off. They are not a physical safety guarantee. Experiment cleanup explicitly disables only generator outputs owned by that run, before final analysis/storage work. Other instruments retain their existing driver-specific safety policies.

## Using the App

1. Add the concrete instrument in Device Setup and complete the separate identity check. Connect from its instrument page.
2. On a scope page, the four colored channel buttons reflect the last synchronized instrument display state; clicking one applies the change and reads the settings back automatically. Save checkboxes select enabled channels for storage without changing the instrument. Sync pulls the instrument settings into the controls. Connection and page rendering never apply default settings. Memory is the default transfer source, with a 100K depth preset until instrument readback is available; actual readback takes precedence. Auto depth is shown truthfully and requires choosing a supported numeric depth before Memory acquisition; Screen transfer remains available. Selecting another depth applies it and reads it back automatically. A changed depth on a stopped DS7054 explicitly refreshes its frozen buffer through temporary RUN, then confirms STOP and final depth; an unchanged depth never does so. Depth application has a bounded 30 s native confirmation budget, separate from the per-I/O stall timeout, the owned STOP restoration budget and a fresh follow-up status-read budget. A timeout never replays the setting; retained channel/depth values are labeled previous, not current.
3. Acquire sends STOP, confirms that the scope is stopped, and transfers the frozen buffer into a temporary native-memory preview. It does not save a Record or RunRecord, request SINGLE, or claim that a new trigger occurred. Advanced offers frame count, interval, Screen transfer and an optional final Start (Resume after transfer is on by default for new captures; explicit choices are preserved). For multiple manual frames the sequence is STOP → transfer → verified preview → RUN → interval → STOP → transfer; the final frame remains stopped unless Resume after transfer is selected. Unsaved previews are owner-scoped, capped at 512 MiB aggregate / 32 previews, and lost on Host exit. Only Save record commits the stable preview ID and requested name atomically to the record library; repeated saves update labels without duplicating samples. Export Data can export an unsaved preview without saving it. Replacing the visible preview releases its temporary memory after the replacement loads successfully; previously saved Records remain intact. Cancellation or failed transfer never starts the next interval. Existing saved definitions with omitted acquisition mode and Q/FSR experiments retain their explicit SINGLE-based fresh-acquisition semantics; automated experiments still commit durably before acknowledging Worker staging.
4. Drag a colored horizontal ROI on Scope, Record/Data or Analysis to zoom into calibrated time. Right-click steps back through the zoom history. Each channel maps the shared time window through its own calibration; hiding a channel does not redefine the time axis. Display reduction preserves extrema/tail indices but never changes original samples or analysis inputs.
5. Record/Data is the saved collection: waveform Records and sealed OSA spectrum archives use typed adapters over their existing stores. It provides browsing, frame/channel selection, labels, waveform deletion, compatible Analyze links and exports. OSA archives retain their sealed evidence and immutable labels. Analysis consumes compatible saved waveforms independently of live instruments; known experiment or saved-analysis calibration prefills parameters, unknown manual calibration stays blank, and operator edits take precedence.
6. Export `.db` for typed raw round-trip. CSV/NPZ exports retain calibrated columns/arrays and include a metadata/analysis sidecar. Export selected frame or Record through the native folder picker. Existing files are never silently overwritten; per-file failures remain visible. Record/Data Import appends legacy `.db`, CSV and NPZ without replacing the saved collection. The legacy replacement API remains available only with explicit confirmation and recoverable trash; it is not offered as the normal import action.

Import rejects object/pickle arrays, hostile ZIP paths, ambiguous shapes, unbounded payloads and invalid values. Actual NPY dtype is authoritative; legacy metadata conflicts are reported. Imported history never acquires hardware authority.

## Experiments

Templates are Waveform Capture, Q Measurement and FSR Measurement. A saved Experiment is a versioned configuration; a Run is its immutable parameter/device snapshot, step evidence, record references and outcome. Q/FSR bind Laser, Scan Signal and Detector explicitly by Host/device/channel. Laser roles use the native TLB controller; templates do not automatically enable laser emission.

All roles must currently belong to one owning Host. Mixed-Host composition is rejected before instrument changes. Parameters and readiness are checked before actuation. Reservations share authority with manual control and prevent conflicting commands. Coarse tuning is optional. Generator output OFF, configuration and explicit ON precede fresh captures; owned output OFF occurs before expensive final processing.

Cancel stops future steps and waits for the actual driver/transfer boundary. The Host retains connections while run-owned cleanup is pending. Timeout, process interruption or an unknown command outcome never causes automatic replay. Partial completed frames remain historical records. No software path can guarantee physical output shutdown after machine power loss or an unreachable instrument.

## Analysis definitions

Single Drop fits an inverted Lorentzian. Q uses positive fitted FWHM and the explicit scan calibration `2 × ramp_hz × excursion_pm`; the wavelength and requested ROI are saved with the result. Fit diagnostics, algorithm version and residual statistics are retained. The native fit uses an explicit bounded original-index fit policy, not displayed min/max curves.

FSR detects dips using prominence and minimum-distance settings, fits a regular contiguous subset and maps spacing through the supplied scan speed. Results retain dip markers, regression diagnostics and their algorithm version. Native algorithms are not represented as bit-for-bit SciPy equivalents. Flat/short data, invalid calibration and failed fits return errors, not NaN or invented successful values.

## Historical data sharing

Trusted paired Lab computers may browse, analyze, export, rename/comment and delete saved waveform records over the authenticated pinned-TLS connection. Remote history does not require live instrument control. Requests use bounded result/status/background/safety channels; export transfers use opaque file IDs and checked chunks, not remote filesystem paths.

Remote file import and remote experiment start/stop/save remain disabled pending their separate authorization. Pairing alone does not expand that scope. Cross-Host experiment roles are independently unsupported as described above.

## Limits and development checks

Capture/run raw input budget is 512 MiB; frame count is 1–9999, channel size up to 250M points when supported, and normal waveform viewports are at most 8192 points/channel. Native transfer chunks are at most 16 KiB. The workbench has two workers, a 16-item queue and bounded history. Large valid acquisitions can require substantial memory/disk; failures and cancellation must be reported rather than silently reducing requested samples.

Run `App/scripts/test-native.ps1 -Offline` with the native Rust/Node toolchain. It builds the actual Worker and private finite diagnostic fixtures, runs Rust and frontend regressions, and checks packaging without Python/conda on PATH. Transcript fixtures are development-only and must not ship as selectable devices.

Offline coverage is not real-device qualification. Stage physical diagnostics separately: enumeration, read-only identity/connection, then explicitly approved reversible acquisition/output actions. `yang-debug` scope/generator preview/read-only support never performs an action merely by previewing a request. Current diagnostics intentionally reject scope/generator action-stage execution; use the reviewed App workflow after the applicable physical authorization.
