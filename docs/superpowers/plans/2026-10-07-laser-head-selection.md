# Laser-head selection and connection latency

User intent: choose an available laser by its actual head model and recognizable wavelength band, with a serial retained to distinguish identical heads. Investigate the reported approximately ten-second connection separately; asynchronous execution does not remove device communication time.

Bounded design: add one private read-only native `discover` operation, sharing a single SDK lifetime across all detected controllers. Reuse the existing strict identity checks, preserve serial binding and release responsibility, and do not perform status reads or output actions during discovery. Keep `enumerate()` and connection behavior compatible. Discovery metadata travels through the existing worker query/Host scan path. GUI label becomes Laser head; examples are `780 nm · TLB-6712` and `1060 nm · TLB-6722-P`, with serials. Wavelength labels describe a standard model family for selection only; they do not establish tuning limits, current wavelength, head suffix meaning or connector type. Unknown models display their exact identity without invented wavelength/PC information.

The additional identity reply must remain finite: at most 32 heads, validated bounded fields, native requests remain 4 KiB and native replies become bounded at 16 KiB. A failed release or malformed/lost acknowledgement must retain responsibility; no fallback or operation replay. Active sessions block a fresh discovery. Existing production query/admission, SDK pacing and safety limits stay unchanged.

Reference: Newport Velocity datasheet, https://www.newport.com/mam/celum/celum_assets/resources/Velocity_Datasheet.pdf (6712 765–781 nm, 6721 1030–1070 nm, 6722 1045–1085 nm). Family labels are presentation-only. The user confirmed that the actual model suffix is sufficient; preserve it without creating a nickname or inferring `PC` from `-P`.

- [x] RED/GREEN: two identity-bound heads in one SDK open/close; no status/output command, failed release responsibility and active-owner rejection.
- [x] RED/GREEN: bounded native acknowledgement and exact serial/head validation; timeout settlement without repeat discovery; worker scan propagation.
- [x] RED/GREEN: Laser head label, readable wavelength/model/serial options, distinct identical heads, unknown-head fallback and stale head-change proof invalidation.
- [x] Measure the authorized read-only connection path and report measured components without attributing unmeasured latency.
- [x] Offline driver/worker/native/frontend validation, one independent final review, package verification and normal disconnected restart.
- [x] Authorized enumeration and read-only discovery validation, preserving output/front-panel settings.

Publish the validated change on the assigned `codex/laser-1060-desktop` branch. Evidence and measured latency limits are recorded in `docs/superpowers/validation/2026-10-07-laser-head-selection.md`.
