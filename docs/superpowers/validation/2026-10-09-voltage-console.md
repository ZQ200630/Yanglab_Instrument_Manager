# Compact Voltage Source controls

The operator requested removal of routine lifecycle, completed-operation,
zero-evidence and sample-age banners. Each channel now places measured voltage
and current side by side, followed by a quiet trend and a distinct target editor.
The eight cards use four columns in wide windows and two in narrower windows.

All eight target fields reuse the existing per-digit selection and stepping
logic with two whole digits and three decimal places. Editing, arrow stepping,
blur, telemetry and navigation do not submit a command. Enter and Apply capture
the same channel and normalize the same draft before using the existing typed
set_channel operation. Pending requests remain deduplicated. The 0.001 V target
editing/display resolution does not claim 1 mV physical measurement accuracy.

Current values remain signed. Missing measurements are Unknown and stale,
disconnected or uncertain measurements are labeled previous. Real failures and
unknown outcomes remain visible. Ordinary completed activities are hidden on
connected and disconnected Gain/Voltage pages, while active or failed/unknown
connection work remains visible. Other instrument activity behavior is unchanged.

Drafts and trends bind to Host boot, Worker session and connection identity.
Changing identity clears old drafts and history; changing only a safety epoch
does not submit or discard a draft. This also prevents a new Worker clock from
being rejected against a prior connection's trend timestamp. Existing Resume,
normal-command, Zero and uncertain-outcome gates remain intact. Driver ramp,
voltage ceiling and connect/close zero behavior are unchanged.

Gain's standalone feedback row removal is included from bb6f25d. Candidate 09
was built but not activated; both UI changes will ship in combined candidate 10.
All validation uses finite injected transports or metadata-only Host inspection;
no live instrument command has been issued by the development workflow.

Final frontend regression passes all 508 tests. The six Voltage Chromium groups
cover every channel's digit selection, bounded stepping, draft-only edits,
Enter/Apply normalization, captured channel/value, repeated-key deduplication,
pending 5 Hz observations over 5.6 seconds, unchanged DOM/focus/caret/layout,
Zero access, explicit STOP_HELD recovery, stale/unknown outcomes and no replay.
Wide and narrow windows (1440/1281/960/800 px) preserve all target digits and
full signed ADC-boundary voltage/current readouts without overflow. The final
Gain Chromium regression passes all 20 groups; the unchanged Laser regression
and 13 packaging contract checks also pass.

Regression failures were captured before the specific fixes: connection clock
history reset, retained-clock offline status, and an actual mounted DOM that
kept 09.123 when a new connection had no requested target. The latter now clears
to blank and accepts a later fresh seed without submitting anything. Independent
source review finds no remaining P1/P2 issues. Driver/native source is unchanged;
the prior full native baseline is 722 passing tests, not a new run for this UI
change. No physical output validation is claimed.

Evidence: Result/voltage-ui/{frontend-green.log,browser-green.log,
browser-binding-red.log,browser-boundary-red.log,connection-edge-red.log,
offline-edge-red.log,status-edge-red.log,gain-control-green.log,
laser-control-green.log,package-tests-green.log,browser/}.

Combined candidate 10 is qualified at
Result/native-package/console-ui-20261009-10/portable:

- Package: 0.1.0-28753e96c232
- Source: tree-28753e96c2320b5d5e66d1d1ba386e5d439e4e36eff3dbd6f6a30f32f34236ce
- GUI: 5a9b4df921db4f4fa2032dd54fe37bb4f865e28fe28ea31ad37915ad1cc32075
- Host: 2147c4f3177f03ecb1d630b55172b225997a4b2cf5a9b5e262c006d9bb24db69
- Worker: 814c26edd8e0edf409e90e2e6f0c6ab5537361857b036d7752bed217b27c4caf
- ZIP: ec1280e82b4a6004b9855160b1a826c0ffbbefa4732d905c2ee8032e29ae0c75

The 29-file portable contract, approved resources/drivers, source fingerprint,
three x64 PE imports and every ZIP entry hash pass. Disarmed qualification runs
the exact packaged Worker with System32-only PATH: native protocol 3 identity,
not activated, no connected domains, clean EOF and exit code 0. It starts no
Host/GUI and performs no instrument operation. The initial relative-path package
check was rejected by the existing guard; the absolute-path check succeeds.
Evidence: build-candidate.log, package-check.log, pe/, portable-worker/,
portable-worker-check.log and zip-qualification.json under Result/voltage-ui/.

Readonly pre-switch inspection confirms candidate 08 still owns the live GUI,
Host and Worker. Voltage is STOP_HELD and connected; Gain and Laser are
disconnected. The operator has been asked to disconnect Voltage and normally
close the GUI. Activation is pending that resource release.
