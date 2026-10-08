# Native backend qualification

## Current status

The production chain is Rust: five instrument driver libraries, Fiber Setup,
the independent Worker, Host, archive/export, local/remote communication and
native diagnostic CLI. The Tauri GUI remains English HTML/CSS/JavaScript.
No production interpreter search, source-worker launch or Python fallback is
available. Historical Python code and experiments remain reference-only.

The candidate is **implemented and offline-tested, not deployment-qualified**.
Nothing in this migration replaces the currently installed App or authorizes
a new hardware test, vendor installation, merge or push.

| Stage | Evidence on 2026-10-07 | Status |
| --- | --- | --- |
| Native workspace | 437 Rust tests; no ignored tests | Passed |
| Frontend | 275 Node tests; no skipped tests; six actual native-status to UI/resume cases | Passed |
| Packaging regressions | Six PowerShell tests, including Unicode paths, environment restoration and active-owner refusal | Passed |
| Native release | Three x64 MSVC executables; static CRT; no Python/VCRUNTIME DLL imports | Built |
| Portable payload | Seven-file allowlist, manifest/resource hashes, no Python or diagnostic fixtures | Passed |
| NSIS | Candidate compiled using guarded, pinned template; no installer executed | Built; installation pending |
| Formatting | Migration-owned files formatted; full `cargo fmt` still reports legacy untouched files and three pre-existing user-dirty files | Scope passed; whole-tree not passed |
| Genuine no-Python Windows | No fresh Windows computer/VM available for this run | Pending |
| Real instruments | No native enumeration, identity or action stage executed | Pending |
| Two installed Apps / actual two-PC Tailscale | Offline native flow and actual TLS finite-peer tests passed; no interactive installed-App/two-PC run | Pending |
| Installed-App migration / restart / driver cleanup | Covered by offline fixtures only | Physical/installation acceptance pending |

The old Python-backed physical OSA tests are historical, not evidence that the
rewritten Rust driver works on a real AQ6370. Removing Python from this
development shell's PATH is not a clean-Windows qualification.

The final-review candidate is machine-local at
`tmp/native-candidate-final2-20261007`. Its `qualification.json` records
source/package identity, protocol 3, startup revision 1 and SHA256 of the GUI,
Host, Worker and installer. It records `installed=false`,
`clean_windows_validated=false` and `physical_validated=false`. Formatting or
review fixes require a new candidate directory and new hashes. Use the exact
latest candidate and its qualification report for acceptance; do not mix files
from builds or credit an older package to a changed source tree.

The final whole-branch review identified six important issues. The one fix pass
covers cooperative safety cancellation, original attempt status and Resume,
delayed serial replies, diagnostic orphan ownership, storage-only OSA recovery,
and no-follow/pinned settings backup and migration. Regression tests reproduce
the failures; the native, frontend and packaging suites are the offline gate.

Saving failures preserve the original OSA samples, read timing and context.
The App offers only a failure-specific **Retry save**, which performs no new
instrument read and grants no control lease, even after disconnect. Repeated
disk failures reuse owned staging files instead of exhausting capture slots.
On EOF, native handles close first; if saving fails the Worker retains data
responsibility. After storage recovers it can leave unacknowledged native bytes
and metadata on disk before exiting. Those files are **not** a completed Host
archive and require operator recovery. RAM-only data is not protected from an
abrupt process termination or power failure before durable staging.

Machine-local evidence belongs in a new
`Result/console/native-<run>` qualification directory, never Git. Keep separate
report fields for offline, installer, dependency, startup, identity, action,
archive/export, cleanup, restart, loopback and actual two-PC outcomes.

## Offline gate

Prepare x64 MSVC, Rust and Node as described in `../development.md`. No Python
command or package manager is used. Release any existing live instrument owner
through ordinary verified shutdown; do not kill it to make a test pass.

```powershell
./App/scripts/test-native.ps1 -Offline
pwsh -NoProfile -File App/scripts/tests/native.Tests.ps1
cargo fmt --all -- --check
```

