# Gain status inside the existing panel

The operator requested removal of the standalone finished/outputs feedback row
introduced by c574cc4. The dashboard now begins with Temperature trend and
Overall status. A compact footer inside Overall status carries pending readback,
native operation phase/progress and real failure details. Ordinary successful
completion does not show Operation finished or its elapsed timer.

Activity labels reuse the existing shared formatter. Active work keeps its real
elapsed timer; failed and unknown terminal work remain warnings even with READY
driver state and fresh fields. Native operation progress does not depend on a
local activity. Evidence, authority, ramps, output toggles and Off admission are
unchanged. Long faults are not clipped. The note and progress reserve 66 px in
the status card so routine readback/operation transitions do not move controls.

Updated existing Chromium assertions first failed on the standalone row. All
497 frontend tests and 13 packaging checks pass; 85 focused tests, the 20 Gain
Chromium groups and existing laser suite also pass. Finite 800/960/1440 px probes
each publish 32 observations at 200 ms, including a wait longer than five seconds:
monitor, controls, footer and both inputs move 0 px; footer stays 66 px; there is
no overflow, independent status row or success timer. Input node identity, draft,
caret, focus and Off remain intact. Genuine faults remain visible. Independent
review finds no new P1/P2 and root inspected the healthy 1440 px screenshot.

Evidence: Result/gain-compact/{unit-green.log,frontend-green.log,
package-tests-green.log,browser-red.log,browser-green.log,laser-browser-green.log,
responsive-probe.json,responsive-1440-healthy.png}. All clients are finite injected
transports. No live instrument command was issued.

Candidate 09 built successfully but was not activated. The operator then added
the Voltage Source UI request. Gain and Voltage changes will be qualified and
delivered together; candidate 08 remains the live application during source work.
