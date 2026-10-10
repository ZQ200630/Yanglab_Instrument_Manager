# Forward / Backward single-scan qualification chain

Date: 2026-10-08. Worktree: merge-desktop. Baseline: 30064b9.

## Behavior and physical boundary

The user approved completing the four-action workflow. Full is one round trip;
Forward moves from current position to Stop, Backward to Start, and Stop holds.
Default velocity remains 0.1 nm/s. Draft Goto / Enter and emission are independent
of scan settings. The new production qualification set is deliberately empty:
software checks do not establish physical RESET slew or stopping behavior.

The native driver exposes exact reviewed rates, admits ScanTo through BeginMove,
and rejects unreviewed rates before setters or inherited-Tracking takeover. Worker
forwards that capability and publishes the rates. The UI checks the current draft
velocity independently for each direction, including edits without repainting.
Fresh natural OPC-complete / Tracking-off endpoint evidence finishes the motion;
off-target or uncertain evidence never becomes an automatic setter replay.

Production records are compiled, bounded and strict, and bind full identity,
loaded SDK SHA256 and protocol revision. No runtime override exists. The SDK hash
is acquired while that module is loaded. File hash checks themselves load no SDK.

## Offline evidence

RED-to-GREEN evidence is retained under Result/single-scan and Result/laser-single.
Native qualification, native rate/movement, Worker and UI failures were observed
before their fixes. Scoped suites passed: 88 native tests (including qualification,
SDK hashing and protocol), 117 Worker tests, 25 diagnostic tests and 420 frontend
tests. Full workspace regression, independent review and candidate qualification
are recorded below after their actual completion.

Mounted Edge browser test: PASS. It covers inherited Tracking, qualified rate
editing (an unreviewed direction is disabled without native I/O), Forward/Backward
typed intents, shortcuts, delayed holds, repeated Goto, Stop, uncertain outcomes,
no replay, input draft preservation and 800 / 960 / 1440 px layouts. Final log:
Result/single-scan/browser.log. During this check a digit-edit parameter shadowing
the instrument-key helper was reproduced and fixed; the final test passed.

## Physical acceptance still required

The schema-2 native diagnostic retains getter time brackets and requires three
intermediate points with at least 0.08 nm displacement, correct direction and rate
within coarse temporal/readback bounds. A final uninterrupted hold spans at least
one second, with OPC complete, Tracking off and <=0.04 nm readback span. Arrival
uses +/-0.02 nm; exact readback equality is unnecessary. Full final state and
SCANCFG are read separately. Emission / blanking are never set by the probe.

The offline reviewer needs six successful real reports: endpoints both directions
at 0.05 and 0.1 nm/s; midway Stop both directions at 0.1 nm/s. It recomputes raw
predicates, checks settings, common exact identity/SDK and preserving cleanup, then
emits a source-review candidate containing six report SHA256 values. It neither
opens hardware nor installs a capability. Reports from bounded transport tests
are rejected as physical evidence.

Current known controller is TLB-6700 serial 22500001, firmware 2.4, head 6722-P
serial 0953. It is owned by the running earlier App. No current-task hardware
commands have been sent. Separately approved enumeration, read-only and bounded
reversible movement stages remain pending. RESET might move at the published head
maximum (10 nm/s), despite a requested 0.05 / 0.1 nm/s. This uncertainty must be
disclosed before action authorization. Preserve emission / blanking and do not
automatically return to origin. Insufficient speed/hold evidence cannot enable
Forward or Backward in the production App.
## Independent review corrections

One fresh independent review found four Important issues, no Critical issues.
All were corrected before final qualification:

- The diagnostic already allowed +/-0.02 nm arrival, but native / Worker still
  used +/-0.005 nm for scans. Both now share SCAN_ARRIVAL_TOLERANCE_NM. Full and
  Single at +/-14 pm complete; +/-21 pm do not trigger completion or setters.
  Source ownership, setting readback and probe-origin binding remain strict.
- A Stop that ACKed but allowed a later natural endpoint hold could qualify.
  The diagnostic now records Stop start/end, requires an interior final hold,
  <=0.06 nm movement from its preceding sample and stopped evidence within 2 s
  after Stop completion. The reviewer independently recomputes these conditions.
  Ignored Stop, endpoint hold, missing/late timing and excess drift are rejected.
- First/last average rate could hide a fast step followed by a stall. Every
  intermediate bracket now must support the requested linear slope within the
  existing combined readback/time uncertainty. A nonuniform trajectory with a
  matching endpoint mean is rejected; ordinary constant-rate traces still pass.
- Diagnostic hash acquisition re-resolved the vendor path after SDK load. It
  now reads the already-loaded SDK's immutable cached hash through Wire / Bus.
  Unopened / closed instances return None; the getter performs no file or SDK I/O.

The three new diagnostic behavioral counterexamples failed before the fixes
(16 pass / 3 fail in diagnostic-review-red.log), then passed. The native and
Worker arrival defects and cached-fingerprint forwarding have their own RED /
GREEN logs. No hardware commands were executed during these corrections.
Final full regression: App/scripts/test-native.ps1 -Offline exited 0 after all
four review corrections. 655 Rust tests, 420 frontend tests and all 13 packaging
checks passed. Log: Result/single-scan/offline-final.log. Summary:
Result/single-scan/offline-summary.json. The earlier mounted-browser source was
unchanged by the native/Worker/diagnostic review corrections.
## Concrete validation package

Final candidate: Result/native-package/single-scan-20261008-02/portable.
Package revision: 0.1.0-5c4182e4deb2.
Source revision: tree-5c4182e4deb2833f7730659586957ba9c804f454cbb70614e11afdd245a04009.

- GUI SHA256: c19b61f40282076c665803531bd453e8245724a19298a5024738b91e53938ce0.
- Host SHA256: 870314af8e769f3b82d0603f5313bb682322d6145f615c42937400f990146808.
- Worker SHA256: 21d85f431be2d568ed953a02c3ef6218dd36e19ac4715deb85f93561beca1489.
- Pristine ZIP SHA256: ac7b6d53c52aeb47c75d3aa7dbf5b7eef0680f982d8e00701a0b1e972c428510.

The closed 29-file contract and every ZIP entry hash matched. PE checks confirmed
AMD64 for all three executables, no Python or dynamic MSVC CRT imports. Exact
packaged Worker started with PATH restricted to System32, returned the matching
revision / protocol 3 / real / disarmed empty state and exited cleanly on EOF.
No Host, GUI or instrument was started by these package checks. No installer was
built or installed. The compiled reviewed-single-scan table remains empty.

Evidence: build-final.log, package-check.log, pe/pe-qualification.json,
portable-worker/44a55177ccc84b248ea05741df716eba/qualification.json and
zip-qualification.json under Result/single-scan. The first package built before
review corrections is superseded and must not be switched into use.

Enumeration and read-only plans have been previewed without SDK loading. The
cached-position action plan is preview only, not approved for execution; actual
probe origins must be bound to fresh read-only evidence after owner release.
No current-task physical stage has run. User stage consent and normal release of
the earlier App / retained Gain owner are pending.