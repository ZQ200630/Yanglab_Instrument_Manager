# TLB laser control and scanning

The approved UI is a compact Laser Status card, a full-width Control card with one state-labelled output button, scanning start/stop/speed, wavelength setting, collapsed fine tuning, and device information last. No ordinary Remote/Local or confirmation dialogs.

Use the published Velocity model table automatically, accepting the documented -P fiber-coupling option only. Preserve the complete returned head identity. Unknown/custom suffixes retain read-only access. Scan speed is bounded by both published model speed and a fresh MAXVEL query. Do not set current or power limits from a marketing table.

Typed composite controls validate before writing, take Remote for the requested command, and return Local after acknowledgement. The legacy wavelength movement enables motor tracking; visible Target edits preserve Tracking/Ready. Do not return Local or replay a write after a protocol fault. Connect, identity, status and normal close stay preserving and read-only.

Native scanning is one cycle: set start/stop/forward speed, return speed capped by the same effective speed ceiling and cycle count 1; verify settings before START. The instrument moves to Start rapidly, scans to Stop and returns to Start. STOP stops motion; it does not reset to Start or disable emission. Start never enables output. Preserve reverse-scan blanking. Never infer precise scan position from a UI timer.

Foreground periodic read-only refresh reflects front-panel changes; it must not overlap another request or run after authority/connection loss. Inputs and expanded details survive telemetry renders. Faults remain visible. Controls remain gated by known recent readback, owner authority and native fresh preflight.

Real output/tuning/scanning qualification is a separately authorized diagnostic; this change is tested with bounded injected transports only.

Two layers of limits: the automatic published hardware envelope cannot be expanded. Saved operator limits may narrow wavelength range and lower maximum speed; both legacy and composite tuning enforce them. All scan legs, including travel to Start and return, obey the effective speed ceiling. Limits are persisted atomically and applied through preserving disconnect/reconnect. Serialize saving with connection admission; after a failed activation, block that domain until Host restart reloads the saved configuration.

Latest steering: Target Wavelength has aligned fixed digits; Left/Right select a digit and Up/Down change it. Valid edits send typed target updates automatically, with one active request and only the newest queued value. Tracking state is preserved, matching Ready/Track semantics; no implicit Tracking On. Place Tracking On/Off beside Target and remove Set wavelength. Target is grey throughout scanning and follows actual stop readback after completion. Start/Stop wavelength, Forward Velocity and Backward Velocity are separate inputs; backward speed controls both approach and return. Scope pending edits to the original boot, lease and connection; discard them on authority/context loss. Native busy checks are preserved. Continuous edits during motor movement wait for ordered read-only completion before the next target command.

Latest latency policy: Target/Piezo/Start/Stop publish ACK without full post-readback. Observe motion separately with completion checks before/after wavelength. Full output/power/current evidence keeps its original timestamp. Strict identity/envelope and prestart five-setting verification remain. Velocity inputs are local drafts until verified Start. Faster initial getter pacing is read-only qualified; setters/retries retain 200 ms and no setter replay.
