# Yang LAB INSTRUMENT CONSOLE

Windows instrument console with a native Rust driver → Worker → Host backend,
and an English Tauri GUI. The active runtime has no Python/Anaconda dependency.
This branch contains the Rust migration candidate; standalone clean-Windows
and physical acceptance are recorded separately, not implied by an offline pass.

## Develop

Install Rust MSVC (1.88+), Microsoft C++ Build Tools/Windows SDK, and Node.js.
From x64 Developer PowerShell:

```powershell
git clone https://github.com/ZQ200630/Yanglab_Instrument_Manager.git
cd Yanglab_Instrument_Manager
git switch codex/pic-desktop
./App/scripts/test-native.ps1
```

Use `codex/laser-1060-desktop` on the 1060 laser PC. Keep changes on the assigned
machine branch until the operator requests integration.
See [development](docs/development.md), [driver parity](docs/development/rust-driver-parity.md)
and [workspace safety rules](AGENTS.md).

## Layout

- `Code/Utils`: native OSA, Voltage Source, Gain, PM400 and MDT693B drivers.
- `Code/Setups`: laboratory-coordinate fiber coupling setup.
- `Code/Debugs`: staged native diagnostics and hardware-free tests.
- `Code/Experiments/<name>`, `Result/<name>`: experiment code and local data.
- `App`: native GUI, Host, Worker, protocol and trusted catalog.

OSA focuses on **Connect → Read trace → Save**, preserving front-panel
measurement settings and native samples. Settings configures one-time local and
remote connections; remote devices appear alongside local devices, and their
owning Host remains the only hardware owner.

Vendor VISA/GPIB/serial drivers are separate Windows dependencies.
All hardware operations retain driver safety limits and separately authorized
diagnostic stages. Read-only probes differ from Voltage/Gain normal connection,
which performs safety writes. Never bypass a driver or force-kill retained
resource responsibility.

Legacy Python files and dependency pins are preserved only for reference.
They are not an alternate backend, production build input or automatic migration
path. Measurements, credentials, local device bindings and build outputs stay
out of Git.
