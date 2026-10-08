# Device selection and direct actions

User-directed scope: routine actions execute from their named buttons without browser Yes/No dialogs. Results and failures appear inline. Connection and output effects belong next to their controls. Fiber baseline and nominal-conversion attestations remain separate inline checkboxes bound to the live connection; driver limits, identity proofs and leases remain mandatory.

Selecting TLB-6700 automatically checks prerequisites and scans controller serials. Choose among discovered serials; never infer identity from USB index. Re-scan preserves a selected serial only if still present. Selection changes invalidate verification. Successful prerequisite checks produce no banner. Missing dependencies produce a red message and Install driver button; unknown readiness and empty/failed scans are separate errors, not installation success.

The app packages the existing official Newport 5.0.8 x64 MSI with a fixed SHA256. A named Rust Host maintenance operation installs only that package through Windows UAC, never a user-selected executable. Installation requires all Host resources/leases released and blocks new admissions until its process exits. Exit 3010 reports reboot required without rebooting automatically. Only completed installation is followed by a fresh prerequisite check and scan. CH340/CP210x catalog and the full Rust instrument migration remain subsequent work.

Verification uses finite injected transports and production DOM/Host contracts; no hardware setters or maintenance installation on this already provisioned computer. The legacy close-guard test fixture is outside the production UI path.

Routine display cleanup: instrument cards show the chosen name, model and serial. The only refresh controls are interval and Refresh now. Hide Fiber setup entry without compatible registered controllers or configured setups. The add-dialog backdrop cancels with cleanup; successful verification lights Add & Save and opens the saved panel. No routine safe-stop/proof-TTL text. Five-second notifications dismiss on left click and copy on right click. Async actions show progress immediately and reject duplicate clicks quietly. Real unknown operation outcomes retain their recovery controls; normal disconnect progress does not display an uncertainty warning.

Settings shows concise driver installation state and reads metadata on entry. Unavailable remote features are hidden and Host maintenance controls are collapsed under Advanced.