`test-native.ps1` builds the native finite pipe fixture and diagnostics, runs
the complete workspace sequentially, then all frontend tests. It removes
Python/Anaconda/WindowsApps aliases from the child test PATH. Test adapters are
not selectable by production arguments and never packaged. Also run the
plan's `cargo test --offline --locked --workspace --all-targets --features
sil-instrument-console/host-bin -- --test-threads=1` before final qualification.

Run `check-package.ps1` on the candidate portable directory and compare all
three executable hashes to its qualification report. Inspect native PE imports
for interpreter/CRT prerequisites. The check validates the portable payload;
it does not claim that an unexecuted installer was installed or that a vendor
driver is healthy.

## Genuine clean Windows installation

Use a fresh Windows x64 VM or another real computer with **no Python, Anaconda,
Rust or Node** installed. Record Windows version, ordinary-user account, exact
installer hash and prerequisites. Install Microsoft WebView2 separately if
missing; the candidate does not silently install it, VISA or USB/serial vendor
drivers or accept their licenses.

1. Close the installed GUI, Host and Worker normally, retaining cleanup reports.
   Confirm no owner remains. The installer must refuse a live owner rather than
   kill or restart it. Do not deliberately run this check with active outputs.
2. Install the exact candidate as an ordinary user in a new empty directory.
   Repeat with spaces and Unicode in install/data paths. Never overwrite a
   historical Python installation or machine-local data as a test shortcut.
3. Start the App. Local Host startup should be automatic, empty and disarmed:
   no instrument port opened and no output change. Check Overview, Settings,
   Add New and truthful missing-VISA status. Serial and archive functionality
   must not require a VISA library just to start the App.
4. Check that the Worker is the packaged `yang-worker.exe`, with matching
   protocol/startup/package identity and actual process identity. There must
   be no Python subprocess, interpreter field, simulation fallback or source
   launch. Retain startup errors if a prerequisite is absent.
5. Exercise saved device/setup IDs, names, revisions, policies, Host data root
   and existing DPAPI-protected trust under the same Windows account. Confirm
   migrated original bytes were backed up before replacement. Future/corrupt
   records must remain intact and produce an actionable error.
6. Restart the GUI and then the Host normally. Configuration/trust must survive;
   stale identity proofs, control leases and stage baselines must not. Close
   again and retain actual Worker/Host exit and cleanup evidence separately.

Do not mark the clean-machine stage passed when only source builds or a
PATH-filtered developer computer have been tested.

## Real-device staging

Every instrument proceeds through independently approved enumeration,
read-only connection and an explicitly approved reversible action. A former
Python-driver trial or earlier output approval does not cover this native trial.
Do not touch unknown USB serials `DS7A241400216`, `MY52091763`, or the network
resource `10.229.158.143`.

Use `Code/Debugs` native diagnostics and the strict catalog DomainConfig;
experiment code never opens VISA/serial directly. CLI execution requires exact
`--execute --confirm-stage <stage>` acknowledgement. Without those flags it
prints a preview and opens nothing. Each CLI stage writes a **new absolute
`Result/<short-name>`** directory; the qualification index references those
immutable reports. No report is overwritten and no failed action is replayed.

For AQ6370, separately authorize enumeration, then read-only identity on
`GPIB0::4::INSTR`, then reading an **existing trace**. Compare the returned panel
context, sample counts, native units, raw little-endian f64 archive bytes,
SHA256 and CSV round-trip values. This stage must not issue INIT, ABORt or
front-panel setting writes. Sweep acquisition is another separately approved
action, not an automatic way to replace a failed trace read.

For other devices, disclose the following before requesting an action stage:

| Device | Required action disclosure / invariants |
| --- | --- |
| Voltage Source | Normal startup/close zeros all eight channels. Commands 0–14 V, normal steps <=0.1 V and >=50 ms; handled faults/shutdown send immediate all-zero. Read-only probe uses a distinct nonzeroing lifecycle. |
| Gain Chip Driver | Normal startup/close disables current before TEC. Current <=200 mA, target 15–40 degC; enabling requires TEC on and continuous fresh temperature within +/-0.2 degC for five seconds. Watchdog/fault cleanup remains enabled. |
| PM400 | Connect/normal close preserves panel settings. Sensor capabilities gate all dependent writes. Reset, optical zero/calibration, detector response and adapter changes need explicit action confirmation. |
| MDT693B | Read-only connect/recover/normal close holds outputs. Ceiling 75 V, steps <=0.1 V at >=50 ms. Faults stop and hold, never automatically zero. Motion needs explicit baseline authority; zero is separately confirmed. |
| Fiber Setup | Exact left/right serial binding, lab coordinates and explicit nominal MAX312D scale. Session-only estimate; toward-chip <=0.2 um, other per-axis <=1 um. Lost authority/partial failure invalidates estimate and holds both stages, never rollback/zero. |

Gain/Voltage action diagnostics additionally require
`--acknowledge-lifecycle`. If identity, output state, cleanup or release is
uncertain, record failure/unknown; do not call it a pass. An absent instrument
remains physically unvalidated. Handle release, ordered safety attempts and
host-observed zero are different claims; none is an independent physical
measurement.

## Local/remote Apps

First run two native Apps on one PC, with real Host ownership kept separate from
offline fixtures. Then repeat on **two physical computers** using the lab's
Tailscale network; label loopback and actual two-PC results separately.

Approve first-use trust once in Settings. A trusted peer reconnects after App
and Host restart without another approval. Local and connected remote devices
coexist, and the instrument's owning Host remains the only process doing I/O.
Read OSA remotely and confirm both Apps observe the same capture/operation ID,
native bytes and archive. Check navigation, drafts, previous capture, elapsed
pending feedback and export during a wait; never fabricate progress or replay
a timed-out action. Test disconnect and trust revocation, ordinary driver-led
cleanup and truthful retained-owner state before any replacement.

Only after these pending stages have actual evidence may the candidate be
called deployment-qualified. Integration/publication is a separate operator
decision; this document does not authorize main merge or GitHub upload.

## Rulings I made

The complete ordered implementation decisions and their costs are preserved
below from this plan's ledger. No deferred minor review findings were reported.

1. Ruling: skill bookkeeping helpers are Bash scripts and Bash is unavailable — preserve the same ignored plan-scoped ledger, read task briefs with PowerShell and record fresh test results beside commits — cost if wrong: bookkeeping differs, not product behavior.

2. Ruling: avoid legacy native process baseline tests while preserving the running Host — frontend baseline and new crate suites now; full converted native suites at cutover after ordinary owner release — cost if wrong: pre-existing native failures may surface later and will be reported.

3. Ruling: no legacy Python dependency reinstall at setup — approved migration eliminates Python and leaves legacy reference intact — cost if wrong: legacy tests are not newly qualified.

4. Task 4: Ruling: include the ASRL/COM reservation alias fix in this lifecycle increment — reading the legacy ResourceClaims showed both address the same Windows port and shared safety ownership requires one key — cost if wrong: an unusual ASRL mapping may be conservatively refused; no resource is opened to infer it.

5. Task 5: Ruling: Backend explicitly provides its shared DomainRegistry in addition to execute/observe — the approved constructor has no registry argument, and a scheduler must use the worker's known session/domain authority rather than infer it from incoming requests — cost if wrong: Rust adapter implementers must implement one extra metadata method; public wire protocol is unchanged.

6. Task 5: Ruling: add a bounded metadata-only Backend::request_stop hook and wait for per-domain native transaction/callback quiescence before safety I/O — cancellation/interlocks must reach a held driver without byte interleaving or pretending unfinished native calls released resources — cost if wrong: output drivers must implement the cooperative stop hook; unresponsive vendor calls still retain responsibility until their finite timeout/completion.

7. Task 6: Ruling: reuse the reviewed Rust Host catalog code inside the native worker with error/path adaptation — preserves the current field/profile validator without changing the running Host; catalog extraction/shared deduplication can occur at cutover — cost if wrong: both validators must stay synchronized until shared extraction.

8. Task 6: Ruling: ProbePort supplies registry and metadata-only supervised_snapshot, with no implicit normal connection — constructor otherwise cannot bind proof to actual worker session, and output-affecting device verification requires independent controlled-session authority — cost if wrong: native adapters must supply these metadata hooks; unavailable supervised sessions remain explicitly blocked rather than auto-opened.

9. Task 8: Ruling: add complete-reply VISA framing and an owned-error opening result during OSA integration — earlier fragment API discarded VISA EOI/count status and an error with a nonzero handle otherwise lost local retry ownership; keep fragment reads available for other consumers — cost if wrong: native adapters implement the additional local termination-attribute hook; no instrument settings writes are introduced.

10. Task 8: Ruling: one explicit acquisition starts one sweep, without Python's automatic replacement-sweep retries; operation faults retain the session until explicit cleanup — approved no-replay/uncertain-outcome contract takes precedence over legacy retry convenience — cost if wrong: an operator must close/reconnect and explicitly retry after a failed acquisition; this avoids silently replacing a measurement.

11. Task 9: Ruling: bounded 256-entry ACK receipt history rather than unlimited tombstones — duplicate completed acknowledgements are idempotent while their receipts are retained; unknown/aged receipts reject instead of authorizing deletion — cost if wrong: an extremely delayed duplicate ACK after 256 newer receipts needs Host reconciliation, not indefinite worker memory.

12. Task 9: Ruling: reuse guarded archive helper pattern in a worker-only file module, and add a durability boundary that cannot replace guarded opens/writes — keeps the live Host production path unchanged while testing disk commit failure — cost if wrong: helper maintenance is duplicated until later shared extraction.

13. Task 10: Ruling: add explicit consuming VisaManager::release with retry-owned errors and Backend auxiliary-cleanup hooks — dropping an Arc is not proof of native resource-manager release; the independent process must truthfully retain both instrument and pool responsibilities — cost if wrong: native shutdown may remain pending until vendor completion instead of falsely reporting exit readiness.

14. Task 10: Ruling: bind a single guarded spool through a metadata-only Backend hook and reserve one of 32 capture obligations before instrument I/O — approved Worker constructor otherwise does not let NativeBackend stage owned data, and capacity failure must not silently repeat or discard a measurement — cost if wrong: conservative worst-case admission can refuse a read earlier than actual disk exhaustion.

15. Task 10: Ruling: continue shutdown by retrying retained cleanup only, never measurements/connections, and emit the explicit shutdown terminal only after the first actual cleanup attempt — immutable failure evidence and uncertain writes remain truthful while the process retains responsibility — cost if wrong: an unresponsive native call can keep the worker alive; the Host must display retained cleanup rather than force-kill it.

16. Task 11: Ruling: retain the existing v2 lifecycle/broker coverage only behind cfg(test), with a native finite pipe peer; production RuntimeConfig/spawn is protocol-3-only — converting those temporal regression assertions directly to device domains would mix protocol redesign with process lifecycle migration, while no obsolete worker command may remain registered or linked in the shipped App — cost if wrong: test-only v2 adapter must remain clearly isolated and packaging must exclude the diagnostic fixture.

17. Task 11: Ruling: use OS-pinned fixed package image plus manifest digest and actual process image/PID/creation time, not worker self-report alone — native replacement must not pass merely by echoing a requested executable path, and no Python/PATH selection is allowed — cost if wrong: installers must wait for the Host to close before replacing a pinned binary; whole-installation compromise remains outside this trust boundary.

18. Task 11: Ruling: normalize native immutable shutdown receipts into the existing Host cleanup envelope while retaining the original receipt under native_cleanup — preserve public Host protocol and distinguish process exit/resource release/physical-zero evidence — cost if wrong: malformed/conflicting cleanup evidence leaves the runtime retained rather than permitting replacement.

19. Task 11: Ruling: obsolete interpreter arguments in test-only legacy adapters are ignored, not validated as potential executables — the shipped supervisor cannot accept them at all, and migration compatibility must never execute or search them — cost if wrong: obsolete source-only tests no longer predict old interpreter-validation errors.

20. Task 11: Ruling: perform pure pinned-package preflight before ownership intent — a missing package should not create an unresolved child record when no process was ever spawned — cost if wrong: package rejection takes precedence over a later ownership/configuration error.

21. Task 12: Ruling: extend WorkerVerificationPort proof extraction and native session observation outside the listed Host files — native proof nesting and cached connected/resource/identity fields must match their actual consumers — cost if wrong: these compatibility fields must remain consistent with future driver adapters; no new hardware polling is implied.

22. Task 12: Ruling: explicit disconnected OSA Connect performs a fresh read-only identity probe, compares all saved identity fields, consumes the private proof, then connects under the same lease/context and original operation — saved metadata cannot authorize a restarted native worker, and startup must never open instruments — cost if wrong: Connect adds one finite identity-query/open-close cycle; changed firmware requires explicit operator reconfiguration. Probe/configuration requests are serialized, deadlines retain uncertain requests and never replay.

23. Task 12: Ruling: retain up to two immutable verified presentation pins across navigation, always labeled previous and invalidated by Host boot/context changes — current result hydration remains scoped, while a pending read must not blank the existing plot — cost if wrong: up to two bounded sample arrays remain in UI memory; the cache confers no hardware authority.

24. Task 12: Ruling: move retired v2 interpreter API/settings renderer into test-only modules instead of deleting temporal regression coverage — production frontend has only native Host routes while existing v2 fixtures remain meaningful source-only references — cost if wrong: obsolete fixture tests are not native acceptance evidence and packaging must exclude them.

25. Task 12: Ruling: nest cfg(test) native_osa_flow_tests below Host service to exercise private operation/lease/archive boundaries — avoids adding production test constructors or bypass routes — cost if wrong: integration tests remain coupled to internal Host construction, not public UI behavior; mounted tests independently cover UI.

26. Task 13: Ruling: add SerialOpenFailure with a returned live session for configuration failures — a voltage driver must retain and explicitly close its own port rather than losing it into a global transport fallback — cost if wrong: native serial users have an additional owned-error API; existing ordinary factory API keeps its behavior.

27. Task 13: Ruling: one I/O-owning reader actor with bounded ordinary/safety queues and atomic cleanup intent — continuous reads and writes cannot interleave bytes or destroy an overlapped read; finite caller deadlines retain the same job — cost if wrong: a held native read delays safety bytes until it actually settles, while the GUI/other domains remain responsive and resource responsibility stays visible.

28. Task 13: Ruling: quantized DAC ramp steps use at most floor(0.1*65535/28) codes and enforce spacing across separate ordinary operations — nominal 0.1 V floating steps can exceed the ceiling after quantization — cost if wrong: a 0→14 V ramp may need one extra step, not larger steps.

29. Task 13: Ruling: never replay a failed nonzero frame; native partial-write completion remains bounded, and failures request a distinct safety zero — the global no-replay policy overrides Python's whole-frame retry convenience — cost if wrong: a transient write failure requires explicit cleanup/retry rather than an invisible automatic retry.

30. Task 13: Ruling: stream alignment observes 102 bytes and uniquely plausible consecutive 34-byte frames; embedded/ambiguous payload markers never fabricate ADC channels or zero — the protocol has no CRC, measurement ID or unique identity; board-scale plausibility does not impose the lower command ceiling on external telemetry — cost if wrong: initial/recovery validation waits roughly three frames and rejects genuinely ambiguous data until it changes. This remains weak host-observed protocol evidence, not physical voltage proof.

31. Task 13: Ruling: track the first-byte command generation of fragmented telemetry and block normal writes while stale/recovering — post-write receipt alone cannot make a pre-write fragment new measurement evidence — cost if wrong: conservative refusal/waiting may require an explicit new operation after stream recovery.

32. Task 13: Ruling: keep a stranded owned SerialSession slot through reader spawn/panic failure, and retry cleanup-only from that slot — moving the sole owner into a thread closure before successful spawn could lose driver-local cleanup control — cost if wrong: a poisoned/unresponsive native operation can remain conservatively retained; thread-spawn exhaustion itself has not been induced in tests.

33. Task 13: Ruling: recovery I/O backoff uses finite wall-clock instants and checks safety/close intent every millisecond; safety freshness and failure authority still use the injected monotonic clock — polling an empty serial stream should not spin or delay a safety deadline by a long configured retry interval — cost if wrong: test clocks do not accelerate the backoff; recovery tests may wait the actual configured interval.

34. Task 14: Ruling: a single bounded Gain I/O actor owns request/reply, watchdog and shutdown; an atomic close intent preempts queues while retaining unfinished native reads — preserve byte ordering and metadata responsiveness, as required by the Rust architecture — cost if wrong: a held vendor/native call delays physical shutdown bytes; caller deadlines report retained responsibility, never a force-kill or confirmed-off claim.

35. Task 14: Ruling: monitor each one-second tick with all five typed status reads, not temperature alone; the first invalid monitor exchange latches FAULT and requests ordered Q=0 then D=0 instead of allowing three failed exchanges — live TEC/current/manual-target observations and an ambiguous serial stream must not preserve enable authority — cost if wrong: five finite queries add serial traffic and a transient protocol error requires explicit cleanup/reconnect earlier than the Python driver. Reviewed >1 degC/three consecutive samples and >3 degC/severe thresholds remain unchanged for valid samples.

36. Task 14: Ruling: fence SerialSession after a timed-out/incomplete Gain reply; still record both ordered shutdown attempts, but never bypass a quarantined transport to reinterpret late bytes as ACKs — whole-frame retries could associate another operation's old READY reply with new current/TEC state — cost if wrong: shutdown effects remain unknown when the broken transport cannot send, even if handle release succeeds; physical fail-safe needs hardware protection beyond software.

37. Task 14: Ruling: use Deadline::earlier to intersect a compound-operation budget with each packet's I/O budget — neither fragmented reads nor PID/status sequences may multiply one packet's native timeout — cost if wrong: a genuinely slow reply is rejected at the configured per-packet timeout, not tolerated for the entire operation budget.

38. Task 14: Ruling: repeated enable observes an already enabled output without sending Q=1; only an off-to-on request uses fresh six-sample authority and subsequent actual RDCA — Q=1 resets the hardware setpoint to 3 mA — cost if wrong: the already-on check performs an extra read-only snapshot; it never changes or restores current.

39. Task 14: Ruling: timestamp temperature at RDTA receipt and divide long ramp intervals into cancellation quanta without shortening total normal spacing — thermal freshness must not be inflated by later state queries, and metadata stop must not wait a whole configured step interval — cost if wrong: slow multi-query snapshots may already be stale at publication and need a later explicit operation; small wait-loop overhead can lengthen ramps, never accelerate them.

40. Task 15: Ruling: native PM400 cancellation quarantines uncertain I/O and requires explicit close/reconnect, rather than Python's abort/drain/sentinel auto-resynchronization — shared native transport deliberately fences ambiguous reads, and no late response may become a subsequent reading — cost if wrong: operator needs another explicit Connect after cancellation; a held vendor call retains its session until actual completion. No measurement or configuration command is replayed.

41. Task 15: Ruling: add private typed PM response-boundary probe that accepts only an exact zero-byte native timeout, with one-millisecond timeout and no generic fault-fence bypass — the Python driver rejects extra buffered responses; ordinary VisaSession reads intentionally fault on timeouts, so a separate strict idle probe is needed — cost if wrong: one extra finite native read per query; a vendor ignoring its timeout retains its device-domain owner. Every normal exchange sets its own timeout instead of depending on mutable restoration.

42. Task 16: Ruling: retain reviewed capability-bit gates rather than inventing detector-type numeric meanings; zero/calibration requires optical power or energy capability in addition to confirmation — connected sensor flags are actual available protocol evidence, and no-sensor zero must not be admitted — cost if wrong: a flags-zero adapter requires a capable sensor configuration before optical zero; unclassified physical detector types remain firmware/sensor conditional rather than guessed.

43. Task 16: Ruling: use typed positive u32 average count and a finite 1..128 explicit error-drain budget — native API and bounded device-domain work cannot admit Python's arbitrary-size positive integers or unbounded drain counts — cost if wrong: clients requesting more than 128 errors must make another explicit consuming read rather than one oversized drain. Typed facade methods, limits and confirmation remain preserved.

44. Task 17: Ruling: add optional receive-count inspection to the serial backend, defaulting to unavailable rather than fabricated zero; native Windows uses ClearCommError/COMSTAT without purging input — MDT's prompt boundary must inspect already-arrived bytes without a blocking post-prompt read — cost if wrong: third-party injected MDT transports must implement the inspection; existing Gain/Voltage backends do not call it. Communication error flags fault rather than being silently cleared.

45. Task 17: Ruling: retain three complete-frame monitor failures, but immediately quarantine a partial/ambiguous native exchange and require explicit close/reconnect — the shared native fault fence prevents unsafe late-reply reuse; recover remains read-only and disarmed on a clean stream — cost if wrong: ambiguous transport failures require a new explicit connection instead of in-place auto-resynchronization.

46. Task 17: Ruling: initial typed getters refresh the complete reviewed query snapshot while the periodic monitor reads only XYZ — correctness/identity/control-state evidence is shared through one serialized owner, with no raw endpoint — cost if wrong: single-property reads incur additional finite serial queries. Later native integration must keep this off the GUI thread and report pending work.

47. Task 18: Ruling: invalidate command authority on every fault instead of preserving a Python reduction-only FAULT exception when an old command was confirmed — uncertainty must stop and hold and require explicit baseline re-adoption under current workspace rules — cost if wrong: an otherwise provably reducing operation needs read-only recovery and attestation, or separately confirmed emergency zero, before proceeding.

48. Task 18: Ruling: enforce >=50 ms between each unequal-axis write, and refresh Master/XYZ/hardware/axis limits/DAC after every interval — older coordinated rounds could issue three consecutive axis setters and relied on cached limits — cost if wrong: unequal multi-axis ramps take longer and use extra finite queries; frontend/worker must keep work pending off the UI thread.

49. Task 18: Ruling: bounded 100000-step and one-day request budget, per-packet finite timeout, <=50 ms interruptible wait quanta; retain native owner on deadline — arbitrarily small configured steps otherwise create unbounded worker work — cost if wrong: extremely fine/slow profiles require smaller explicitly requested segments; ordinary 0.1 V/50 ms profiles remain supported.

50. Task 19: Ruling: freeze the approved serial-to-side and logical-to-controller axis mapping in validated Fiber configuration, while keeping directional coefficients/polarity and lowered limits configurable — changing those identity/coordinate definitions would describe another physical setup, not this approved setup — cost if wrong: another lab layout needs a separately reviewed setup definition instead of silently editing these bindings.

51. Task 19: Ruling: add an explicit Host-controller/current-config-bound supervised normal-connect authorization for Gain/Voltage and reuse its cached session for supervised identity proof — the previous native interface could neither bootstrap a supervised session nor prove it without circular registration, and read-only probes must not perform startup writes — cost if wrong: direct callers must supply the separate acknowledgement/binding; no automatic availability check may use this path. Existing profile modes and physical-stage policy stay unchanged.

52. Task 19: Ruling: generalize saved read-only preconnect verification to PM400/MDT and Fiber members, inheriting the setup's current Host lease while each native physical claim remains exclusive — restored records are metadata, not fresh worker identity authority — cost if wrong: explicit Connect includes finite read-only probe/release operations before the live connection and can reject a saved identity mismatch rather than rebind it.

53. Task 19: Ruling: EOF safety begins independently per device domain; each Gain close orders current-off before TEC-off and each Voltage close zeros immediately, while piezos hold — global serialization behind a stalled OSA would delay safety for unrelated devices — cost if wrong: cross-device shutdown byte ordering is not promised; each instrument's required ordering and truthful retained responsibility are.

54. Task 20: Ruling: centralize device diagnostics through the complete typed native SystemFactory instead of creating six forwarding-only device modules — shared admission/lifecycle rules must not drift from Worker behavior; all requested subcommands remain available — cost if wrong: special future diagnostic protocols need a dedicated adapter, not a copied driver.

55. Task 20: Ruling: keep actual TLS diagnostic regression in the Host library beside private credential/session APIs, with CLI remote-stage tests in Code/Debugs — exposing credential injection solely for an integration test would expand the public trust boundary; actual TLS, raw archive bytes and release receipts are still tested — cost if wrong: running only the CLI test target omits the TLS test; the mandatory workspace test script includes both.

56. Task 20: Ruling: diagnostics default to pure preview, require exact stage/binding/action acknowledgement and a new Result/name directory; incomplete local cleanup retains the process owner and native factory for explicit cleanup retry — automatic CLI exit after uncertain cleanup would abandon responsibility — cost if wrong: a permanently stalled native transport keeps the diagnostic process alive and requires operator recovery.

57. Task 21: Ruling: keep Task 11's actual startup identity filename native-package.json, with native-worker.schema.json describing it — the brief's native-worker.json spelling conflicts with the already tested fixed resolver — cost if wrong: external packaging consumers must use the documented identity filename rather than an unimplemented alias.

58. Task 21: Ruling: package source_revision is a deterministic content hash of the actual compiled inputs, package_revision includes that hash — a Git HEAD alone would mislabel builds from this dirty linked checkout — cost if wrong: line-ending changes produce a distinct package identity; reproducibility requires matching input bytes, not only a commit name.

59. Task 21: Ruling: use a pinned copy of tauri-bundler 2.10.1's NSIS template to remove RestartManager forced shutdown, check ownership before any older-uninstaller path and refuse automatic old-uninstaller execution — PREINSTALL hooks alone run too late, and the stock template can terminate a GUI — cost if wrong: template upgrades need a reviewed port; operators use a new empty folder or separately uninstall an old package after ordinary shutdown.

60. Task 21: Ruling: require preinstalled WebView2 and detect vendor prerequisites without downloading/installing them — candidate qualification must not silently change vendor licenses/backends — cost if wrong: a new Windows computer needs the separately installed Microsoft WebView2 runtime before GUI installation.

61. Task 21: Ruling: statically link the Microsoft C runtime in all packaged native executables — a PE import check found VCRUNTIME140.dll in the initial candidate, which would introduce another target-machine runtime prerequisite — cost if wrong: larger executables and a separate package build cache; native/debug regression ABI remains unchanged. Verify imports on the final candidate, not a source-build assumption.

62. Task 22: Ruling: format migration-owned files, leaving untouched legacy formatting and pre-existing user-dirty leases.rs/sessions.rs/main.rs intact — full fmt would sweep unrelated code and user changes into this migration; scoped rustfmt and diff checks are the gate, full fmt failure stays explicitly reported — cost if wrong: whole-tree formatting is deferred, not falsely described as passing.

63. Task 22: Ruling: hand off implemented/offline-tested candidate with genuine clean-Windows, hardware and interactive two-PC acceptance pending — no fresh machine/VM is available and implementation approval is not staged physical or installed-App replacement authorization; the approved plan explicitly permits this handoff — cost if wrong: vendor/protocol/installer compatibility defects can still surface in those required acceptance stages; do not mark deployment qualified.

64. Final: Ruling: actual firmware/physical outputs remain unjudged offline — pending separately authorized native hardware acceptance is not credited from historical Python trials — cost if wrong: real protocol/vendor/output defects remain possible until those checks.

65. Final: Ruling: genuine clean-Windows install/upgrade remains unjudged — compiled candidate and script guards are not an installed-package pass — cost if wrong: target-machine prerequisites/installer behavior may require correction at acceptance.

66. Final: Ruling: interactive Apps and actual two-PC/Tailscale remain unjudged — TLS/fixture regression is useful but not actual interactive network acceptance — cost if wrong: target network/GUI issues may surface at acceptance.

67. Final: Ruling: legacy Python experiments/analysis remain outside this backend rewrite — the approved scope retains references rather than silently rewriting experimental procedures — cost if wrong: those old procedures still need separately planned migration before native live execution.

68. Final: Ruling: exclude the three pre-existing user-dirty files from this change — preserve unrelated work and review the committed range — cost if wrong: uncommitted external changes are not covered by this branch review.

69. Final: Ruling: replace unavailable privileged file-symlink regressions with actual unprivileged NTFS ancestor-junction regressions — Windows returned privilege error 1314 before reaching the bug; the junction tests reached the vulnerable read/backup path and failed behaviorally — cost if wrong: the tests exercise ancestor redirection rather than a privileged final-file symlink; production handles reject both reparse classes.

70. Final: Ruling: use same-parent native handle rename for pinned atomic configuration writes — ReplaceFileW/MoveFileExW reopen the write-protected parent and broke 59 existing tests; NtSetInformationFile replaces only the fixed simple destination inside the pinned parent — cost if wrong: this implementation is Windows-specific and future filesystem compatibility needs explicit validation.

71. Final: Ruling: call cooperative safety notifications outside the scheduler status lock and gate safety execution until all admitted notifications settle — the original permanent stop invalidated typed safety work, but executing before notification completion can cancel the new safety call — cost if wrong: a stalled metadata hook can retain that lane; cached status and other domains remain responsive rather than declaring release.

72. Final: Ruling: cooperative cancellation invalidates only the old operation generation; terminal faults, EOF and disconnect still stop permanently — current-off must preserve TEC while real faults still disable current before TEC — cost if wrong: an unsupported adapter defaults to terminal fencing instead of accidentally enabling normal control.

73. Final: Ruling: expose data-only capture recovery without granting an instrument lease — a completed read must be savable even after disconnect/control release; the exact ticket, configured OSA and revision/context still bind original samples and provenance — cost if wrong: storage authorization remains Host-client trust, not instrument control; no new hardware action is admitted by this endpoint.

74. Final: Ruling: retry a failed staging write using only the same owned partial file identities and original capture signature — fresh file IDs exhaust the bounded capture budget during repeated disk failure; linked/replaced/unowned files must never be overwritten — cost if wrong: unknown or externally altered partial files require operator recovery, not an automatic overwrite.

75. Final: Ruling: EOF can release native handles after raw samples are durably staged, leaving their native file and manifest unacknowledged — a Host pipe cannot archive after EOF, but retaining raw samples forever prevents recoverable shutdown; staged bytes are not claimed as a completed Host archive — cost if wrong: a process/power failure before durable staging can still lose RAM-only samples, and orphan staging requires operator recovery rather than automatic remeasurement.

76. Final: Ruling: preserve the full ordered decision/cost record in the acceptance document and link it from a concise handoff — the user explicitly requested efficiency instead of extensive verification narration; no decision or pending qualification is hidden — cost if wrong: the complete audit requires opening the linked document rather than reading a long chat message.

77. Final: Ruling: wait finitely for the exact asynchronous terminal event in the native remote-observer regression — under concurrent release compilation, a repeated full suite reproduced an empty queue after durable operation completion; Host code intentionally publishes the event after journal finish — cost if wrong: the test permits up to three seconds of notification latency but still fails missing or mismatched delivery; no hardware command or production retry was added.
