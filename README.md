# Yang LAB INSTRUMENT CONSOLE

Development source for the Yang Lab instrument drivers and Windows console.

This is a development baseline, not a verified standalone installer. The App currently uses the Anaconda **VISA** environment. The private runtime and remote Host connection are still being implemented. Instrument entry points are real-only; offline tests use bounded, explicitly injected inputs, not a selectable instrument backend.

## Develop on another computer

```powershell
git clone https://github.com/ZQ200630/Yanglab_Instrument_Manager.git
cd Yanglab_Instrument_Manager
git switch codex/laser-1060-desktop
conda env list
```

If VISA does not already exist:

```powershell
conda env create -f environment.yml
```

Do not replace an existing environment automatically. See [Development environment](docs/development.md) for checks and native build prerequisites.

- **main**: shared driver, Host, data and UI framework.
- **codex/pic-desktop**: instrument work on the PIC desktop.
- **codex/laser-1060-desktop**: instrument work on the 1060 laser desktop.

The two development branches start from the same main snapshot. Commit and push on the appropriate branch; integration happens after the operator reports both sides ready.

## Layout

- `Code/Utils`: AQ6370 OSA, voltage source, Gain Driver, PM400 and MDT693B drivers.
- `Code/Setups`: coordinated lifecycle and laboratory-coordinate fiber setup.
- `Code/Debugs`: diagnostic scripts and offline safety tests.
- `Code/Experiments/<name>`: experiment code.
- `Result/<name>`: local generated data, not committed.
- `App`: native GUI/Host, Python worker, catalog and device-specific pages.

Read [AGENTS.md](AGENTS.md) before changing or operating devices. Preserve driver safety limits and staged diagnostic authorization. Voltage/Gain connection may perform safety writes; connection is not universally read-only.

Keep machine-local configuration, credentials and measurements out of Git. Reference archives and historical lab data are deliberately not included in this clean source history. The source baseline originates from local commit `cf98883f41355c6a95a8f0a055498c77d3e5f2f3`; unfinished local changes are excluded.
