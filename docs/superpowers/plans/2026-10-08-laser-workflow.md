# Laser workflow repair

## Approved spec

The operator edits a local wavelength draft with digits/arrows. Nothing is sent until Enter or Goto Wavelength, which use the same action. One accepted move enables tracking and the Rust session turns tracking off only after fresh endpoint and completion evidence. Moving, arrived, stopped, interrupted, uncertain and failed are distinct states. Drafts survive background readings and focus changes.

Full Scan performs one Start → Stop → Start cycle. Forward/Backward perform current → selected endpoint and hold, but production transport qualification is still required for their speed/hold implementation. Stop is available with live authority during movement and ordinary reads, serializes behind the exact active exchange, and never replays unknown setters. Emission is independent of motor completion and remains subject to controller key/interlock. Default forward/backward velocity is 0.1 nm/s (or a lower operator ceiling).

## Global constraints

Keep existing preserving connect/close and driver bounds. No hardware diagnostics without separately approved enumeration, readonly and reversible-action stages. Do not terminate a responsible Host/Worker. Tests use finite injected transports. Do not ship fixtures or Python. Stop completion and optical output must be based on readback, not ACK.

## Tasks

1. RED→GREEN native emission-busy tests and explicit owned-move completion tests. Replace the superseded live-retarget experiment. Add strict goto_wavelength catalog/Host/Worker admission. Rust scheduler observations finish owned moves independently of the GUI; external target changes or interrupted tracking never cause an automatic write.
2. RED→GREEN UI state tests and mounted browser interaction tests. Draft-only edits; shared Enter/Goto commit; per-operation availability; queued Stop behind the current exchange; preserve draft/focus; background refresh without disruptive waiting banners. Use 0.1 defaults and explain actual movement/capability restrictions.
3. Full native/UI/package regression; one fresh whole-change review and meaningful regression fixes. Build a fresh portable candidate, verify PE/runtime/package evidence, then normal resource release and switch the App.

## Interfaces and review focus

goto_wavelength(wavelength_nm, confirm=true) → typed Worker owned move → snapshot.move (kind, phase, target_nm, elapsed_s, message). Read-only transport queries remain read-only; completion is a separately typed conditional command. UI draft has explicit dirty state and is scoped to connection authority. Ordinary pending requests serialize I/O without blocking local edits, output/Stop affordances. Review unknown-response handling, external-panel target changes, close while moving, arrival tolerance, final Tracking Off verification, stale authority/queued Stop, and qualification gates.

## Ledger

- Pre-flight: Worker scheduler observes connected sessions every 2.5 s, independently of the visible page. Use this existing lane for owned completion; no UI polling lifecycle dependency.
- Pre-flight: UI normalPending currently conflates reads and motor conflicts; per-action intent queue will retain one exact active exchange and reject obsolete authority after awaiting it.
- Ruling: Replace unpublished live-retarget edits/tests with draft-only commits, per the operator's latest design. Existing shipped Stop repair and diagnostic preparation remain.
- Ruling: Goto timeout is 120 s; report timed_out and require operator Stop rather than inventing arrival or replaying a setter. Full Scan can legitimately run longer and waits for cycle completion. Cost: a long legitimate Goto needs explicit recovery.
- Ruling: Forward/Backward stay capability-gated until real staged speed/hold qualification. Cost: they cannot be honestly enabled by software-only tests.
- Ruling: Firmware 2.4 real journal shows OPC false with actual wavelength equal to target. For Goto only, two endpoint observations at least 1 s apart (within 0.005001 nm, same owned setpoint) may request tracking-off. The serialized native exchange rechecks the endpoint and verifies tracking off plus OPC true before reporting arrived. Full Scan does not use this shortcut. Cost: controller readback remains a controller estimate, not independent optical wavelength metrology.
- Task 1: completed targeted RED→GREEN emission/Goto/timeout/interruption/Stop tests; existing preserving lifecycle and production qualification gate retained. Gain identity mismatch reproduced from actual cached proof: matching model with resource/protocol metadata rejected by Host schema. Added native metadata whitelist and retained proof/save/reopen regression, without accepting arbitrary keys or nested identity values.
- Task 2: completed RED→GREEN panel/draft tests and mounted browser scenarios including repeated Enter, equivalent Goto intent, emission while moving, Stop queued behind a delayed read, draft/focus/shortcut persistence, limits/reconnect and 800/960/1440 layouts. Evidence: Result/laser-workflow/browser.log and screenshots.
- Whole-change review: one fresh read-only reviewer found two important arrival-evidence defects, no critical/minor findings. Accepted both: Tracking Off alone cannot bypass OPC completion, and getter duration cannot count toward endpoint dwell. Added failing native/Worker regressions, then fixed both and passed all 40 native laser and 27 Worker laser tests. Evidence: Result/laser-workflow/review-{native,worker}-{red,green}.log.
- Additional confirmed failure: external Full Scan interruption at a non-endpoint retained the moving phase indefinitely. Added a failing regression and report interrupted without issuing any automatic command when fresh OPC is complete and tracking is off away from the endpoint.
- Queue/uncertainty verification: mounted production UI rejects a queued Stop after connection context changes; an uncertain Goto result preserves the draft and never replays Enter/Goto. Evidence: Result/laser-workflow/browser-final.log. Earlier complete offline suite passed 575 Rust / 414 frontend tests and 13 packaging groups; rerun after review fixes is required before packaging.
- Task 3 software verification: fresh full offline script succeeded (exit 0), 579 Rust / 414 frontend tests and 13 packaging groups. Exact portable candidate 0.1.0-c1e9ceddbf39 passed closed-package, AMD64/static-runtime, System32-only empty/disarmed Worker start and normal EOF exit. Pre-launch archive verified 29 files. Evidence: Result/laser-workflow/offline-verified.log, package-check.json, pe.log, worker.log and archive.json; detailed record in docs/superpowers/validation/2026-10-08-laser-workflow.md.
- Operator-dependent work pending: current old App still owns live Laser and Gain resources. Asked for normal disconnect/draft cancellation and exit before software switch. Separately asked whether to authorize enumeration, the first stage of Forward/Backward physical qualification. No output-changing diagnostic or forced process termination has occurred.
