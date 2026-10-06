> Operator priority update (2026-10-06): publish the current development source first for parallel work on the two lab computers. Use proportionate targeted tests; defer exhaustive package hardening. Earlier publication gates below no longer block source synchronization, but standalone-install, hardware and remote acceptance are still unproven. Do not restart completed tasks or imply these gates have passed.

> Later operator direction (2026-10-06): choose gradual migration to an all-Rust
> instrument execution path instead of an app-private Python runtime. Preserve
> the verified ownership and driver safety boundaries. The private-runtime
> design below is historical context; do not continue its launch integration
> as the selected deployment architecture. This does not claim the Rust drivers
> or Python-free desktop have been implemented.

# Private Windows Runtime and Common-Framework Release

Status: the operator approved this written specification and its detailed implementation plan, choosing inline execution on 2026-10-06. The previously approved real-only/telemetry work remains independent. This document does not claim implementation or clean-machine acceptance.

## Intent and boundaries

An ordinary lab user installs Yang LAB INSTRUMENT CONSOLE and launches it without installing Python, Anaconda, pip, Rust, Node or development tools, activating an environment, or entering an interpreter path. Every hardware-owning computer still runs one Rust Host and one real-only Python worker. A GUI may observe explicitly paired remote Hosts; the local/remote device architecture and driver safety policies are unchanged.

This is an architectural deployment increment, not a rewrite of drivers or experiments. Source development and ordinary offline regression continue in Anaconda VISA. Explicit qualification of the shipped runtime runs that private executable: qualifying it through external Anaconda would not prove independence. Installed instrument actions remain staged and driver-mediated; installation and initial startup never command outputs, run experiments, or enable a previously unused probe policy.

The user chose an app-private CPython runtime. A frozen worker executable was considered; it would require adapting executable/package identity, module discovery and data-path assumptions. A complete copied Anaconda environment was also considered; it would add unrelated development packages and require a distinct redistribution audit. Neither alternative is the selected deployment path.

## Verified starting state

- GUI preferences, Host settings and Host startup arguments currently carry an external Python path, with a machine-specific Anaconda default.
- Runtime admission checks the interpreter's parent directory and handshake environment name against VISA. Removing only the Settings input would not remove this dependency.
- The installer currently includes Python source, catalog/configuration resources and the native Host sidecar, but no interpreter or locked Python dependency payload.
- The worker imports NumPy, PyVISA, PySerial and PyMeasure. PyMeasure has transitive dependencies; the whole environment.yml is not the installed App's minimal runtime inventory. Matplotlib/Pillow used by diagnostics or icon generation do not automatically belong in the worker payload.
- Current tests use CPython3.10.16 in Anaconda VISA. Shipping that environment is not justified by existing tests alone; the private runtime and binary wheels need independent qualification.
- Origin is https://github.com/ZQ200630/Yanglab_Instrument_Manager.git. Connector metadata reports default main, no branches, public visibility and write permission. No files have been uploaded. The bundled Git currently lacks its HTTPS helper; the repository connection is not proof that native Git transport works.
- The local tracked reference/configuration/result subset contains111 files totaling396457498 bytes, including original reference archives, compiled executables and laboratory spectra. It must not be blindly published with development history.

## Package boundary and reproducible assembly

The installation contains the GUI executable, its exact Host sidecar, driver/setup/worker/catalog resources and runtime/python.exe with its standard library, required native libraries and vendored third-party packages. Use the official CPython Windows x64 embeddable distribution. The implementation plan pins an exact supported CPython release after checking wheel compatibility; the first candidate family is3.13, not an automatic upgrade to whatever python.org calls latest. A failed compatibility check blocks that candidate and is recorded; it does not silently fall back to external Python.

Build inputs are an exact interpreter archive and a complete hash-locked Windows x64 wheel dependency closure. Resolve/install wheels during controlled build staging, not by running pip on the end user's machine. Include package/native-library license notices and redistribution provenance. Do not copy credentials, test fixtures, device emulators, unrelated site-packages, build caches, or the development environment into the payload. Dynamic imports, NumPy native DLLs and PyMeasure's actual headless import closure must be exercised rather than guessed away.

The published source also contains requirements.txt, environment.yml and development instructions. They specify the existing Anaconda VISA development baseline (Python3.10.16 and reviewed exact direct-package versions), creation/verification commands, offline regression commands and separately installed vendor dependencies. A semantic test keeps both development dependency lists aligned. These development files are distinct from the installed-runtime hash lock; they do not claim to lock every transitive development dependency or install a native VISA/USB driver. Do not overwrite an existing VISA environment automatically.

