# Scan shortcuts and fresh Gain draft admission

## Result and scope

The Laser console now displays Full Scan (Start → Stop → Start), Forward Scan
(current → Stop), Backward Scan (current → Start), and Stop Scan (hold). Full
and Stop retain the existing native controller operations. Forward/Backward
remain unavailable in production until the exact controller, firmware and head
have a qualified single-pass motion contract; the UI states this limitation.

Each button has a small route description and shortcut hint. A folding Scan
shortcuts section offers an overall enable switch, editable key bindings,
Unassign and Reset. Only Escape Stop is assigned by default. Versioned local
storage persists the settings across reloads. Duplicate and browser-reserved
keys are rejected. A write failure preserves the previous active settings;
malformed or unavailable storage disables shortcuts and displays the cause
outside the closed folding section.

Shortcuts click the existing enabled button on the currently visible Laser
page. Background documents, other pages, modals, repeated/composition keys and
ordinary editing cannot trigger motion. Escape Stop can operate from numeric
Laser fields; shortcut capture fields consume only binding input and allow
Tab/Shift+Tab navigation. The standard lease, context and confirmation path
remains authoritative.

## Gain draft root cause

Host create_draft configured and bound the new Worker domain, but the Host
snapshot still used metadata cached before creation. A fresh Gain draft was
therefore published as UNKNOWN and the production connection guard refused
Connect. The previous fixture reused a pre-existing DISCONNECTED domain and
missed this case.

Creation now invalidates the metadata generation and waits for the bounded
Worker scheduler metadata refresh before returning. This status request does
not communicate with an instrument. If refresh fails, UNKNOWN remains blocked;
no connection or ownership guard is relaxed. The regression uses a genuinely
new draft while a sibling domain remains connected, then exercises the mounted
wizard through connect, verification and save.

## Native single-pass boundary

The [manufacturer-authored Rev D manual](https://manualzilla.com/doc/5650195/tlb-6700-user-manual-rev-d)
describes RESET movement to the programmed Start, but does not explicitly bind
RESET to either configured slew rate or its output blanking behavior. Stored
rate readback alone cannot establish a physical velocity ceiling. OPC completion
alone also cannot prove endpoint arrival after an interrupt.

The typed single-pass candidate checks fresh origin/target position, head and
operator limits, native maximum speed, configuration readbacks, and consent.
Its transport capability defaults false and is checked against freshly verified
identity before any single-pass query or write. The production SDK inherits
false; there is no profile, CLI, environment or operator bypass. Finite test
transports explicitly model a qualified contract for one fixture identity;
these tests do not qualify physical hardware.

Production enablement still requires separately authorized enumeration,
read-only identity validation, then reversible action qualification covering
both travel directions, rate, endpoint/OPC behavior, Stop hold, Tracking/Ready,
and emission/blanking. No such action was performed in this change.

## Verification

- Native suite: **554 Rust tests passed**.
- Frontend suite: **406 Node tests passed**.
- Packaging: **13 groups passed**.
- Production modules in isolated Edge: four controls, capability gating,
  default Escape Stop, enable/disable, capture/remap, persistence after reload,
  disabled-button dispatch, duplicate suppression, Tab navigation, visible
  corrupt-settings error, preserved numeric drafts and 800/960/1440 px layouts.
- Review findings closed: default-false native qualification fence, keyboard
  capture Tab navigation, and startup settings errors outside collapsed details.

The first combined runner passed all Rust tests but found a frontend pairing
Escape fixture whose target had no dataset. The handler now guards that optional
property; the full frontend suite and packaging stage subsequently passed.
Browser RED/GREEN checks reproduced both UI review findings before repair.

Evidence: Result/scan-shortcuts. The current App/Host/Worker and connected
instruments were preserved during implementation and qualification. No
serial/VISA/Newport diagnostic, setter or driver installation was performed.
Physical single-pass qualification is not claimed.

## Qualified portable candidate

The portable build is Result/native-package/scan-shortcuts-20261008-01/portable,
package revision `0.1.0-2a4a22e6bbb7`, source fingerprint
`tree-2a4a22e6bbb7ca6b184f65267887a9361b896b1757574c18b5fd63a0df268e7a`.
Package verification confirmed all 29 files, three driver packages and 18 driver
pins, with no Python payload or development fixtures. PE inspection confirmed
AMD64 executables with no Python or dynamic MSVC CRT imports. The exact packaged
Worker ran with only Windows System32 on PATH, reported native protocol 3,
startup revision 1, no activated/connected instruments and zero domains, then
released normally on EOF with exit code 0. This did not start another Host or
communicate with hardware.

SHA-256:

- GUI: `bebc0af7bdfe146a9fd3f06395b18b8e298319fc710d9968fd464e4e6aed6d47`
- Host: `014d51399f9b237b2aebd7dec6cbe69dc9f95296bea1001a38abe82bc0579bfe`
- Worker: `69ae7ded6ba51cdd5ad8f3f4bead40f57d6fc3db10ec8aa9e775ac968d504196`
- Yanglab-Instruments-Rust-win64.zip (23,310,747 bytes):
  `d7585fad95eee99295d91f1c5d34242c41f87612df3cee4d6967121202725284`

This is a portable build; no installer or clean-Windows physical qualification
is claimed.

## Authorized version switch

The operator requested the new version after the old GUI had exited normally.
Authenticated Host snapshot and Worker scheduler metadata both confirmed the
Laser domain DISCONNECTED, no connection ID, no resource responsibility and no
pending work. A completed release receipt reported no unreleased resources.
The formal Host stop confirmed resource release and successful Worker exit;
old Host PID 74580 and Worker PID 61192 exited normally. No process was
force-terminated. Physical zero was not asserted by the stop receipt.

The verified replacement GUI PID 65652 was launched from the portable directory
above. It automatically started packaged Host PID 39196 and Worker PID 68128
with the expected executable paths and parent chain. The GUI window title was
`Yang LAB INSTRUMENT CONSOLE`, its window handle was nonzero and it responded.
Authenticated IPC confirmed Rust Worker, protocol 3, verified startup,
confirmed activation and ONLINE Host. Instrument domains remained DISCONNECTED
without resource responsibility or pending requests. No instrument connection
was restored. No existing desktop or Start Menu shortcut targeted this App.

Evidence includes pre-switch-inspection.json, safe-stop.json, launch.json,
launched-processes.json and startup-inspection.json in Result/scan-shortcuts.
