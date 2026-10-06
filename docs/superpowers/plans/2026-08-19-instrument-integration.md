# Instrument Driver Integration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Integrate the three independently tested drivers into a safe combined health check and complete offline and real-device acceptance evidence without adding experiment logic.

**Architecture:** `check_all.py` orchestrates existing diagnostic APIs in the safe order OSA, Voltage Source, then Gain Driver. It defaults to identity/read/startup-zero checks and delegates output-changing diagnostics only after explicit confirmation.

**Tech Stack:** Python 3.10, existing `Code.Utils` drivers, standard-library `argparse`, `logging`, and `unittest`, Anaconda environment `VISA`.

**Spec:** `docs/superpowers/specs/2026-08-19-instrument-drivers-design.md`

## Global Constraints

- Complete the foundation, OSA, Voltage Source, and Gain Driver plans first.
- Do not add physical-experiment behavior or save experiment results in this plan.
- Read-only/measurement checks precede any nonzero output; every output-changing action requires explicit confirmation.
- A combined-check failure must still close every driver already opened, invoking its approved fail-safe behavior.
- The current workspace is not a Git repository; use passing tests and recorded hardware results as checkpoints.

## Locked File Structure

- Create `Code/Debugs/check_all.py`: combined orchestration.
- Modify `Code/Debugs/test_drivers.py`: combined cleanup/order tests.
- Modify `Code/Utils/__init__.py`: verify final public export surface only if an export is missing.

---

### Task 1: Combined Safe Health Check

**Files:**
- Create: `Code/Debugs/check_all.py`
- Modify: `Code/Debugs/test_drivers.py`

**Interfaces:**
- Consumes: `AQ6370`, `VoltageSource`, and `GainDriver` public APIs only.
- Produces: `run_checks(osa_factory=AQ6370, voltage_factory=VoltageSource, gain_factory=GainDriver) -> dict[str, object]` and CLI `main() -> int`.

- [ ] **Step 1: Add failing orchestration tests**

Append tests using three fake context-manager drivers that record entry, check, and exit events:

```python
from Code.Debugs.check_all import run_checks


class IntegrationTests(unittest.TestCase):
    def test_safe_check_order_and_cleanup(self):
        events = []
        results = run_checks(
            osa_factory=make_fake_osa(events),
            voltage_factory=make_fake_voltage(events),
            gain_factory=make_fake_gain(events),
        )
        self.assertEqual(events[:6], [
            "osa.enter", "osa.acquire", "osa.exit",
            "voltage.enter", "voltage.status", "voltage.exit",
        ])
        self.assertEqual(events[6:], ["gain.enter", "gain.status", "gain.exit"])
        self.assertEqual(set(results), {"osa", "voltage", "gain"})

    def test_later_failure_does_not_skip_cleanup(self):
        events = []
        with self.assertRaises(RuntimeError):
            run_checks(
                osa_factory=make_fake_osa(events),
                voltage_factory=make_failing_voltage(events),
                gain_factory=make_fake_gain(events),
            )
        self.assertIn("osa.exit", events)
        self.assertIn("voltage.exit", events)
        self.assertNotIn("gain.enter", events)
```

- [ ] **Step 2: Run and verify the missing-module failure**

Run:

```powershell
conda run -n VISA python -m unittest Code.Debugs.test_drivers.IntegrationTests -v
```

Expected: FAIL because `Code.Debugs.check_all` does not exist.

- [ ] **Step 3: Implement safe orchestration**

Create `Code/Debugs/check_all.py`. `run_checks()` must use three separate `with` blocks, in order, so one device is fully cleaned up before the next opens:

```python
def run_checks(osa_factory=AQ6370, voltage_factory=VoltageSource, gain_factory=GainDriver):
    results = {}
    with osa_factory() as osa:
        spectrum = osa.acquire()
        results["osa"] = {
            "identity": osa.identity,
            "points": len(spectrum.wavelength_nm),
            "wavelength_nm": (float(spectrum.wavelength_nm[0]), float(spectrum.wavelength_nm[-1])),
        }
    with voltage_factory() as voltage:
        status = voltage.read_status()
        results["voltage"] = {
            "voltage_v": status.voltage_v,
            "current_ma": status.current_ma,
        }
    with gain_factory() as gain:
        results["gain"] = {
            "temperature_c": gain.status.temperature_c,
            "target_c": gain.status.target_c,
            "tec_enabled": gain.status.tec_enabled,
            "current_ma": gain.status.current_ma,
            "current_enabled": gain.status.current_enabled,
        }
    return results
```

`main()` configures timestamped logging, calls `run_checks()`, and prints one concise section per device. It performs no optional nonzero voltage/current action; those remain in `check_voltage.py` and `check_gain.py` where confirmations are specific.