Use an explicit private ._pth/module search configuration and vetted package layout. Installed launch ignores external PYTHONHOME, PYTHONPATH, user site-packages and Python registry/default-interpreter discovery. Never invoke bare python/py/pip or download packages during ordinary startup. Keep the worker in a child process with stdin/stdout protocol framing and hidden console; do not embed Python into the GUI process or weaken lifecycle supervision.

Package manifest version1 records build ID, source revision, target OS/architecture, exact interpreter/package versions, file sizes and SHA256 hashes. The native build binds to the expected package-manifest identity. Validate containment, required files and expected executable/package identity before activation. Hashes demonstrate byte agreement; they are not a substitute for publisher signing or protection against an attacker who can replace the entire application. Damaged/mixed/incomplete packages fail visibly, never use a source checkout or installed interpreter as a fallback.

Default target is Windows x64; other architectures are unsupported until separately built and qualified. Bundle the Microsoft prerequisites necessary for this payload, including an offline WebView2 installer through the existing Tauri packaging path. Preserve official redistributable license/install behavior. Package size is reported after building; no small-download promise precedes measurement.

## Launch, identity and settings migration

Installed GUI startup resolves Host, interpreter and worker resources from its own verified installation. Frontend requests cannot supply an executable, Python path, module search directory, or arbitrary launch command. Host independently checks its runtime descriptor before spawning; a direct Host launch cannot bypass the package boundary.

The worker handshake identifies protocol, real-only source, exact executable/package/build identity, disarmed/empty startup and existing session/ownership nonce. Directory name VISA is no longer an installed-runtime trust test. Development launch remains explicitly distinct and retains the project's VISA rule; it is not selectable in the installed GUI. Do not invent a VISA environment name for the shipped interpreter merely to satisfy an old check.

Maintain the machine-wide physical owner guard, current-user local IPC authentication, durable child identity/ownership record, bounded request/reply queues, separate safety lanes and retained-resource handling. No interpreter migration replays driver calls, replaces an unknown owner, claims a crashed worker released its resources, or restores output/motion authority. Compatibility checks distinguish old and new packages; an incompatible running Host requires normal verified shutdown, not automatic kill-and-restart.

Settings removes the Python executable field. Keep Host name, recording destination, remote pairing and actionable runtime/device-driver diagnostics. The installed runtime version/build appears read-only in Diagnostics, not as another normal operation button.

Version preference and Host-registry migrations. Back up original bytes atomically before replacing valid legacy schemas. Preserve devices, names, identities, revision-bound checks, selected data root and unrelated settings. An obsolete external interpreter path is historical metadata only and cannot influence installed launch. Invalid/unsupported records remain visible and block the affected automatic action rather than being silently discarded. No migration grants old command/verification authority or relabels historic synthetic evidence as real.

## Native instrument dependencies

Bundled Python libraries and Windows instrument drivers are different prerequisites. Native VISA implementations, a GPIB adapter's driver and CH340/CP210x/other USB drivers may require vendor installation and administrator approval. Do not promise that an arbitrary instrument can connect merely because the App starts.

The App starts with no instruments and no native VISA backend installed. Missing backend/driver health is a specific unavailable capability, not a reason to demand Anaconda or crash every unrelated serial device. Health diagnostics inspect package/native-dependency metadata first; instrument enumeration, resource opening and active testing retain the existing staged authorization boundaries. A launch check never opens an unknown USB/serial device or sends *IDN?.

Do not redistribute proprietary VISA/adapter installers without confirmed permission, accept license terms for the user, switch a system's preferred VISA, modify the global Python/PATH configuration, or change Tailscale accounts/routes. Offer precise missing-driver guidance and official installation sources. Local hardware prerequisites apply to the instrument-owning Host; observing a remote device must not demand that instrument's local USB/VISA driver on the receiving GUI computer.

## Update, failure and data ownership

An installer upgrade must coordinate with the real Host's existing orderly stop/release evidence before replacing running resources. If cleanup is retained or an unknown owner exists, defer the upgrade; do not overwrite/kill it. Use a separate explicit candidate installation for qualification and verify exact destination/registration before running NSIS. The previous installed-console destination incident must not be repeated.

