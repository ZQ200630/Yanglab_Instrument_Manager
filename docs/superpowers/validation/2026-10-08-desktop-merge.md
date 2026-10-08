# Desktop merge qualification — 2026-10-08

Status: native integration, both scoped fixes, final source review and replacement portable qualification passed. The authorized two-parent Git transaction follows this qualification; its receipt is retained separately.

The integration combines PIC `14e171a74403317e7a4adb0208270f30c3ea1c6d` and Laser `f65398258a46c64a946b240d302ea0f0bb9f754f`. The final merge must retain these exact tips as its two parents; both development branches remain intact.

## Production architecture

GUI, Host, Worker and instrument drivers run on the native Rust path. Tauri retains the static JavaScript/HTML/CSS interface. No Python interpreter, launcher, fallback, interpreter setting or Python payload is required by the production application, build or native test route. Historical Python experiments, reference helpers and offline tests remain source-only and excluded from the runtime/package.

The TLB driver is linked into Worker with one SDK owner thread and bounded typed commands. USB driver metadata uses read-only Windows SetupAPI/Configuration Manager and bounded PE inspection; metadata discovery does not load the SDK or enumerate instruments through it. Local and TLS clients require native implementation capabilities. Actual Worker verification and activation are reported separately.

The package contract is 29 files: three native executable images and 26 approved resources, including all three fixed vendor packages and their original 18 size/SHA256 pins. No TLB bridge, Python runtime, test fixture or credentials belong in the payload.

## Finite regression evidence

- 533 Rust tests passed with zero failed native test summaries.
- Final frontend: 366 Node tests passed with zero failures after the remote limits fix (the initial integration had 364 passes). Three bounded frontend browser fixtures were also reviewed.
- All 13 finite package groups passed under the same restricted Python-free PATH.
- Installer ownership fix covering: 17 tests passed after two finite red pin-loss regressions. Unknown launched processes retain file/process ownership; confirmed termination releases pins, and actual Host stop/management fencing was exercised.
- Ten PowerShell scripts parsed; original driver pins, native-only paths, resource agreement and source conflict/whitespace checks passed.

Full-run evidence is `Result/merge-desktop/task-4-native-full-final.log`. Its native and Node phases passed, but the process exited 1 when a package test used bare `pwsh` on the deliberately restricted PATH. Only that test fixture changed to the absolute current PowerShell executable. All 13 affected package groups then passed in `task-4-package-covering-final.log`; unchanged Rust/Node suites were not rerun and a whole-run exit 0 is not claimed. Finite structural MZ package fixtures do not qualify actual deployable images.

## Integration decisions

1. Preserve the original unknown-head stop-only controls (`Output(false)`, `Tracking(false)`, `ScanStop`) with explicit confirmation and unchanged preflight/fault rules. Tuning, starting and enabling remain rejected. If this decision is wrong, unknown heads retain those explicit stopping writes rather than being entirely write-disabled.
2. Fixed native Host capabilities describe its implementation, while actual Worker verification/activation remain separate evidence. A genuine native failed/retained startup stays reachable for disarmed management, with instrument actions fenced. If this decision is wrong, clients may attach before Worker verification and rely on the action fences.
3. Present-device/problem-28 installation prerequisites apply to CH340/CP210x. Preserve the original Newport exception for independently verified SDK absence, allowing explicit manual installation without an attached Newport device under all existing ownership, fresh metadata, disconnection and pin checks. If this decision is wrong, Settings offers Newport installation without an attached device when SDK absence is verified.

## Scope and remaining qualification

No real hardware enumeration, connection, output write, SDK load or driver installer was executed. No Python command or environment replacement occurred. The original checkout remained clean, and its user-owned Host was not connected, stopped or replaced. Actual native portable image/import/disarmed startup qualification passed. The GUI and Host were built and inspected without launching them; only the empty, unactivated packaged Worker was started. NSIS tooling is unavailable; no installer build, clean-Windows deployment or physical instrument qualification is claimed.
## Actual portable candidate

`Result/native-package/merge-rust-20261008-1f25b480` was built with locked offline Cargo and the existing x64 MSVC tools. Build exit 0; package checking passed all 29 files, 26 resources, three vendor packages and 18 original pins. No Python or fixture payload was present.

- Source identity: `tree-6eb747bacc1fd8eb8fa4478c96161c34b39c510c18c7e3e68c5400005e063d20`.
- Package revision: `0.1.0-6eb747bacc1f`.
- GUI SHA256: `ea1bde2d2a114c076ab51960f7b084bae3655ca6ab8324b503b8af33bf729d46`.
- Host SHA256: `d340a9ec6efb4ceb6cb0843ab0ab66b5ae25f571b89eec0e5fb2d1305008cde1`.
- Worker SHA256: `c4294cca2f5c4e97aa59dcc30739c5bca68d113732aa9490fc8911f167b112f2`.

MSVC dumpbin confirms AMD64 for all three actual images, with no Python, VCRUNTIME, MSVCP, UCRTBASE or api-ms-win-crt imports. The exact hash-verified packaged Worker returned the required native/protocol/startup/package/session/executable identity with activated=false, connected=false and zero domains on a System32-only PATH. No capture data was written; stderr was empty, EOF exit was confirmed with code 0. No activation or hardware enumeration was requested.

Build log: `Result/merge-desktop/native-candidate-final-build.log`. Package check: `native-package-final-check.json`. Raw PE header/import results and JSON are under the candidate's `evidence/pe`; raw startup reply and qualification are under `evidence/startup/ef3c4752f04a4a0a8d9cdf06bb061ae7`.

NSIS installer_built=false, installed=false, physical_validated=false and clean_windows_validated=false remain truthful. A current Windows WebView2 runtime and the applicable vendor dependencies are still necessary for their corresponding GUI/hardware functions; Python/Anaconda is not a production dependency.
## Final remote fix and review

The final review found remote Laser Save & reconnect interrupted the domain before the local-only configuration API rejected it. The owner Host's remote attachment now makes limits fields/save read-only with local-Host guidance, and the handler rejects before any stop/save/reacquisition/reconnect. TLS permissions were not broadened. Two finite red regressions preceded the correction; 6 focused, 88 covering and 366 final frontend tests passed. Supported local navigation and all three boot-loss cases remain covered. The final scoped rereview closed I1 with no new blocking findings.

An accidental `*.mjs` test glob invoked two argument-taking helpers without inputs and exited 1; no native runtime executed. The correct final `*.test.mjs` command exited 0 with 366/366 passes. Both outputs and exact commands remain in the review records. Existing compiler warnings are deferred, preserving declared Rust 1.88 compatibility; observed qualification used the installed Rust 1.99 toolchain.

The pre-fix candidate remains historical evidence, superseded by the replacement identified above. Final ZIP `Yanglab-Instruments-Rust-win64.zip` contains exactly the 29 approved payload files; each compressed entry's SHA256 matches the qualified portable source. ZIP size: 23,257,888 bytes; SHA256: `429f6f6309773ed232f1c31ba49561bb1a7d0a5589dab9b947f58cfc960298da`.