- [ ] **Step 4: Run integration tests**

Run:

```powershell
conda run -n VISA python -m unittest Code.Debugs.test_drivers.IntegrationTests -v
```

Expected: 2 tests PASS and event order proves cleanup.

### Task 2: Full Offline Acceptance

**Files:**
- Modify only when a failing test reveals a defect: `Code/Utils/common.py`, `Code/Utils/osa.py`, `Code/Utils/voltage.py`, `Code/Utils/gain.py`, or `Code/Debugs/test_drivers.py`.

**Interfaces:**
- Consumes: all planned driver APIs and tests.
- Produces: complete offline evidence that imports, protocol logic, boundaries, retries, interlocks, threads, and cleanup work together.

- [ ] **Step 1: Compile every maintained Python file**

Run:

```powershell
conda run -n VISA python -m compileall -q Code
```

Expected: exit 0. Remove no files; `__pycache__` is permitted locally but must not be treated as source.

- [ ] **Step 2: Run the entire offline suite three times**

Run this command three separate times to expose thread timing leaks:

```powershell
conda run -n VISA python -m unittest Code.Debugs.test_drivers -v
```

Expected on every run: all tests PASS, no hang, no real serial/VISA resource opens, and the process exits normally.

- [ ] **Step 3: Verify public imports**

Run:

```powershell
conda run -n VISA python -c "from Code.Utils import AQ6370, Spectrum, VoltageSource, VoltageStatus, GainDriver, GainStatus; print('imports ok')"
```

Expected: `imports ok`.

- [ ] **Step 4: Verify persistent rules and maintained-file placement**

Run:

```powershell
rg --files Code docs AGENTS.md
rg -n "serial\.Serial|pyvisa\.ResourceManager" Code\Experiments
```

Expected: maintained drivers/checks are in approved directories; the second command finds no direct hardware access in experiment code. An exit code of 1 from the second `rg` is expected when it finds no matches.

- [ ] **Step 5: Fix only evidence-backed failures and rerun the affected command**

For each failure, first capture the exact traceback, add or tighten the smallest reproducing test in `Code/Debugs/test_drivers.py`, implement the minimum correction, then rerun both that test class and the full suite. Do not add features outside the approved spec.

### Task 3: Staged Real-Hardware Acceptance

**Files:**
- No source changes unless a hardware result exposes a reproducible defect; if so, return to Task 2's failing-test-first loop.

**Interfaces:**
- Consumes: all four debug programs.
- Produces: recorded pass/fail evidence for each physical device and final safe state.

- [ ] **Step 1: Enumerate resources without opening outputs**

Run:

```powershell
conda run -n VISA python -m serial.tools.list_ports -v
```

Expected: exactly one CH340 `1A86:7523` and one CP210x `10C4:EA60`; record their current COM numbers and CP210x serial number. Enumerate VISA resources without opening them:

```powershell
conda run -n VISA python -c "import pyvisa; manager = pyvisa.ResourceManager(); print(manager.list_resources()); manager.close()"
```

Record the AQ6370 GPIB resource.

- [ ] **Step 2: Run OSA acquisition check**

With explicit approval to trigger one measurement, run:

```powershell
conda run -n VISA python -m Code.Debugs.check_osa
```

Record identity, point count, wavelength/power range, elapsed time, retry count, and confirmation that front-panel settings did not change.

- [ ] **Step 3: Run Voltage Source safe check**

With explicit approval for startup zero, run:

```powershell
conda run -n VISA python -m Code.Debugs.check_voltage
```

First answer `N` to the 0.1 V prompt. Record frame length, eight decoded channels, startup-zero confirmation, final zero, and clean port release. Run again and answer `y` only after separate approval for the CH1 0.1 V change; record measured CH1 and final zero.

- [ ] **Step 4: Run Gain Driver read/no-op check**

Run:

```powershell
conda run -n VISA python -m Code.Debugs.check_gain
```

Decline output enable on the first run. Record CRLF confirmation, temperature, target, TEC/current states, current setting, no-op write verification, and final current/TEC off state. Perform the stabilized output test only after separate approval and record trip/cleanup behavior.

- [ ] **Step 5: Run combined safe health check**

Run:

```powershell
conda run -n VISA python -m Code.Debugs.check_all
```

Expected: all three sections print successfully in safe order; Voltage Source ends at zero and Gain Driver ends current off then TEC off even if the user interrupts after any section.

- [ ] **Step 6: Record the final checkpoint**

Provide a concise acceptance table with device, resource/port, identity/protocol, read result, controlled-write result, fault/cleanup result, and final safe state. Explicitly list any test not run or any hardware behavior that differs from the spec. Do not claim physical control for an unrun output test.