Keep mutable settings, credentials, recording spools and Result data outside the immutable runtime. Upgrade does not delete data, alter front-panel settings or silently resume recording/outputs. Runtime files are replaced as one compatible versioned package, not partially patched by pip. Corruption has a concise repair/reinstall error. Vendor-driver health, worker package startup failure, retained physical ownership and actual hardware communication are separate statuses.

Uninstall removes only the application payload; persistent instrument configuration/results are preserved by default. Credential removal and device/peer revocation need a clearly disclosed explicit choice. This increment does not silently revoke another host or delete experiment data.

## Qualification and release gates

1. Existing offline Python/Node/Rust regressions pass in their specified development environments. New tests exercise installed/private launch without external-Python options, unsafe environment injection, missing/altered package files, incompatible worker identity, migration preservation, retained owner and orderly shutdown. Use finite transport doubles at test boundaries; they cannot become an instrument backend or user-visible measurement.
2. The built candidate's exact installer/runtime/Host/GUI/resource inputs, version manifest, SHA256 and license inventory are verified against the qualified source. A build-only result is not clean installation or native GUI acceptance.
3. On an actual isolated Windows x64 computer or fresh Windows VM without Python/Anaconda/development tools, install the candidate, launch as an ordinary user, verify the native GUI and its single Host/private worker, close/reopen, persist configuration and perform safe orderly stop. Repeat with external Python-like environment variables present and confirm they are ignored. No real hardware is opened for this launch qualification. A PATH-hiding experiment on this development computer is an auxiliary test only, not this gate.
4. The clean target also lacks native VISA initially: show truthful missing-backend guidance while the App remains usable. Offline installer/prerequisite behavior is verified with network unavailable. Hardware-only functions remain unavailable until appropriate drivers/devices exist. Later authorized known-device tests prove actual acquisition/storage separately; clean launch does not count as a hardware pass.
5. Upgrade/reinstall on that isolated target preserves settings and data and rejects a retained/active owner correctly. Verify no external Python/pip, development source path or runtime download was used. Record OS architecture/build, installed prerequisites, candidate hashes, commands/process identity, screenshots/native interaction and outcomes in Result/console and docs/acceptance.md. No available clean target means this gate is pending, never fabricated.
6. Complete the rest of the common-bottom-layer acceptance: real-only drivers/pages/data and TLS/multi-Host safety/routing. A clean runtime does not establish remote-network acceptance or make unresolved Gain/absent PM400/MDT hardware pass.
7. Before publication, audit the exact source/history for private lab results, machine-local credentials/settings, generated binaries, caches and third-party redistribution. No automatic publication of the existing raw history: if excluded laboratory data is in it, produce a clean, traceable common-source export in an isolated publication repository, retaining the original local history unchanged. Do not delete/rewrite reference_code or unrelated user changes to achieve this. Audit/license evidence precedes the first push, and secrets never appear in logs.
8. After all applicable gates, publish the verified common framework to main in the operator's target repository with normal non-force semantics. Verify the remote main commit/tree, then create codex/pic-desktop and codex/laser-1060-desktop from that SAME commit. Verify both remote refs and provide clone/setup guidance. Do not create empty placeholder branches from an incomplete baseline or count connector write permission as a successful upload. If native Git transport needs installation/configuration, disclose and resolve that deployment prerequisite separately without replacing credentials or global Git settings silently.

The two machine branches contain instrument-specific development/tests, not another copy of the common framework. Machine-local addresses, credentials and generated data stay local. The operator will report when both branches are ready; later review/merge is a separate requested integration, not automatic during branch creation.

## Primary references and self-review

- Python Windows embeddable-package documentation: https://docs.python.org/3/using/windows.html#the-embeddable-package (private runtime isolation; dependencies vendored by application, not managed by end-user pip).
- PyVISA installation documentation: https://pyvisa.readthedocs.io/en/latest/introduction/getting.html (Python library and native VISA backend are separate requirements).
- Tauri Windows-installer documentation: https://v2.tauri.app/distribute/windows-installer/ (offline WebView2 prerequisite packaging).
- PyInstaller operating-mode documentation: https://pyinstaller.org/en/stable/operating-mode.html (reviewed alternative, not selected implementation).

Self-review: this deployment scope changes only installed runtime selection/packaging/readiness, not driver limits or transport authority. Development VISA and direct shipped-runtime qualification are explicitly distinguished. Package, clean native Windows, physical instrument and remote-network gates cannot substitute for each other. Publication remains conditional; public-source export never rewrites the retained original laboratory history. Exact interpreter/wheel/build inputs are pinned in the implementation plan before assembly, not resolved on end-user launch.
