# Rust Instrument Backend Migration Design

Status: the operator approved the scope and the Rust Host plus independent Rust worker architecture on 2026-10-06. This written design is awaiting review before implementation planning. The current App still uses the Python worker.

## Purpose and scope

Yang LAB INSTRUMENT CONSOLE must run without Python, Anaconda, pip, a private CPython payload or an interpreter setting. Device transports, drivers, setup logic and the worker execution layer will be Rust. Existing Rust Host services and the English HTML/CSS/JavaScript frontend remain; they are not rewritten merely to change language.

The operator's main concern is deployment reliability and environment maintenance. Responsiveness remains a requirement, but a language change alone is not a claim of faster instrument communication. The design replaces the whole Python backend, not only OSA and not a Rust wrapper that still invokes Python.

The migration includes all five current physical driver types: AQ6370-series OSA, eight-channel Voltage Source, Gain Chip Driver, PM400 and MDT693B. It also includes the two-stage Fiber Coupling Setup, resource enumeration and reservations, verification, configuration admission, scheduling, telemetry, interlocks, cleanup and capture staging. Existing Rust archives, exports, Host ownership, local IPC, encrypted remote connections and saved trust are preserved and adapted where their worker assumptions change.

The current Python driver implementation is a behavior and protocol reference during migration, not the future runtime. Keep existing source and Git history recoverable; do not delete measurement data or uninstall the computer's Anaconda. Legacy Python experiments and analysis scripts are outside this backend rewrite: they are retained as legacy references, not included in the App or claimed to have become Rust experiments. New live experiment execution will use the Rust driver/setup boundary; migrating an existing experimental procedure is a separate task.

## Architecture and responsibilities

The deployed chain is frontend to Rust GUI bridge to Rust Host to one independent Rust worker to installed VISA or Windows serial transports. Remote Apps still communicate only with the owning Host; a worker has no network listener and receives no direct GUI command.

The Host owns device configuration, verification policy, controller leases, operation history, event streams, durable storage and remote access. The worker owns physical sessions, driver transactions, safety monitors and temporary capture staging. The frontend owns presentation, drafts, navigation and honest waiting feedback, not hardware resources or control authority.

One worker per Host keeps all cooperating driver reservations in one process. Device operations are independent execution domains; shared serial ports and canonical VISA aliases cannot be opened twice. The existing machine-wide Host guard remains the outer ownership boundary. Native diagnostics must acquire an equivalent owner guard and refuse an existing hardware owner; neither a diagnostic nor an experiment may bypass the App's reservations by opening a raw resource.

These guards protect cooperating applications, not every third-party program or front-panel action. Use available OS/vendor exclusive-open checks and refuse a busy resource; do not steal ownership. Report externally changed state truthfully and invalidate motion authority when its assumptions no longer hold.

A single combined Host and driver process would remove a pipe boundary but put vendor-library stalls and faults inside the network and storage process. The selected independent worker preserves the current separation while removing the interpreter dependency. It does not provide per-device crash isolation, physical output protection after an OS crash, or a guarantee that a vendor call can be interrupted.

## Source layout

Use a root Cargo workspace and one committed workspace lockfile. Keep generated Cargo output outside driver directories. The following crate locations preserve the existing workspace hierarchy; Python files may coexist as migration references until retirement is documented.

| Location | Responsibility |
| --- | --- |
| `Code/Utils/Cargo.toml` and `Code/Utils/src` | Reusable `yang-drivers` library, common errors, typed status, VISA and serial transports, five drivers |
| `Code/Setups/Cargo.toml` and `Code/Setups/src` | `yang-setups` library, Fiber Coupling Setup and logical stage coordinates |
| `App/protocol/Cargo.toml` and `App/protocol/src` | GUI-independent `yang-protocol` library, worker messages and bounded result types |
| `App/worker-rs/Cargo.toml` and `App/worker-rs/src` | `yang-worker` executable, domain scheduler, verification, telemetry and capture staging |
| `App/src-tauri` | Existing GUI, Host, IPC, archive, remote transport and worker supervisor |
| `Code/Debugs/Cargo.toml` and `Code/Debugs/src/bin` | Native diagnostics and offline diagnostic regression tests |
| `Code/Experiments/<name>` | Future Rust experiment entry points, using typed drivers/setups through the owning execution boundary |
| `Result/<name>` | Generated measurements and diagnostic evidence |

