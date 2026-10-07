# Laser-head selection and connection latency

Baseline `af989f55139eab068625e8f63e40c0fd673f7911`. The laser dropdown now displays the actual head model, a recognizable standard model-family wavelength where known, and the head serial. USB selection remains bound to the exact controller key. Examples: `780 nm · TLB-6712 · Head S/N H1`, `1060 nm · TLB-6722-P · Head S/N H2`. Unknown models retain their actual model and serial. Legacy identity-only replies clearly label the controller serial instead.

Wavelength family text is presentation-only, based on the [Newport Velocity datasheet](https://www.newport.com/mam/celum/celum_assets/resources/Velocity_Datasheet.pdf). It is not a measurement or a reviewed operating envelope for a suffixed/custom head. The user clarified that the actual model suffix is sufficient and no nickname is needed. The complete returned suffix is preserved; no meaning for `PC` is inferred from `-P`. Existing suffix-head read-only gating remains intact.

## Offline evidence

- RED tests reproduced missing native discovery, missing worker metadata, the old controller-only label and a retained proof after replacing a head on the same controller. GREEN: one SDK lifetime reads actual controller/head identities, closes once, performs no status/output requests, and blocks discovery while an owner is active. Malformed identity and failed release cannot publish partial successful results; failed release retains responsibility for explicit cleanup.
- Native request capacity remains 4 KiB. Native reply capacity is bounded at 16 KiB to support 32 validated identities. A 32-head finite reply larger than the old 4 KiB limit passes. Duplicate/malformed identities, arbitrary discovery commands, timeout and late-reply cleanup are covered; the original discovery is never replayed.
- Worker query/real-Host process fixtures use explicit finite discovery transports, pass through the production metadata validation and authenticate channel permissions; no instrument is opened by the tests.
- Independent review found one Important issue: `refreshDraftConnection` discarded previous scan metadata before its inner helper compared heads. RED reproduced a proof remaining saveable on the actual normal-refresh path. GREEN: previous metadata passes through the wrapper, and a replaced head invalidates the proof. One review and one fix pass were performed.
- Full VISA Python driver suite **1032/1032**; offline App Python suite **515/515**; native Rust driver/protocol **22/22**; frontend Node **254/254**; App Rust library **175/175**. Existing isolated Edge responsiveness acceptance passed; this finite browser fixture does not measure real native GUI connection latency. No additional dependency or selectable backend was introduced.

Evidence/logs are under ignored `Result/async-requests/head-*`. All Python commands use the Anaconda VISA executable. Output state, SDK pacing, shared resource ordering, leases, Host admission and cleanup protections remain unchanged.

## Connection timing

Source investigation found 13 ordered status reads with a configured 200 ms interval before each send: a minimum 2.6 seconds of pacing before device/SDK overhead. Frontend result polling runs every 250 ms; its 10-second snapshot timeout is an error deadline, not a fixed wait on every successful connection.

After the offline checks, separately authorized read-only diagnostics used the exact newly packaged native executable. Enumeration took **2.3224048 s**. Opening, verifying controller/head identity, reading full status and normally closing took **5.5940878 s**; the full status acquisition within that attempt took **2.6895815 s**. These stopwatch totals include native child startup and cleanup, not the user's full GUI click-to-display interval. SDK release was confirmed in both attempts. The controller returned serial `22500001`, firmware `2.4`, head `6722-P`, head serial `0953`, wavelength `1061.809 nm` and output enabled `false`. This is controller telemetry, not an independent optical measurement. No output, tuning, remote/local, tracking or reset command was sent.

The Host status poll/cache interval is **2500 ms** in `App/src-tauri/src/host/service.rs`. Ordinary terminal-operation callbacks do not immediately invalidate that cache, so publishing the completed connection state can add a delay of up to approximately 2.5 seconds. This pass measured the read-only components and identified that possible display delay; it did not change the Host cache or SDK pacing, and did not fully measure or resolve the reported ten-second GUI interval.

## Delivery

Before rebuild, an authenticated snapshot had no active instrument domains or held controls. GUI PID 20512 was normally closed. Authenticated Host stop separately confirmed SDK/resource release and successful worker exit (code 0); Host PID 53200 then exited normally. No real GUI, Host or native owner was force-killed.

The Host/native and GUI offline builds succeeded. Package verification found **40 matching resources**, with no retired resources. New artifact SHA-256 values:

- GUI: `6EF2ADC33991C3655A923FCF81B926006B68C640A70161C1ABE24FDB3EEF375F`
- Host: `84E40A264889432C690076CA735EDC1501F6227036E3B30EECE5CEC9D45AEE21`
- Native Newport worker: `5A29F8D0C277C33BAEC5D3D2EAB341437986A0A3277B95E4B14529DC3FCC75C4`

The new GUI PID 20636 and Host PID 16868 started normally and remained responsive. The authenticated background scan returned the real `6700 SN22500001` controller bound to `6722-P` / `0953` in **2.8885921 s**. Successful discovery is acknowledged only after the bridge validates release and the temporary native process exits normally; the final inventory had no `yang-lab-tlb` process. The observer session was closed normally. Registry domains and controls remained empty; no instrument record was added on the user's behalf.

The first bookkeeping probe mistakenly tried to read a private release field from the public Host status snapshot after the scan had already succeeded. Its observer session closed in `finally`. A corrected, explicitly fresh read-only probe verified the supported response and process evidence; the successful original operation was not replayed as timeout recovery.
