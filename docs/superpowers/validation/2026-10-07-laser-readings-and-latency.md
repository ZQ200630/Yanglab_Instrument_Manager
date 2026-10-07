# Laser readings and prompt operation publication

Baseline `81b76803fab9a7515b8a2341cf20753136fe4e2d`, branch `codex/laser-1060-desktop`. The readings card emphasizes wavelength, power, current and output state. Local/Remote, current/power mode and tracking remain a small operational line. Raw status byte, completed-operation indication and the longer readback explanation move into closed Reading details. Controller readback remains explicit beside wavelength. Busy state, controller error-buffer indication, read failure, stale readings and unknown connection remain visible outside the collapsed details. A retained output value is labeled Last reported when freshness is unknown, the Host is offline, or its event sequence has a gap.

Refresh now reuses the asynchronous typed read-status operation, immediately showing a disabled Refreshing button/spinner. The expanded details and its DOM node survive telemetry updates. No tuning, output, Remote/Local, tracking, reset, head-envelope or admission gate was changed.

## Cache publication

The worker already acquires an ordered post-operation observation. Previously the Host could publish an ordinary terminal receipt while retaining metadata up to 2500 ms old. The new callback retrieves the worker's cached metadata before terminal publication when the reserved query lane is free. This metadata request does not acquire instrument status again.

A busy inventory/scan cannot hold an already completed operation's terminal receipt. Metadata uses try-lock on the existing reserved query lane; a free-lane reply wait is bounded at 500 ms with the existing PendingReply timeout/responsibility accounting. If metadata is delayed or fails, terminal evidence publishes with unknown freshness, and a queued metadata refresh publishes afterwards. No hardware operation is replayed.

Metadata is versioned at operation completion. Each status cache retains the generation captured before its query. Both TTL reuse and snapshot freshness require the generation to match, so an older cache hit/reply cannot erase a newer completion's dirty state. Cache hits do not clear query failure; only an actual serialized status query updates failure. The query lane, worker reservations, leases, exact identities and cleanup evidence remain in force.

## Latency interpretation

`Code/Utils/tlb_native/src/sdk.rs` currently sleeps 200 ms before each SDK send. This is a software compatibility policy, not a measured physical USB latency floor. Full status makes 13 ordered requests, explaining a 2.6 s pacing floor and the previously measured 2.6896 s status read. SDK initialization also drains the receive buffer before identifying devices. The legacy Newport transport documents an approximately 2 s empty-input SDK timeout; the current initialization/drain split has not been instrumented separately.

The same connected native owner handles subsequent operations. There is no SDK reinitialization for every ordinary action. Current code needs two packets for Remote/Local or output-off, three for piezo/tracking/output-on, and four for a wavelength setting: software pacing alone contributes 0.4/0.6/0.8 s before acknowledgement, followed by the current full post-operation status read. These are code-derived timings, not measured setter latency or physical settling time. The connected `6722-P` head remains read-only until its specific limits are reviewed. No setter, sweep, trigger or stopping precision has been qualified here.

The read-only Host diagnostic now records terminal elapsed time separately from sample publication after terminal. It submits each acquisition once and observes the same receipt. Timing fields are diagnostic evidence only and are not displayed as new user-facing controls.

## Verification

- Full VISA driver suite **1032/1032** before hardware; final App suite **518/518**, Node **258/258**, App Rust **175/175**. The full App run includes the production Host with finite transport fixtures and the new timing evidence assertions.
- RED reproduced Host-offline/event-gap readings incorrectly labeled current, a failed forced refresh discarded by a young cached success, and a completed read waiting behind an explicitly held inventory gate. GREEN covers actual production rendering and native Host receipts/snapshots. Unknown freshness persists through failure and recovers after a successful query. The existing query-lane ordering test remains.
- Independent source review approved after fixing the cache-hit/older-reply race with generations. Reviewer did not exercise hardware or claim physical accuracy.
- Isolated Edge acceptance passed at 960/1440 px: normal values/units/output/mode, collapsed debug fields, retained expanded details, exactly one typed read-status request, immediate spinner/disabled state, no duplicate click replay, updated values and visible uncertainty. Screenshot `Result/async-requests/readings-ui/laser-readings.png` uses a finite fixture and is not physical telemetry. Existing responsiveness acceptance also passed.
- Final offline Host/native and GUI builds passed with three pre-existing compiler warnings. Package check found **40 matching resources**, no retired resources. SDK pacing and native-driver code are unchanged.