Drivers do not depend on Tauri or write archives. Setups depend on drivers but contain laboratory-specific calibration and coordinate mapping. The protocol library does not depend on GUI code or load hardware. The worker composes these libraries; the Host never creates physical driver sessions.

Reuse dependencies already in the reviewed Rust lockfile where practical. Windows transport bindings can use the existing `windows-sys` dependency with the required native API features. The implementation plan must pin additional dependencies and establish the workspace lockfile before introducing them; do not use a floating crate version or a machine-specific build-cache path as a deployment requirement.

## Native transports and ownership

### VISA

Use a small Windows x64 C ABI boundary around the installed VISA library. Load the reviewed 64-bit library from its explicit system installation path, not the working directory or an arbitrary frontend-supplied DLL path. Verify function signatures and integer widths against the installed vendor headers. Keep unsafe operations inside the transport module, with bounds and ownership enforced by a safe Rust interface.

A manager lease and resource reservation must outlive every dependent device handle and outstanding call. Canonicalize aliases through VISA parsing before reservation; different resource names referring to one device are not independent sessions. Different resources may operate independently, subject to any shared physical-interface serialization required by the vendor. A transaction lock must prevent query replies from interleaving on one session.

Enumeration opens the resource manager but not instrument sessions and issues no instrument commands. Missing VISA must be reported as an unavailable dependency without preventing the Host from opening its GUI or managing serial-only devices. Installed vendor libraries are detected, not copied from this computer or replaced silently.

### Serial

Use Windows native COM discovery and serial I/O. Normalize equivalent COM names, retain USB VID/PID and transport serial evidence, and require explicit selected-resource or serial-number binding. Do not choose an instrument by enumeration order.

Transactions have bounded reads, writes, reply sizes and whole-operation deadlines. Handle partial reads/writes and unsolicited telemetry without treating an incomplete frame as a successful command. A serial handle and its buffers remain owned until all outstanding I/O is settled; requesting cancellation alone is not confirmed completion or resource release.

Transport configuration must preserve the reviewed device protocol, including baud rate, terminators, prompts and echo behavior. Do not infer one device's framing from another's serial adapter family. Gain communication remains 115200 baud. Voltage Source and MDT settings follow their existing reviewed driver/configuration references.

## Worker protocol and startup

Preserve the instance-addressed worker v3 request/action/result semantics and the existing public Host protocol. Requests retain their ID, method, typed parameters and session/domain/connection/epoch context. Configuration revisions, ownership nonces, request history and stale-context fencing remain mandatory. No raw SCPI or raw serial endpoint is introduced.

Retain newline-delimited JSON on the child's stdin/stdout; stdout is protocol-only and logs go to stderr. Keep the existing finite frame limits and strict validation of duplicate keys, numeric ranges, nesting and typed fields. Reject malformed or oversized frames without unbounded buffering; a broken reply channel after admission leaves the attempt uncertain rather than proving the instrument call failed. Spectrum payload delivery remains separately chunked as described below.

Startup identity changes from interpreter identity to a native startup identity contract with revision 1. It includes `worker_kind = rust`, worker executable identity, package revision, protocol version, real mode, session ID, disarmed activation state, `connected = false` and an empty domain map. The Host validates the expected shipped executable and package pairing; self-reported paths or hashes alone are not proof against replacement of the entire installation.

Resolve the packaged `yang-worker.exe` relative to the installed application resources. Source builds may use an explicit matching native development build location. Never search for Python, execute a source script, accept an arbitrary executable from the frontend, or fall back to an older backend. Public Host/network protocol versions remain unchanged because remote Apps do not select or launch workers. A mismatched worker startup identity is rejected rather than downgraded.

Before activation, record the child's actual OS process ID, creation identity and ownership nonce durably. Activation consumes the nonce once. Startup and activation open no instrument session, modify no output and restore no previous control lease. Restore configured records and authorized check policies, not live connections or piezo baselines. Any startup availability check uses the existing staged probe policy, never a normal output-affecting connect as a shortcut.

