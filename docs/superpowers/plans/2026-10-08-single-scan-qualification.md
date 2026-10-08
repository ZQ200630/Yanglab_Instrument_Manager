# Native single scan qualification plan

> Execute inline with superpowers:executing-plans. Keep hardware stages separately authorized.

**Goal:** Enable Forward (current → Stop) and Backward (current → Start) only after establishing the real controller's bounded slew and hold contract.

**Architecture:** Keep the production capability gate closed during investigation. Add a development-only native diagnostic in Code/Debugs that uses the Code/Utils driver, exact identity checks and existing ownership guards. An explicitly authorized small RESET probe records telemetry; it cannot enable production capability. Review real trajectories before introducing an identity-specific compiled qualification.

**Tech stack:** Rust, locked Cargo dependencies, existing Newport SDK owner, finite injected offline transport tests.

**Spec:** User's Full/Forward/Backward/Stop semantics and AGENTS.md hardware diagnostic staging. Manufacturer TLB-6700 Rev D pp. 61–62 and 68–74 describe RESET to Start, but omit its slew/blanking contract. The Vortex manual on Newport's current site is a different head application and does not supply that missing contract.

## Constraints

- No actual hardware I/O until each applicable stage is authorized; never replace the current resource owner.
- No raw SDK/SCPI from diagnostic or experiment code. All writes pass the reusable driver's checks.
- Preserve emission and scan blanking settings; never enable output to run a diagnostic.
- Probe identity is exact including firmware and controller/head serials. Displacement ≤0.25 nm, requested speed 0.05 or 0.10 nm/s, within existing operator/head limits. Unknown RESET rate must still be covered by the reported hardware maximum and operator ceiling.
- Default CLI invocation previews only; action requires explicit confirmation. Diagnostic API is absent from App/RPC routes and cannot claim a qualified production profile.
- Cleanup release and observed motion/output evidence are separate. Faulted exchanges are not replayed.

## Review focus

- Changed controller/head or a moved origin must reject before writes.
- A lower operator speed ceiling must reject an unknown-rate probe before writes.
- OPC alone must not be interpreted as endpoint arrival.
- Failed/uncertain writes must not trigger replay or an automatic return movement.
- An active App/Host must prevent acquiring the diagnostic owner.

## Tasks

- [x] Add failing native driver tests for bounded, independently authorized probes and unchanged production capability.
- [x] Implement a narrow probe API and scan-setting readout inside Code/Utils/tlb_native; no new serialized control operation.
- [x] Add staged native diagnostic preview/authorization tests and CLI in Code/Debugs. Record before/trajectory/after and explicit cleanup receipts.
- [x] Run mandatory native offline regression and preview an enumeration plan without opening hardware.
- [ ] Obtain enumeration authorization, then read-only authorization, then concrete bounded motion authorization based on the fresh read-only result.
- [ ] Qualify both directions at two rates, endpoint hold, and an interrupted move. Do not infer speed, blanking or physical measurement from an ACK.
- [ ] If real evidence establishes the contract, add compiled identity-specific capability, forward it through the Worker wire wrapper, rerun offline/UI regression, build and safely switch the App.

## Current state

Forward/Backward production capability remains false. No real probe has run. The currently connected App owns the laser and must release it normally before diagnostic execution.

The user's subsequent Stop Scan report takes priority for the next package: STOP was acknowledged but readback remained tracking-on / OPC false, leaving the UI's motion lock active. Stop now explicitly sends scan STOP then motor tracking-off, preserving emission, position and the user's future target-following preference. A fresh completed motion readback releases the existing editor lock. Native Worker regression covers Stop while busy followed by a successful manual wavelength command; no real Stop has been issued by the agent.

Read-only code review found and resolved two diagnostic evidence issues: verified START may not expand the approved 0.25 nm span, and hold evidence must contain multiple samples conservatively spanning a full second even with slow getters. Both fixes have regression tests. Diagnostic preparation does not enable production single-pass scanning.
