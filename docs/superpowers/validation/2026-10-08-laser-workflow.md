# Laser workflow and Gain identity validation

## Behavior

Wavelength editing is local until Enter or Goto Wavelength. Both issue the
same typed, deduplicated Goto action. The Rust session observes the owned
move independently of the visible page and turns tracking off after fresh
endpoint evidence, then verifies OPC complete and tracking off. A slow
getter does not contribute to the minimum endpoint stability interval.
External target/tracking changes terminate ownership without a setter.
Full Scan does not use the Goto stability shortcut or its 120-second timeout.

Stop waits for the exact active exchange and remains available during motor
movement and ordinary reads. Queued actions are rejected after connection,
Host boot or control-lease changes. Unknown outcomes are not replayed.
Emission switching is independent of motor OPC and retains the controller
key/interlock requirements. Local drafts and focus survive telemetry.
Both scan velocities default to 0.1 nm/s, bounded by a lower operator cap.

The actual cached Gain identity reports model `Gain Chip Driver`, resource
`COM4`, protocol `READY fields`, quality `weak_protocol`. The reported model
matches the selected model. Host identity validation rejected the additional
native scalar metadata. Validation now accepts bounded reviewed fields;
unknown fields, nested values and wrong models remain rejected. Regression
covers retained verification, saving and reopening the native identity.

## Verification

RED/GREEN logs in Result/laser-workflow cover emission while OPC busy,
owned arrival/timeout/interruption, fresh conditional hold, external
Tracking Off with OPC false, slow getters, and external Full Scan stop.
All 40 native laser and 27 Worker laser tests pass after the review fixes.
The earlier complete suite produced 579 passing Rust tests, 414 frontend
tests and 13 passing package groups. Its outer command incorrectly treated
the final intentional negative-test child exit code as suite failure;
offline-verified.log records the successful rerun (exit 0) with script success
checked directly: 579 Rust, 414 frontend tests and 13 package groups passed.

Mounted production UI checks draft-only digits/arrows, equivalent Enter/Goto,
repeated Enter, independent emission, Stop queued behind delayed reads,
context change rejection, unknown-result no-replay, preserving newer drafts,
shortcut capture/enable/persistence, limits/reconnect and 800/960/1440 widths.
Evidence: Result/laser-workflow/browser-final.log and browser screenshots.
The finite modeled transport exercises UI behavior, not physical qualification.

One fresh whole-change read-only review found two important arrival-evidence
defects and no critical/minor findings. Both were reproduced with failing
regressions and fixed. An additional external Full Scan interruption failure
was reproduced and fixed in the same pass.

## Candidate

Portable candidate: Result/native-package/goto-20261008-01/portable.
Package revision: `0.1.0-c1e9ceddbf39`.
Source fingerprint:
`tree-c1e9ceddbf39985640ed140fe4c1a711bad0ed9a5d2efdc2c301b3a5ed7303c5`.

The build and closed package contract passed: 29 files, three bundled driver
packages and 18 vendor pins; no Python or test fixtures. All three executable
files are AMD64, with no Python or dynamic MSVC CRT imports. The exact
packaged Worker started empty/disarmed on a System32-only PATH and exited
normally on EOF with code 0. No Host or instrument was started in this check.

GUI SHA256: `d0996b872db1127814265538fa455a06db5b2f5502138717d11c2959b6dff50a`.
Host SHA256: `870314af8e769f3b82d0603f5313bb682322d6145f615c42937400f990146808`.
Worker SHA256: `1a9f4b90b342a36cd99fff26b6967a73f10956c84bc6048476a5e9235bf58729`.
Pre-launch archive SHA256:
`d562f84b838b858b4bc9c0196d3eb198d4ed875cbd9778db1371c90d2d505c1c`.

## Remaining physical qualification and switch

Forward/Backward remain production capability-gated. RESET's actual speed,
blanking and endpoint hold have not been physically verified. The diagnostic
preview opens no device; enumeration, read-only checks and bounded motion
each require separate operator authorization under AGENTS.md. No agent
hardware command has been issued during this repair.

Pre-switch metadata still shows live Laser and Gain owners in the current
App. Normal disconnect/draft cancellation and window exit are required
before a guarded Host stop, diagnostic acquisition or software switch.
No process was forcibly terminated. The candidate is built and qualified,
but has not yet been launched in place of the current App.