An incompatible or unfinished backend operation returns an explicit unsupported/unavailable result before hardware admission. Development milestones may expose only completed Rust capabilities, but incomplete devices are not silently routed to Python and the milestone is not called a complete migration.

## Scheduling and responsiveness

Port the current domain scheduler behavior, including independent ordinary, observation, safety and management capacity. Preserve the existing starting limits: 64 execution domains, 31 pending normal requests, 226 reply slots and 225 responsibility slots. Admission must reserve enough capacity to deliver outcomes and retain unfinished work; no unbounded thread, message or task growth is allowed.

Long-lived serial readers, device monitors and blocking vendor calls use bounded dedicated execution threads in the worker, not the GUI or Host async event loop. Preserve four ordinary and four observation execution slots, with reserved safety/management capacity. Safety admission fences queued conflicting work immediately; it does not issue competing bytes into an already active device transaction. A deadline on a vendor call does not prove the call has stopped. Tokio cannot forcibly abort an already running blocking task, so a generic async timeout cannot substitute for retained ownership. [Tokio blocking task documentation](https://docs.rs/tokio/1.53.1/tokio/task/fn.spawn_blocking.html)

Hardware calls, decoding and disk operations have separate measured timing fields. Keep the current immediate click acknowledgment, elapsed feedback, usable navigation, disabled conflicting actions, previous-capture labels and preservation of inputs/focus/scroll. Percentages are permitted only for measured work or received bytes. A stalled device must not freeze a Host status query, remote event stream or unrelated serial device. Synchronous VISA occupies its calling thread, which is why it belongs behind the worker execution boundary. [NI VISA threading explanation](https://www.ni.com/en/support/documentation/supplemental/18/choosing-between-synchronous-and-asynchronous-ni-visa-functions.html)

The worker retains the five terminal operation phases: `rejected_before_call`, `superseded_before_call`, `completed`, `completed_readback_failed` and `failed_after_call_started`. Host transport timeouts remain uncertain tracked attempts, not fabricated terminal failures. Reconnect, a later status reply or a worker restart must not replay an instrument action or complete a newer operation using stale evidence.

## Driver behavior and safety parity

Preserve the existing reusable driver's implemented public functions, not only the buttons presently exposed by the catalog. Before porting each driver, build a function/capability and regression matrix from its implementation, tests and reviewed instrument protocol. Tag every operation as read-only, output-affecting, maintenance, calibration or motion. A driver is not complete if a method is replaced with a no-op, a permissive generic command, a mislabeled unit or an unannounced unsupported result.

The App may continue to expose fewer functions than a driver. This migration is not a request to add every instrument setting to the GUI. Sensor-dependent and optional firmware functions keep their existing capability checks; protocol support, offline regression and physical validation are recorded separately.

| Driver or setup | Required behavior |
| --- | --- |
| OSA AQ6370 series | Connect/ordinary close preserve the panel; read the current trace without INIT, ABORt or settings writes; retain bounded explicit sweep support separately; do not abort panel-owned sweeps |
| Voltage Source | Eight-channel writes and continuous telemetry; 0 to 14 V; normal steps no larger than 0.1 V with at least 50 ms between them; handled failure and shutdown send every channel to zero immediately |
| Gain Chip Driver | 0 to 200 mA and target 15 to 40 degC; current enable requires TEC on and measured target within +/-0.2 degC continuously for five seconds; fresh temperature monitoring and interlocks; disable current before TEC on shutdown |
| PM400 | Typed measurements/settings and full existing sensor capability gates; preserve panel settings on normal connect/close; reset, zero/calibration, response and adapter changes require explicit operator authorization |
| MDT693B | Full existing read/write/settings/recovery API with project ceiling 75 V and 0.1 V/50 ms normal ramps; connect/close are read-only; fault stops and holds; no automatic zero, rollback or adoption of a motion baseline |
| Fiber Coupling Setup | Bind left/right by registered serials, support either side alone, apply laboratory coordinates and authorized nominal conversion, preserve session-only estimates and per-axis motion limits |

Fiber bindings remain left `2110148249-10`, right `160721175410`. Logical +X is right, +Y is away from the operator and +Z is up. Left logical X/Y/Z maps to MDT Y/X/Z; right maps to MDT X/Y/Z. Left +X and right -X are toward the chip. Each toward-chip move is at most 0.2 um; each other per-axis move is at most 1.0 um. Configuration can lower, not raise, these limits. MAX312D nominal conversion requires explicit authorization and is not measured displacement. External movement, authority loss or partial failure invalidates the estimate and requires explicit baseline adoption; one side cannot be inferred from a COM number.

Read-only diagnostics cannot call a normal Voltage/Gain connection that performs startup safety writes. Preserve separate identity/probe paths and their disclosed effects. Rewrite diagnostic entry points in Rust under `Code/Debugs`; enumeration, read-only connection and reversible output action remain three separately authorized stages. Migration approval itself does not authorize hardware testing or bypass driver limits.

## Capture and telemetry compatibility

OSA captures preserve the current native sample representation and metadata. Maximum trace size is 200001 points. Per instrument range is at most 1024 points, with at most 65536 reply bytes. Support strict ASCII and definite-length little-endian REAL,32/REAL,64 decoding as in the reviewed current driver. Reject truncated/oversized replies, malformed numbers, non-finite samples and unsupported interpretations before publication.

Retain exact native dBm or W values, wavelength conversion, trace identity, read interval and panel context before/after. Changed context invalidates the capture; unchanged context still has consistency `unproven`, not atomicity. Unsupported density/CALC/frequency interpretations remain unsupported until independently validated. Do not write format or display settings to make a read easier.

Use the existing interleaved little-endian f64 payload, descriptor schema 1, SHA256, point/byte count and bounded metadata. Rust staging retains at most 32 captures and 128 MiB; metadata is at most 8192 bytes and delivery chunks at most 16384 bytes. Keep the ownership nonce, guarded contained directories, reparse protection, immutable capture identities and acknowledgment only after durable import. A large spectrum is not one oversized control JSON frame. This rewrite does not add a shared-memory protocol or lossy sample conversion.

Retain Host durable archive and CSV export schemas so existing real captures and historical selections continue to load. Staging ownership, committed archive state, export completion and instrument read completion remain distinct outcomes. A storage failure after a successful read does not authorize another hardware read. Historical synthetic data, if present, stays historical and cannot become a real-device result.

Voltage/Gain/PM/fiber telemetry keeps its existing units, freshness, identity and quality fields. Loss of a monitor or old sample is not presented as healthy output. Do not change timestamp meaning or invent physical voltage-zero evidence during serialization.

## Cleanup and fault handling

Use explicit typed lifecycle states and idempotent, generation-fenced cleanup. Rust RAII releases uncomplicated local objects, but `Drop` is not a substitute for verified hardware shutdown. Do not claim release, current-off, TEC-off, piezo safety or physical zero merely because an object or process was dropped.

GUI closure releases that GUI's ownership through the Host; the Host may outlive all GUIs. A Host stop or worker input EOF stops admission, fences pending actions and performs ordered per-device cleanup. Voltage requests zero, Gain requests current-off then TEC-off, OSA respects sweep ownership, and MDT/fiber hold. Separate immutable reports record attempted actions, acknowledged effects, pending calls, handle release and process exit.

A timed-out or failed close retains its reservation, manager/module lifetime, buffers and cleanup obligation until a later explicit retry settles them. A late read cannot publish into a newer generation. Reopening a resource, unloading its DLL or starting a replacement worker while responsibility is retained is forbidden. Do not force-kill a worker that may own active instruments merely to make an App close quickly.

An unexpected worker crash makes affected resource/output state uncertain, revokes command authority and requires explicit recovery. Persisted receipts can establish only what the prior attempt actually confirmed. After a Host crash, a surviving worker attempts its EOF policy; a replacement Host must reconcile recorded ownership before activation. Neither process isolation nor Rust can guarantee shutdown commands after power loss, kernel failure or complete worker failure; hardware failsafes remain external requirements.

## Configuration and installation

Remove Python path inputs, `--python`, interpreter discovery, Python environment validation and source-worker launch from production entry points. Migrate saved settings through a new typed registry/preferences version. Preserve device IDs, addresses, identities, configuration revisions, recordings, remote pins, authorization and Host names. Back up old configuration before conversion; obsolete Python fields are discarded by the migration, never executed. Corrupt or unsupported versions produce a recoverable configuration error, not an empty overwrite.

An upgrade must not restart an existing hardware owner, reconnect instruments, restore current/voltage, or adopt stage baselines. Block replacement of running executables and request ordinary verified shutdown when necessary. Keep real-only operation; test doubles remain injectable offline test boundaries, not a user-selectable backend.

The Windows x64 installer includes the GUI, Host, Rust worker, approved catalogs/assets and native runtime prerequisites. It includes no Python source worker, interpreter, wheels, pip, environment YAML or Python bootstrap. Users do not need Rust, Node or build tools. WebView2 and applicable vendor VISA/GPIB/CH340/CP210x dependencies remain separately detected prerequisites; do not silently install vendor software, accept its license or alter a system VISA backend.

Source developers use the Cargo workspace/lockfile and documented Windows native build prerequisites; frontend tests use Node. Rust diagnostics and backend CI must run without Python. Keep historical requirements/environment manifests only with an explicit legacy label while referenced code remains. Replace the active AGENTS/development instructions at cutover so a later agent cannot reintroduce an Anaconda launch requirement or accidentally run a retired experiment.

## Milestones and acceptance

Use small reviewed increments on `codex/pic-desktop`, retaining current runnable App and source until a Rust candidate is validated. This design does not authorize a main merge, a 1060 desktop branch rewrite or a GitHub publication step. Shared protocol compatibility must be maintained for the other computer's ongoing development.

1. **Native foundation and worker lifecycle.** Establish the workspace, pure protocol/driver boundaries, native transports, reservation/lifecycle machinery and a hardware-free Rust worker. Port scheduler/context/fault tests using bounded Rust transport doubles and fixed wire fixtures. No real resource is opened.
2. **OSA vertical migration.** Implement the complete existing OSA driver and Rust capture staging; connect the Rust worker to the existing Host/archive/page. Validate exact fixture bytes, maximum trace bounds, stalled reads, late replies and cleanup. Then perform separately authorized identity and existing-trace checks on the known OSA. Explicit sweep diagnostics remain separate.
3. **Remaining driver parity.** Port Voltage, Gain, PM400, MDT and Fiber Setup with their individual public-function/regression matrices. Preserve omitted GUI functions at the library boundary. Authorize and record each real-device stage separately; an absent PM400 or stage blocks its physical acceptance claim, not unrelated offline development.
4. **Native cutover and deployment.** Remove production Python launch and payload assumptions, migrate settings, rebuild native diagnostics/package scripts, and retire Python backend CI. Install and exercise the exact candidate on a genuine Windows machine or fresh VM without Python/Anaconda. Then qualify known devices and remote Apps without erasing earlier unvalidated capabilities.

For each increment, first run focused offline regression for its changed behavior and the shared protocol/lifecycle suite. Run the full native/frontend offline suites at worker cutover and before packaging. Reuse independently checked protocol fixtures and existing safety cases; do not execute a parallel Python backend to generate expected answers during Rust tests. Record actual stage durations to find bottlenecks instead of promising a universal speedup.

Final backend completion requires all of the following:

- All current implemented driver/setup functions are accounted for by supported Rust behavior and capability tests, with no hidden Python fallback.
- The packaged App, local Host, native diagnostics and backend regression entry points operate without Python or Anaconda, including on paths containing spaces/Unicode and after restart.
- Missing vendor dependencies yield clear usable App status; package launch does not open an instrument or change outputs.
- Existing configurations, real/historical archives, native export, paired trust and two-App synchronized status/capture behavior survive migration.
- Delayed/failed reads, saturated queues, external ownership, monitor failure, navigation, disconnect races and incomplete cleanup retain responsive UI and truthful outcomes.
- Physical validation is recorded per device and action. Offline support alone does not establish real-hardware parity; real two-computer/Tailscale validation remains distinct from loopback validation.

## Superseded deployment direction

The app-private CPython deployment direction in `2026-10-06-private-runtime-design.md` and its implementation plan is superseded by this approved architecture choice. Preserve its completed build tooling and historical evidence, but do not spend further work integrating or shipping that runtime. Safety, isolation, configuration preservation and clean-machine acceptance requirements carry over to the Rust deployment.

This design's review approves the migration contract. The subsequent implementation plan will define task-sized changes and verification commands; product implementation starts only after that plan is reviewed and an execution method is selected.
