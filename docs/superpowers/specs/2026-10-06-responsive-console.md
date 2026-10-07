# Responsive console

The user's priority is a fluent workflow with immediate feedback, few clicks and only necessary information. Continue on the assigned branch; another Rust migration is outside this change.

Observed causes: `replaceMarkup` destroys the complete live form whenever telemetry changes; `refresh-device` saves check policy and requests two snapshots; event callbacks synchronously perform every render, including bursts. The native laser status read takes roughly 2.7 seconds; its reviewed pacing and safety gates must remain.

Preserve same-page controls, focus, draft values, expanded details and scroll during telemetry updates. Apply changed text and eligibility attributes immediately; replace incompatible nodes or the page on navigation. Coalesce background rendering to one animation frame while user actions render their busy state immediately. Keep pending controls labelled with an animated indicator, suppress repeated submission and keep navigation responsive. Refresh now uses the already-saved policy without saving unsaved interval edits or adding an extra snapshot. Save interval remains explicit.

No hardware writes, bypasses, speculative operation replay, new dependencies, GUI backend choices, or extra approval dialogs. Retain authentic context, lease, unknown-outcome, cleanup and driver interlocks. Offline tests use finite injected transports; browser tests use an isolated headless test page without hardware access. Native desktop interaction cannot be claimed from browser acceptance.
