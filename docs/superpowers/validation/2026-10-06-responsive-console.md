# Responsive console validation

Scope: the user clarified that fluent operation is the objective. This change fixes UI lifecycle and avoidable waits rather than adding another Rust migration. Baseline: `8941c54516bef498da6b17bd6179b41f500e052a`.

## Evidence

- RED: isolated real Edge acceptance lost input/select/details node identity and input focus during a pending-state update. GREEN: same controls, typed values, expanded details, scroll and a native select selection survive; fault and foreign ownership still disable actions immediately.
- RED: scheduler tests had no background coalescing API. GREEN: 100 notifications paint once using newest state; a click flushes feedback immediately and cancels the queued paint; close cancels callbacks.
- RED: Refresh now committed an edited interval; Save interval performed two synchronizations. GREEN: refresh never saves policy, invokes one refresh and one resync; save commits once and resyncs once without invoking instrument refresh. A delayed refresh permits navigation and suppresses duplicate submission.
- RED: connecting one OSA marked another OSA's Connect button busy. GREEN: pending presentation is scoped to the instrument/page.
- RED: inserting leading whitespace deleted a focused subtree. GREEN: text reconciliation cannot scan past and remove a live element.
- Independent review found one Important regression: managed checkboxes/selects retained dirty browser properties after their authoritative attributes changed. RED reproduced checked=true and controller=two when current authority required false/one. GREEN: managed checked/value properties now follow authoritative next state. Ordinary unsaved edits remain preserved. No remaining Critical/Important findings.
- Final Node suite: **251/251**; isolated Edge acceptance passed. Browser read click displays Reading trace and animated progress, can navigate while the finite transport waits, and clears progress after completion. A burst of 100 small state events including final paint took 2.7 ms in one local browser run; this is a bounded fixture measurement, not a desktop or hardware throughput guarantee.

Offline acceptance uses a private headless browser, fresh context, local test server and finite injected native transport. It never accesses the real Host or instruments. Commands and logs live in `Result/ui-responsiveness` (ignored). Set `YANG_LAB_PLAYWRIGHT_MODULE` to an installed `playwright/index.mjs` and `YANG_LAB_BROWSER` to Edge, then run `node App/tests/browser/responsiveness.mjs`. No runtime dependency was added.

## Boundaries

The Host, Python scheduler, native Rust driver, SDK pacing, leases, interlocks, cleanup and outcome-unknown fences are unchanged. Refresh now consumes the saved policy; it does not implicitly authorize periodic probes. The existing default policy for newly registered records is unchanged. The hardware's roughly 2.7-second full status read is separate from UI responsiveness.

Native desktop interaction was not qualified: the computer-use runtime exited unexpectedly. Browser acceptance is evidence for the shared frontend, not a claim that native window interaction was measured. No hardware setter or physical-output test was run.

## Delivery

Before rebuilding, an authenticated read-only Host snapshot showed the registered TLB domain DISCONNECTED and its control AVAILABLE. The previous GUI was closed normally; no Host, native hardware process, or GUI was force-killed. Rebuild and package/startup evidence is recorded after the final source changes.

Final GUI rebuild succeeded after the managed-control correction (three preexisting Rust warnings). Package check confirmed 40 matching resources. GUI SHA256: `D42455DF6A411879624819DB3C565415CFC8BC7133AF90A11BCAD3FDDC973BFD`; Host SHA256: `C0AA0F9134600FC52F19F50A627E0C04AAEB909994397EB66D81648D89033EB1`. New GUI PID 56496 was responding. Authenticated startup snapshot was healthy; TLB remained DISCONNECTED and control AVAILABLE. The existing Host was reused without restarting or replacing its running binary.
