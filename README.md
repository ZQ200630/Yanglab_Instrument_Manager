# Yang LAB INSTRUMENT CONSOLE

Windows instrument console with native Rust drivers, Worker and Host, an English
Tauri GUI, and static JavaScript/HTML/CSS views. The active App has no Python or
Anaconda backend, interpreter setting, launcher or fallback.

The integrated source retains PIC instrument, Fiber, archive and remote/TLS
features alongside the Newport/New Focus TLB-6700 Laser panel and fixed USB
driver installation jobs. Source verification, portable packaging, installation
and physical acceptance are separate evidence; an offline pass proves no
physical output or clean-Windows installation.

## Develop

Use installed Rust MSVC 1.88+, Microsoft C++ Build Tools/Windows SDK, Node.js,
PowerShell 7 and WebView2. From an x64 Developer PowerShell in this repository:

~~~powershell
./App/scripts/test-native.ps1 -Offline
~~~

Offline mode requires the reviewed Cargo.lock dependencies already cached.
The runner builds native finite fixture binaries, runs locked workspace Rust
tests, frontend Node tests and finite package tests with Python absent from PATH.
It never selects a simulated instrument backend or opens instruments.

See [native development](docs/development.md), [App usage](App/README.md),
[TLB-6700](docs/tlb6700.md), [driver parity](docs/development/rust-driver-parity.md)
and [workspace safety rules](AGENTS.md). The existing PIC and Laser development
branches are preserved; use the operator's requested branch for ongoing work.

## Layout

- Code/Utils: native OSA, Voltage Source, Gain, PM400 and MDT693B drivers, USB prerequisite metadata and the linked Rust TLB library.
- Code/Setups: laboratory-coordinate Fiber setup.
- Code/Debugs: staged native diagnostics and finite transport fixtures.
- Code/Experiments/<name> and Result/<name>: experiments and local generated data.
- App: native GUI, Host, Worker, protocol, trusted catalog and fixed vendor packages.

OSA supports Connect → Read trace → Save while preserving front-panel settings
and native samples. Settings manages local and paired remote Hosts; the owning
Host remains the sole instrument owner. A legacy real/protocol-3 Python Host is
incompatible with the new native client and is never stopped or replaced
automatically. Failed native startup remains reachable for disarmed recovery.

Vendor VISA/GPIB, Newport SDK and USB/serial drivers are independent Windows
dependencies. All three bundled driver packages retain original byte pins and
provenance/licenses; driver installation requires a fresh positive missing check,
confirmed resource release and an explicit operator action with Windows elevation.
No driver is installed at startup or App installation.

Preserved Python source, tests and dependency pins are historical references,
excluded from runtime payloads and active build/test commands. Never auto-run
their hardware or experiment entry points. Credentials, local bindings,
measurements and generated native artifacts stay out of Git.
