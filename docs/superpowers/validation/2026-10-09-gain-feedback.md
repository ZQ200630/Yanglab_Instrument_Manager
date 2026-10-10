# Gain feedback without layout jitter

The operator reported repeated Pending/unknown-readback messages that made Gain
controls jump during use. Read-only Host metadata for the pictured safety attempt
e3ea12c0142f263b28e42f8675a4b1cb shows completed TEC Off followed by fresh readings.
Command-boundary invalidation is intentional: prior samples cannot prove a write
completed. No driver, scheduler, interlock or command-admission behavior changed.

## Root cause and bounded fix

Finite Chromium reproduction measured 62 px vertical movement whenever the
conditional readback warning appeared or disappeared at 5 Hz. Input, monitor and
control nodes retained their identities; wholesale DOM replacement was not the
cause. Separate operation-progress blocks and one/two output-button transitions
could also change geometry.

Gain also called shared legacy roleProgress, which expects a safetyPending object
with sentEpoch/intent. Native instanceView supplies a boolean/request ID and nested
safety result, so a completed stop awaiting synchronization could still display
"Pending · attempt ...".

The connected Gain dashboard now owns one persistent, compact feedback strip for
activity, readback, progress and errors. It replaces the separate top activity,
native legacy role-progress line, conditional alert and progress block. Output
controls use fixed slots. Disconnected pages retain the existing outer lifecycle
feedback. Shared legacy connectionAction/roleProgress remains intact for its
structured safety context, explicit Resume and stability-wait recovery controls;
native console routing already sets hideConnectionAction before panel rendering.

Expected unknown readback during a pending command uses neutral waiting text and
an Updating badge only for the exact live, controlled connection, READY/ACTIVE
driver state and fresh/unknown fields without known age above five seconds.
Explicit stale/error, foreign-connection evidence, fault, disconnect or uncertain
operation remains a warning. Previous values are labeled as previous. Existing
normal-command gates and separate Off eligibility are unchanged; no command is
replayed and fresh readings cannot clear an uncertain original attempt.

## Verification

Unit RED reproduced the old warning and legacy Pending label. Additional REDs
covered old unknown evidence and the transient Check status badge. All 52 focused
tests now pass. Two new Chromium regression groups first failed on actual layout
movement; the final Gain suite passes all 20 groups and the existing laser suite
passes. Finite 800/960/1440 px probes each publish 32 observations at 200 ms,
including operation progress, a wait exceeding five seconds and completion:
monitor/control/feedback movement is 0 px, feedback height is 88 px, and inputs,
caret, draft, safety Off and overflow checks remain correct. Root inspected the
rendered desktop feedback screenshot. Independent review found no new P1/P2.

Full native verification first failed in the existing finite test
idle_gain_cache_poll_keeps_watchdog_samples_fresh_without_transport_io with
"finite Gain peer did not settle". Its exact isolated rerun passed. Read-only
investigation identified a possible ManualClock/actor synchronization race in the
test's first revision wait; no Rust source was changed. The initial failure is
preserved rather than relabeled. The next full run passed Rust but exposed six
frontend regressions: removing shared legacy roleProgress suppressed recovery
feedback, and one readback-label assertion required the old wording. Restoring
shared legacy rendering and explicitly accepting "Previous readings" fixed these
without changing disabled, old-context or replay checks. The native scoped test
uses the production hideConnectionAction flag. All 81 recovery/UI focused tests
pass and the final native-route Chromium suite passes 20/20.

The final full offline wrapper passes all 722 Rust tests, 497 frontend tests and
13 packaging checks, exit 0. Independent review confirms legacy recovery and
native feedback routing coexist and finds no remaining P1/P2.

Evidence: Result/gain-jitter/{unit-red.log,old-evidence-red.log,
pending-badge-red.log,unit-green.log,full-native-first-failed.log,
telemetry-isolated.log,full-native-legacy-failed.log,legacy-recovery-green.log,
full-native-green.log}; Result/gain-ui-jitter/{browser-red.log,browser-green.log,
browser-final-green.log,laser-browser-green.log,responsive-probe.json}. Browser clients are finite injected
transports; they cannot access the real Host or lab outputs. Candidate 07 remains
running during source work; no live instrument commands were issued.

## Qualified delivery candidate

Candidate 08 is staged at Result/native-package/gain-console-20261009-08/portable.
Package identity: 0.1.0-1135a5510113. Source revision:
tree-1135a5510113364e5505b3163001f9afb3b377beaacc878b3de6e22afda9d538.
The independent package check confirms 29 files, three driver packages and 18
pinned files. All three binaries are AMD64 PE with static CRT and no Python
imports. The exact packaged Worker passes empty, disarmed ping and EOF exit 0
with System32-only PATH, without starting a Host, GUI or hardware connection.

GUI SHA256: 4f36fec926b0bdef9b1d48dfb3ee1fc00ba1ff2170765a260b57f838e3e7ff9b

Host SHA256: 2147c4f3177f03ecb1d630b55172b225997a4b2cf5a9b5e262c006d9bb24db69

Worker SHA256: c0860215fedbc323db3e740a70dfe21c01c554d26e956d1e70ce6b8e37143631

ZIP SHA256: 48a349657e807d1640f822011190a80fcbd7c805b9338e696bf6f3d1e88ac00a

Every ZIP entry matches the pristine 29-file portable package. Qualification
evidence: Result/gain-jitter/{build-candidate.log,package-check.log,
pe/pe-qualification.json,portable-worker-check.log,zip-qualification.json}.
No physical output validation was performed. Read-only pre-switch Host metadata
still shows candidate 07 online, Gain READY and connected, Laser DISCONNECTED
and no pending requests. Switching awaits normal operator disconnect and exit.

## Activated after normal exit

After the operator replied "已退出", fresh inspection confirmed the old GUI had
exited and both domains were DISCONNECTED with null connections, no responsibility
and no pending requests. Gain's current completed disconnect receipt records
current_off, tec_off and transport_close with no errors or unreleased resources.
The guarded formal Host stop confirmed resource release and Worker exit 0;
physical_zero_verified remains false.

Candidate 08 passed another package check before launch. GUI PID 24336, Host PID
49660 and Worker PID 73928 run from the qualified portable directory, with all
three executable hashes matching qualification. The visible GUI responds, Host
is ONLINE, startup_error is null and the new boot is
432b3fcf414391b068e0a41ad9a12849. The first post-launch observation had both domains
DISCONNECTED; the final observation has Gain READY, Laser DISCONNECTED and no
pending requests. Deployment issued no instrument connect or output commands
and left the new Gain connection intact.

Evidence: Result/gain-jitter/{post-exit-host.json,safe-stop.json,
pre-launch-package-check.json,launch.json,running-host.json,delivery-host.json,
running-processes.json,delivery-complete.json}. Source implementation: c574cc4.
