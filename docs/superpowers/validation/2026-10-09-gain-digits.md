# Gain fixed digit targets

The operator requested the same airport-board-like digit editing already used
for laser wavelength on the Gain Temperature and Current targets. This is a
bounded reuse of the existing fixed-digit editor, not a new command path.

Temperature displays two whole digits and three fractional digits (25.000 °C),
and current displays three whole digits and three fractional digits (003.000 mA).
A first pointer click selects that exact digit, Left/Right skips the separator,
Up/Down adjusts the selected place with carry, and digit replacement advances.
Raw typing and paste remain editable drafts. Valid values normalize on blur or
Enter; Enter and Apply capture the same three-decimal value. Tab and Shift+Tab
continue cycling between the two targets. Existing 15–40 °C and 0–200 mA bounds,
pending-command gates, current ramps, output toggles and thermal interlocks remain
unchanged. PID and ramp-configuration controls retain their existing behavior.

Programmatic digit changes use the same Gain draft tracking as typed input, so
5 Hz telemetry cannot replace an edited target or its selected slot. Clean seeds
use the same fixed format as rendered values and restored inputs. Decimal-place
navigation and clean blur do not create a dirty draft. Partial and empty drafts
remain intact, composition events are ignored, and a replacement connection
invalidates old drafts without sending them to hardware.

## Verification

Unit RED captured the original number input and unformatted clean seeds;
Chromium RED captured the original input type. An additional real Chromium RED
proved Enter retained raw 26.5004 while blur/Apply rounded to 26.500. The shared
normalization rule fixes both paths without moving focus or changing dispatch.

Focused Node tests pass 42/42. The full offline wrapper passes all 722 Rust,
493 frontend and 13 packaging checks. Three existing clean-seed assertions now
expect 24.000; their dirty-draft, freshness and connection-invalidation checks
remain intact. Real Chromium passes all 18 Gain groups, covering
actual first pointer selection of every digit, carry, decimal skip, replacement,
composition, 5 Hz draft/focus/selection preservation, navigation, Off epochs,
replacement contexts, bounds, precision equivalence and pending Enter deduping.
The existing laser Chromium suite also passes, including shared digit handling
and layouts at 800/960/1440 px. Root inspected the rendered fixed-digit dashboard.
Independent source review finds no P1/P2. All browser work uses finite injected
typed clients; it cannot access the lab Host, serial ports or outputs.

Evidence: Result/gain-digits/{unit-red.log,unit-green.log,browser-red.log,
browser-precision-red.log,browser-green.log,laser-browser-green.log,
fixed-digit-targets.png,control-panels-keyboard.png,full-native-green.log}.

## Qualified delivery candidate

Candidate 07 is staged at Result/native-package/gain-console-20261009-07/portable.
Its identity is 0.1.0-2e92f9c080b4, with source revision
tree-2e92f9c080b4d366b457a36b5a8292dbee5133be7d0961dfa0a6687466c16bfd.
The separate package check confirms the closed 29-file contract, three driver
packages and 18 pinned files. All three binaries are AMD64 PE with static CRT
and no Python imports. The exact packaged Worker passes disarmed ping and EOF
exit 0 with a System32-only PATH, no activation, connections or Host startup.
The ZIP contains the same 29 files with every entry hash verified; SHA256 is
d82057151bf2d580bea72dae62bc2aca49a64b4f788fca1cf8ae64c0a25edfc9.

GUI SHA256: dd34f304cdd9d9aa5e0b55f51357413366d25a997f7579499413d637fd95e9dd

Host SHA256: 2147c4f3177f03ecb1d630b55172b225997a4b2cf5a9b5e262c006d9bb24db69

Worker SHA256: a466573768d82661762cd424977d939fb311465eae5af364573409412020f559

Qualification evidence: Result/gain-digits/{build-candidate.log,
package-check.log,pe/pe-qualification.json,portable-worker-check.log,
zip-qualification.json}. No physical output validation was performed.

At staging, read-only Host inspection confirms candidate 06 remains online,
Laser disconnected and Gain connected to COM4 with no pending requests. The
operator has been asked to disconnect and exit normally before the switch;
candidate 06 has not been stopped or modified during this UI change.