Before rebuild, the user explicitly authorized normal disconnect and update after an automatic reviewer rejected the initial disconnect as insufficiently authorized. The authenticated preserving disconnect completed, the GUI closed normally, and Host stop separately confirmed resource release and successful worker exit (code 0). No real GUI/Host/native owner was force-killed. A discarded concurrency-test experiment tried two overlapping control attempts on one domain; the production worker correctly rejected that admission. The already finite, hardware-free fixture worker exited unsuccessfully and its retained test Host was removed using its independently verified temporary-root PID. That unsupported test was removed; production admission was not relaxed.

New artifact SHA-256:

- GUI: `BC069860FA48A7190240A7B2CBD364BA21B8B170ED75DD3BFECE06F7EE205372`
- Host: `4FCA19B8A4212AF9A5DCBB4EE12AEDE9A1C17C19615540DE85AE573120FB085E`
- Native Newport worker (unchanged): `5A29F8D0C277C33BAEC5D3D2EAB341437986A0A3277B95E4B14529DC3FCC75C4`

Logs and evidence are under ignored `Result/async-requests/readings-*`. Python diagnostics and tests use Anaconda VISA Python 3.10.16. Output values are controller-reported observations, with no independent optical measurement.

## Real read-only timing and restart

After all final offline checks passed, the authorized diagnostic used the exact new native Host with the production worker/driver, an empty private diagnostic registry and controller `22500001`. It read `6722-P`, head serial `0953`, firmware `2.4`, with output disabled in all three samples. Normal cleanup confirmed no unreleased resources, successful worker exit code 0 and successful Host exit code 0. No output, tuning, Remote/Local, tracking or reset command was sent. The user's registered instrument was not replaced.

| Operation | Terminal receipt | New sample observed after terminal | Total |
| --- | --- | --- | --- |
| Read-only connect | 5.7163 s | 0.0210 s | 5.7374 s |
| Full status read 1 | 2.8213 s | 0.0200 s | 2.8413 s |
| Full status read 2 | 2.8306 s | 0.0190 s | 2.8497 s |

Actual native status intervals were 2.6958, 2.6939 and 2.6940 s. The diagnostic observes receipts with a 50 ms poll plus authenticated pipe overhead; these totals are not an exact GUI click-to-paint measurement, setter timing, or optical settling measurement. The former metadata-cache wait is gone in this attempt: publication after terminal is approximately 20 ms. SDK initialization and the 200 ms packet pacing remain the principal connection/read costs. Evidence: `Result/async-requests/readings-hardware-timing/run.json`.

The updated GUI restarted normally and responded. An authenticated metadata snapshot confirmed no startup error, real mode, the existing `TLB-6700` record and actual controller/head identity, its domain DISCONNECTED and its control AVAILABLE. No native Newport owner remained after the diagnostic. The historical failed-before-spawn ownership record was preserved with SHA-256 `2FDC4F245C80EDB4D725A53F62C4A4CA137B4FAFBBA669838C367E4C10517AC7`. The user can reconnect in the updated desktop UI.

## Follow-up: stable output display

The user found the recurring Last reported label distracting. A healthy connected observation now keeps the same reported output badge, reading colors and card height as its known age advances past five seconds. Last updated remains visible in the footer. Age alone no longer adds the outdated notice. This supersedes the earlier age-only Last reported presentation described above; the five-second control eligibility gate is unchanged. Lost connection, missing age, Host offline/event gap and read failure still show Output unknown and visible uncertainty, preserving old reading values without claiming a known current output.

RED reproduced the old age-dependent presentation and old failure/offline badges. GREEN: all **258 Node tests** and isolated Edge acceptance pass. The browser checks identical badge markup, reading color and card height at 4.9, 5.1, 9 and 60 seconds; outdated readings still cannot enable output controls. No backend or instrument command changed. The screenshot under `Result/async-requests/stable-output-ui` is a finite UI fixture.

Using the existing explicit normal-disconnect/update authorization, the active TLB session was normally released; authenticated evidence confirmed DISCONNECTED/AVAILABLE before normal GUI close and Host stop. Resource release and successful worker exit code 0 were separately confirmed. The new GUI build and 40-resource package check passed; GUI SHA-256 is `7E99E4A39BE7A90A9CF99940939CA1AEE4D12D23060051B11306B5BF66FF0267`. Host/native hashes are unchanged. Restart confirmed no startup error and the existing record, available control and disconnected instrument. No real owner was force-killed and no output-setting command was sent.
