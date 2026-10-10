# Preserved tracking after reconnect

## Evidence and cause

After switching to the previous candidate, the operator connected the Laser
and reported disabled Goto and scan controls. Authenticated cached metadata
shows READY with no pending exchange, move=null and motion_pending=false,
while both full/motion readings report OPC=false and tracking=true. The
controller target is 1069.310 nm. These fields show current Tracking On
without an App-owned move, consistent with preserving connection. They do
not establish when Tracking was enabled. No agent instrument query or setter
was issued to investigate; the helper reads Host/Worker metadata only.

The UI required OPC=true for Goto and Full Scan, and native ordinary control
also rejected OPC=false. Worker ownership already distinguished its own
moving/stopping lifecycle, but that distinction was missing from admission.
Previous fixtures started with tracking off and produced OPC=false only
alongside a modeled owned move, so the preserving-reconnect path was missed.

## Repair

The UI admits Goto/Full only for a known inherited state: no owned move,
motion_pending=false, OPC=false, Tracking On, with live confirmed authority
and reviewed bounds. It retains pending/unknown/lease/owned movement gates.
Piezo, single scans and limits retain their existing completion gates.
The hint describes current Tracking On and preparatory hold; it does not
claim when Tracking was enabled or call it active App movement. Enter and
Goto remain the same typed action.

The Worker sends Goto and Full to a typed BeginMove command. The native
begin_move API accepts only these controls. It validates consent, target/
scan bounds and Full Scan controller speed before writing. For fresh busy
tracking, it sends the existing Stop + Tracking Off driver sequence, checks
fresh OPC=true and tracking=false, then starts the requested move in the
same serialized owner exchange. Hold failure faults without a new target,
scan start or replay. An already-ready controller skips preparatory hold.
No connection, ordinary read, close, emission or blanking policy is changed.

## Verification

Behavioral native and Worker RED logs reproduce busy rejection and failure
phase before the repair. GREEN covers inherited Goto/Full, invalid input/
speed before writes, unknown busy rejection, failed hold without replay,
read-only connection, unchanged emission, and retained active owned-move
rejection. Native driver/protocol: 56 passed. Worker laser: 29 passed.
All frontend tests: 417 passed.

Mounted production UI includes both reconnect scenarios with OPC=false and
Tracking On before any explicit intent. No startup intent is sent. Enter/
Goto and Full are enabled, dispatch exactly one typed action, then new
movement is blocked while Stop stays available for the modeled owned move.
Existing context-loss, unknown-result and delayed-exchange tests also pass.
Finite injected transport evidence is not physical controller qualification.

One fresh whole-change review found no Critical or Important defects and
two Minor defects. Both were addressed in one pass: missing move ownership
must not count as explicit move=null, and the hint must not infer when
Tracking was enabled. A failing missing-field regression was captured before
using strict move===null; all 11 focused UI tests then passed. Mounted
production UI passed again after both fixes.

Evidence: Result/laser-reconnect/{initial-state.json,native-red.log,
native-green.log,worker-red.log,worker-green.log,ui-red.log,ui-green.log,
review-ui-red.log,review-ui-green.log,browser-final.log}. Fresh final full
offline regression succeeded with exit 0: 587 Rust tests, 417 frontend tests,
13 packaging groups. Evidence: offline-final.log and offline-summary.json.
The final cached pre-switch inspection again reports READY, move=null,
motion_pending=false, Tracking On and OPC=false, matching the regression
fixture. The old App/Host still owns the Laser; the normal resource-release
switch remains pending in this record.

## Verified candidate

Portable candidate: Result/native-package/reconnect-20261008-01/portable.
Package revision: `0.1.0-f344213e4141`.
Source fingerprint:
`tree-f344213e41410fe364eea8d15465c86c49c93d0f70e1fab8d364ba97bd23813f`.

The actual build and closed-package check succeeded: 29 files, three bundled
driver packages, 18 vendor pins, no Python or fixtures. All three executable
files are AMD64 with no Python or dynamic MSVC CRT imports. The exact
packaged Worker started empty/disarmed on a System32-only PATH, accepted a
metadata ping and exited normally on EOF with code 0. This check started no
Host, GUI or instrument session. The pristine pre-launch archive contains
29 regular files. Evidence is outside the portable directory.

GUI SHA256: `f56952c37b4b8cd2c44ea4dbe9ec3aec9dea3045f725050ddd566acca9b8270c`.
Host SHA256: `870314af8e769f3b82d0603f5313bb682322d6145f615c42937400f990146808`.
Worker SHA256: `9134354d7900b86e321a83fa25e6cbd12cdd37efe9eb2e14e1eb002f2b2655c9`.
Archive SHA256: `23eed043b950a6f07b4b54c1c9eaf844afc78349a1efb9e8a4c06b4fd2081758`.

Evidence: Result/laser-reconnect/{build.log,package-check.json,pe.log,
worker.log,archive.json,pre-switch-state-final.json}. Operator requested to
normally disconnect Laser and exit the old App before a guarded Host stop
and visible candidate launch. No responsible process was forcibly stopped.

## Completed software switch

After the operator reported exit, the old App was absent. Cached metadata
confirmed both domains DISCONNECTED, null connection ID, responsibility=false,
pending=0 and completed release. The metadata helper's guarded Host stop
validated current-context cleanup receipts and confirmed resource release and
normal Worker exit. Physical voltage zero was not claimed. All three old
processes were absent before the candidate was launched.

The unchanged candidate passed its closed-package/source/hash check again,
then launched visibly: App PID 61584, Host PID 2516, Worker PID 14244. All
three executable paths resolve to reconnect-20261008-01/portable. Authenticated
startup confirms Rust, protocol 3, verified startup/activation, no startup
error, responding App and the Yang LAB INSTRUMENT CONSOLE window title.
Both saved instruments started DISCONNECTED with null connection ID, no
responsibility and no pending work. No agent instrument connection, setter
or motion diagnostic was issued.

Evidence: Result/laser-reconnect/{after-exit-processes.json,after-exit-state.json,
safe-stop.json,pre-launch-check.json,new-launch.json,new-startup.json,
startup-summary.json}. This completes the software repair and switch; it
does not assert that the operator's next physical motion has been tested.

Forward/Backward still require their separately authorized physical speed/
hold qualification; this repair does not open that production gate.
