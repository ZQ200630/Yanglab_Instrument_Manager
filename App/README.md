# Yang LAB INSTRUMENT CONSOLE

The production path is GUI → native Rust Host → protocol-3 native Rust Worker →
Code/Utils and Code/Setups drivers. The TLB Bus is linked into Worker and owns the
Newport SDK through one lazy serialized owner executor. There is no Python
backend, interpreter setting, sidecar bridge or fallback.

Multiple local or paired TLS GUI observers can share one Host. Each instrument
has a revision/context-bound exclusive controller; observers cannot displace it.
Safety, metadata, long background requests and stored results use separate
bounded channels. A slow action acknowledges pending work immediately without
turning an old sample into a new reading or replaying a timed-out command.

## Use

1. Start the App and configure or connect the local Host in Settings. The Host
   stays visible in the tray after GUI windows close. Open Console reopens
   management; Stop Host safely is a separate confirmed lifecycle action.
   Remote pairing retains certificate pinning, approval and authenticated release
   reconciliation. Old Python Hosts are rejected without automatic restart.
2. Device setup → Add New selects category, model and connection profile.
   Test Connection requires current identity-bound staged consent. Add and Save
   requires a valid proof for the exact revision. Unknown models remain Driver required.
   Automatic checks default off; enumeration and active read-only policies are separate.
3. Voltage/Gain supervised connection has separate consent and may perform
   driver safety writes. Connecting is not universally read-only.
4. Connect obtains authority for the exact Host boot/domain/context. Disconnect
   waits for current cleanup evidence before offering another connection.
   Uncertain replies offer status/recovery; they never automatically repeat an
   instrument command. Closing an observer does not stop other controllers.
5. Register known MDT controllers, then create a Fiber setup. Side follows serial
   identity, never port order. Only the setup controls the stages. Release or
   removal holds piezo voltage without zero, rollback or baseline adoption.
6. Add Laser → Newport / New Focus TLB-6700. Prerequisite metadata does not open
   the SDK. Explicit controller scan and Test Connection are separate admitted
   operations. Controller/head identity and narrowed wavelength/speed limits
   remain bound to the saved device and its current revision.

OSA uses Connect → Read trace → Save with native units/samples and front-panel
measurement settings preserved. Sweep initiation remains a separately disclosed
action. Archive/export/recovery uses exact capture hashes and owning Host/domain
identity; navigation cannot move a pending completion to another instance.

## State and safety

LOCAL/REMOTE identifies source. ONLINE/UNKNOWN describes communication evidence,
not physical voltage, emission or motion. Worker startup verification, activation
confirmation, control ownership, sample age and cleanup receipts are separate
facts. Native implementation capabilities allow management attachment even when
startup failed or retained; instrument work remains fenced until actual readiness
and current authority are established.

Configuration is bounded, strict, atomic and revision-checked. Obsolete saved
interpreter settings are defensively discarded with original-byte backups.
Corrupt or unsupported preferences fail visibly. Historical non-real registrations
cannot authorize hardware. There is no raw serial/VISA/SCPI API in the App.

Source limits remain 0–14 V with normal steps at most 0.1 V/50 ms; failures and
shutdown command immediate zero. Gain remains 0–200 mA and 15–40 °C; current
requires TEC on and target ±0.2 °C stable for five seconds, and shutdown disables
current before TEC. MDT ceiling is 75 V with 0.1 V/50 ms steps; faults stop and
hold, and zero requires its explicit authorized method. Fiber toward-chip moves
are at most 0.2 um and other per-axis moves at most 1.0 um. Baselines and nominal
conversion remain explicitly authorized session estimates, never measured motion.
PM400/OSA preserve front-panel settings and retain capability/authorization checks.

## Bundled USB drivers

The exact fixed resources are Newport USB 5.0.8, WCH CH340/CH341 4.0.2026.02 and
Silicon Labs CP210x 11.3.0 (including CP2102). Their 18 original file size/hash pins,
licenses and provenance are under App/drivers; CP210x README records the operator's
manufacturer authorization for unchanged public redistribution.

Only a fresh positive missing-driver check offers installation. Problem 28 means
a missing USB device driver; absent adapters, unknown status and other faults are
not installable evidence. Newport also preserves its missing-SDK case. Metadata
inspection checks fixed SDK paths/PE architecture without loading the SDK or
opening a port. A compatible SDK file is a prerequisite, not communication proof.

The Host rechecks missing state, all leases AVAILABLE, disconnected Worker status
and truthful cached Newport resource release before launching one fixed package
through Windows System32 msiexec/pnputil and explicit UAC. Files remain pinned
against modification. Cancellation, failure, reboot-required, no-change and
unconfirmed exit stay distinct. No forced downgrade or automatic reboot occurs.
Fresh metadata is required after installation; installer exit alone is not readiness.

## Offline development and packaging

~~~powershell
./App/scripts/test-native.ps1 -Offline
~~~

Use the existing x64 MSVC shell and locked workspace dependencies. The runner
includes all native/frontend and finite package tests; fixtures do not ship.
[Native development](../docs/development.md) documents PortableOnly and prepared
NSIS commands. Both package paths contain exactly GUI, Host, Worker, native
manifest/notices, catalog, Fiber configuration and the same approved driver resources.
No Python source/interpreter, test fixture, TLB bridge executable or source launcher
is bundled. Portable structural tests are not PE execution or installation evidence.

Current merged candidate packaging and physical acceptance are recorded separately.
Historical evidence in docs/acceptance.md and dated validation reports applies
only to its named source/artifact. Follow AGENTS.md for separately authorized
enumeration, read-only connection and reversible action; this merge runs none.
