# Bundled CH340 and CP210x drivers

## Behavior

Voltage Source uses the bundled WCH CH340/CH341 package; Gain Driver uses the
bundled Silicon Labs CP210x package, which supports CP2102. Windows SetupAPI
metadata is read without opening serial ports. Present matching devices with
problem code 28 are missing; recognized healthy bindings are ready. Absent,
unrecognized and faulty devices do not offer installation. Composite CP210x
USBCCGP parent nodes are excluded; the actual MI serial interfaces determine
readiness.

Install driver appears in the selected model's Add New Instrument wizard.
Following the operator's UI feedback, Settings also offers an explicit Install
button beside a missing driver; startup does not prompt or install. Preparation
and connection verification recheck expired prerequisites. Late model-specific
replies cannot authorize a different model, and serial checks never scan Newport
controllers.

Before elevation, the native Host independently requires fresh missing-driver
metadata, no active calls and released instrument sessions/leases. Only fixed
compiled package IDs are admitted. All INF/CAT/SYS/DLL and license dependencies
are size/hash verified and held against write/delete during installation. Windows
System32 pnputil installs the fixed CH340 or CP210x INF; Newport retains its fixed
MSI. No forced downgrade or automatic reboot occurs. Cancellation, failure,
reboot-required and unconfirmed exit block readiness; a no-change result requires
the same fresh post-install metadata check as completion.

## Package provenance

- CH341SER 4.0, INF `02/11/2026,4.0.2026.02`: nine unchanged Windows 10/11 files
  from the official WCH ZIP. ZIP SHA256
  `59967D9CE371D0BF3DF02DEC0B66C8DFBF9CA576DA0572FF4404148A7C381807`.
- CP210x Universal Windows Driver 11.3.0: original INF, CAT, four architecture
  SYS files, license and release notes from an official Silicon Labs community
  attachment. ZIP SHA256
  `C4E9E7C631C1553B96539A19E58357FA0BEB04487ECFDD6E90B75AE783F08C46`.
  This supported pinned version is not the latest; the main-site latest download
  was unavailable. Unrelated registry update scripts are excluded. Original
  license bytes are bundled and displayed in the wizard before explicit install.

Vendor CAT/SYS/DLL Authenticode inspections reported Valid. Individual pinned
hashes and source links are recorded in `App/drivers/packages.json` and each
package's README. No vendor installer execution was performed. Before the GitHub
upload, the operator confirmed manufacturer authorization for public distribution
of the unchanged CP210x package. Original license and notices remain included.

## Review and validation

An independent read-only review found CP210x composite-parent handling, stale
missing-driver installation admission, expired serial preparation prerequisites,
and missing install-result flow coverage. All were corrected with bounded offline
regressions. Focused re-review reported no remaining actionable findings.

Final offline checks:

- VISA Python driver/protocol/lifecycle suite: 1057 passed.
- App Python suite against the rebuilt debug Host: 536 passed.
- App Rust library: 183 passed, including all 9 driver installation checks.
- Node frontend: 284 passed.
- Offline locked debug and release Host/GUI builds succeeded.
- Isolated directory package: 60 resources matched source; no obsolete resources.
- `git diff --check` passed.

The user normally disconnected/exited the GUI, then separately stopped the
remaining tray Host. Its exit was verified before rebuilding the existing debug
executables and running native process contracts. Tests used finite injected
transports and never displaced a lab owner. Test-owned Host processes exited
normally. The desktop was left closed; no automatic instrument connection or
driver installation was performed. Existing three native compiler warnings remain.

## Artifacts and limits

Directory package: `Result/console/serial-drivers-20261007`. Release artifact SHA256:

- GUI: `168C361BC0021302CF121957942A387828FCAB41251DFFD1C93D873620263D0B`
- Host: `FAD3201A7E18DF7CE398744DA66DA7D67E47F2B2D56F99EDDB402D0032BD7C70`
- Native TLB: `B51E80DE0F33E3FB7BF422468069EC3CDB4A94042E1236EB855DD430BB8962F0`

The existing debug executables were also updated:

- GUI: `189930E2B1CAC589A5688D34635E3CCC98C66BFBF854D5337DE5292486F3940D`
- Host: `5679C81FFEA0F286C4A63B125E42BAA6D133390D0BC56070A0508CAA6BBE3411`

Logs and original download evidence are retained under the ignored
`Result/console/driver-inputs` directory. This is a verified directory package,
not an NSIS installer or an installed release. The existing VISA Python worker
remains required. No real device was opened, no driver was installed and no
hardware output was changed in this task. Fresh-machine installation, native
Windows elevation UI and physical instrument qualification remain unperformed.

## Settings UI follow-up

The old large warning-styled Not installed span is replaced by aligned, compact
driver rows with purpose, a small readiness indicator and an explicit Install
button. An absent USB adapter says No device detected; an inventory error cannot
expose an installer. Refresh and additional installation are disabled while an
install job runs. The original CP210x license can be expanded in Settings.

Settings and the device wizard share the same installation/poll/fresh-readiness
routine. The native fixed-package, fresh-missing and released-session admission
guards remain unchanged. Independent read-only re-review found no actionable
issues. Complete Node regression: 287 passed. Isolated browser acceptance verified
aligned columns and a 325 px collapsed card at 960/1440 px, a direct CP210x install
through finite transport, progress, ready recheck, Windows cancellation and absent
device behavior. No real Host or installer was accessed by this browser check.

Offline release GUI rebuild succeeded. Candidate directory
`Result/console/driver-ui-20261007` matched all 60 resources and the unchanged Host
hash above. Latest GUI SHA256:
`44B5E13406DD583F94B91F3E66005A86B19BEFD9C3125B7276E3BA3EC5CE7235`.
After the user confirmed normal App exit, this verified GUI replaced the original
`App/src-tauri/target/debug/sil-instrument-console.exe`; its hash matched. Existing
Host and worker resources were not replaced. The App was left closed. Preview and
browser evidence are in `Result/console/driver-ui`, with test/build logs under
`Result/console/driver-inputs`.
