# AQ6370 Driver Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement and verify a reusable AQ6370 acquisition driver that preserves front-panel settings, bounds every operation, retries failed sweeps, validates spectra, and releases VISA resources safely.

**Architecture:** Wrap the existing PyVISA resource and PyMeasure `AQ6370Series` object behind one `AQ6370` context manager. Inject resource/instrument factories for offline tests; keep all panel configuration outside the driver.

**Tech Stack:** Python 3.10, PyVISA 1.14.1, PyMeasure 0.15.0, NumPy 2.2.4, standard-library `dataclasses` and `unittest`, Anaconda environment `VISA`.

**Spec:** `docs/superpowers/specs/2026-08-19-instrument-drivers-design.md`

## Global Constraints

- Complete `docs/superpowers/plans/2026-08-19-driver-foundation.md` first.
- Do not change OSA center wavelength, span, RBW, points, sweep mode, or any other front-panel measurement setting.
- Default VISA resource is `GPIB0::4::INSTR`; default acquisition timeout is 30 seconds; retry count is two after the initial attempt.
- A failed acquisition never returns an old trace as success.
- The current workspace is not a Git repository; use passing tests as checkpoints and do not initialize Git.

## Locked File Structure

- Create `Code/Utils/osa.py`: spectrum data, connection, acquisition, validation, retry, and cleanup.
- Modify `Code/Utils/__init__.py`: export `AQ6370` and `Spectrum`.
- Modify `Code/Debugs/test_drivers.py`: fake-resource unit tests.
- Create `Code/Debugs/check_osa.py`: real-device acquisition check.

---

### Task 1: Spectrum Validation and Connection Lifecycle

**Files:**
- Create: `Code/Utils/osa.py`
- Modify: `Code/Debugs/test_drivers.py`

**Interfaces:**
- Consumes: foundation `DriverState`, exceptions, `get_logger`, `require_state`.
- Produces: immutable `Spectrum(wavelength_nm, power_dbm, trace, acquired_at, identity)` and `AQ6370.connect()`, `AQ6370.close()`, context-manager methods.

- [ ] **Step 1: Add failing spectrum and connection tests**

Append imports and tests to `Code/Debugs/test_drivers.py`:

```python
from datetime import datetime, timezone
from unittest.mock import Mock

import numpy as np

from Code.Utils.osa import AQ6370, Spectrum


class OsaConnectionTests(unittest.TestCase):
    def test_spectrum_rejects_mismatched_arrays(self):
        with self.assertRaises(InstrumentProtocolError):
            Spectrum.create([1550.0], [-30.0, -31.0], "A", "AQ6370")

    def test_spectrum_rejects_nonfinite_values(self):
        with self.assertRaises(InstrumentProtocolError):
            Spectrum.create([1550.0, float("nan")], [-30.0, -31.0], "A", "AQ6370")

    def test_connect_identifies_instrument_without_writing_settings(self):
        resource = Mock()
        resource.query.return_value = "YOKOGAWA,AQ6370D,91V000000,01.00\n"
        manager = Mock()
        manager.open_resource.return_value = resource
        instrument = Mock()
        driver = AQ6370(
            resource_manager_factory=lambda: manager,
            instrument_factory=lambda opened: instrument,
        )
        driver.connect()
        self.assertEqual(driver.identity, "YOKOGAWA,AQ6370D,91V000000,01.00")
        manager.open_resource.assert_called_once_with("GPIB0::4::INSTR")
        resource.query.assert_called_once_with("*IDN?")
        instrument.initiate_sweep.assert_not_called()
        driver.close()
        resource.close.assert_called_once()
        manager.close.assert_called_once()
```

Also extend the existing common import with `InstrumentProtocolError`.

- [ ] **Step 2: Run the tests and verify the missing-module failure**

Run:

```powershell
conda run -n VISA python -m unittest Code.Debugs.test_drivers.OsaConnectionTests -v
```

Expected: FAIL because `Code.Utils.osa` does not exist.

- [ ] **Step 3: Implement data validation and connection**

Create `Code/Utils/osa.py` with these exact public signatures:

```python
from __future__ import annotations

from dataclasses import dataclass
from datetime import datetime, timezone
from typing import Callable, Sequence

import numpy as np
import pyvisa
from pymeasure.instruments.yokogawa.aq6370series import AQ6370Series

from .common import (
    DriverState,
    InstrumentConnectionError,
    InstrumentProtocolError,
    get_logger,
)


@dataclass(frozen=True)
class Spectrum:
    wavelength_nm: np.ndarray
    power_dbm: np.ndarray
    trace: str
    acquired_at: datetime
    identity: str

    @classmethod
    def create(
        cls,
        wavelength_nm: Sequence[float],
        power_dbm: Sequence[float],
        trace: str,
        identity: str,
    ) -> "Spectrum":
        wavelength = np.asarray(wavelength_nm, dtype=float)
        power = np.asarray(power_dbm, dtype=float)
        if wavelength.ndim != 1 or power.ndim != 1 or not len(wavelength):
            raise InstrumentProtocolError("OSA trace arrays must be non-empty and one-dimensional")
        if wavelength.shape != power.shape:
            raise InstrumentProtocolError("OSA wavelength and power arrays have different lengths")
        if not np.all(np.isfinite(wavelength)) or not np.all(np.isfinite(power)):
            raise InstrumentProtocolError("OSA trace contains non-finite values")
        if len(wavelength) > 1 and not np.all(np.diff(wavelength) > 0):
            raise InstrumentProtocolError("OSA wavelengths are not strictly increasing")
        return cls(wavelength, power, trace, datetime.now(timezone.utc), identity)


class AQ6370:
    def __init__(
        self,
        resource_name: str = "GPIB0::4::INSTR",
        timeout: float = 30.0,
        retries: int = 2,
        resource_manager_factory: Callable[[], object] = pyvisa.ResourceManager,
        instrument_factory: Callable[[object], object] = AQ6370Series,
    ) -> None:
        if timeout <= 0 or retries < 0:
            raise ValueError("timeout must be positive and retries must be non-negative")
        self.resource_name = resource_name
        self.timeout = timeout
        self.retries = retries
        self._resource_manager_factory = resource_manager_factory
        self._instrument_factory = instrument_factory
        self._manager = None
        self._resource = None
        self._instrument = None
        self.state = DriverState.DISCONNECTED
        self.identity = ""
        self.log = get_logger("osa")

    def connect(self) -> "AQ6370":
        if self.state in {DriverState.READY, DriverState.ACTIVE}:
            return self
        self.state = DriverState.CONNECTING
        try:
            self._manager = self._resource_manager_factory()
            self._resource = self._manager.open_resource(self.resource_name)
            self._resource.timeout = int(self.timeout * 1000)
            self.identity = str(self._resource.query("*IDN?")).strip()
            if "AQ6370" not in self.identity.upper():
                raise InstrumentConnectionError(f"Unexpected OSA identity: {self.identity!r}")
            self._instrument = self._instrument_factory(self._resource)
            self.state = DriverState.READY
            return self
        except Exception as error:
            self.state = DriverState.FAULT
            self.close()
            if isinstance(error, InstrumentConnectionError):
                raise
            raise InstrumentConnectionError(f"Failed to connect to {self.resource_name}") from error

    def __enter__(self) -> "AQ6370":
        return self.connect()

    def __exit__(self, exc_type, exc, traceback) -> None:
        self.close()
```

Implement `close()` so it is idempotent, attempts `instrument.abort()` if an instrument exists, then closes the resource and manager in separate `try` blocks, clears references, and ends in `DISCONNECTED`. Log cleanup errors without raising them from `close()`.

- [ ] **Step 4: Run connection tests**

Run:

```powershell
conda run -n VISA python -m unittest Code.Debugs.test_drivers.OsaConnectionTests -v
```

Expected: 3 tests PASS.

### Task 2: Bounded Acquisition, Retry, and Old-Trace Prevention

**Files:**
- Modify: `Code/Utils/osa.py`
- Modify: `Code/Debugs/test_drivers.py`

**Interfaces:**
- Consumes: connected AQ6370 internals from Task 1.
- Produces: `AQ6370.acquire(trace: str = "A", timeout: float | None = None) -> Spectrum`.

- [ ] **Step 1: Add failing acquisition tests**

Append:

```python
class OsaAcquisitionTests(unittest.TestCase):
    def make_driver(self):
        resource = Mock()
        resource.query.side_effect = ["YOKOGAWA,AQ6370D,SN,FW", "1"]
        manager = Mock()
        manager.open_resource.return_value = resource
        instrument = Mock()
        instrument.get_xdata.return_value = [1.549e-6, 1.550e-6]
        instrument.get_ydata.return_value = [-41.0, -40.0]
        driver = AQ6370(
            resource_manager_factory=lambda: manager,
            instrument_factory=lambda opened: instrument,
        ).connect()
        return driver, resource, instrument

    def test_acquire_waits_and_converts_metres_to_nanometres(self):
        driver, resource, instrument = self.make_driver()
        spectrum = driver.acquire()
        instrument.initiate_sweep.assert_called_once()
        resource.query.assert_called_with("*OPC?")
        np.testing.assert_allclose(spectrum.wavelength_nm, [1549.0, 1550.0])
        np.testing.assert_allclose(spectrum.power_dbm, [-41.0, -40.0])
        driver.close()

    def test_acquire_aborts_and_retries(self):
        driver, resource, instrument = self.make_driver()
        resource.query.side_effect = [TimeoutError("first"), "1"]
        spectrum = driver.acquire()
        self.assertEqual(instrument.initiate_sweep.call_count, 2)
        instrument.abort.assert_called()
        self.assertEqual(len(spectrum.wavelength_nm), 2)
        driver.close()

    def test_final_failure_closes_and_raises(self):
        driver, resource, instrument = self.make_driver()
        resource.query.side_effect = TimeoutError("always")
        with self.assertRaises(InstrumentTimeoutError):
            driver.acquire()
        self.assertEqual(driver.state, DriverState.DISCONNECTED)
        resource.close.assert_called_once()
```

Add `InstrumentTimeoutError` to imports.

- [ ] **Step 2: Run tests and verify `acquire` is missing**

Run:

```powershell
conda run -n VISA python -m unittest Code.Debugs.test_drivers.OsaAcquisitionTests -v
```

Expected: FAIL because `AQ6370.acquire` is not defined.

- [ ] **Step 3: Implement acquisition**

Add `acquire()` with this algorithm:

```python
def acquire(self, trace: str = "A", timeout: float | None = None) -> Spectrum:
    require_state(self.state, {DriverState.READY}, "acquire an OSA spectrum")
    trace_name = trace.upper().replace("TR", "")
    if not trace_name or not trace_name.isalpha():
        raise ValueError("trace must identify an alphabetic AQ6370 trace")
    wait_seconds = self.timeout if timeout is None else timeout
    if wait_seconds <= 0:
        raise ValueError("timeout must be positive")
    self._resource.timeout = int(wait_seconds * 1000)
    self.state = DriverState.ACTIVE
    last_error = None
    for attempt in range(self.retries + 1):
        try:
            self._instrument.initiate_sweep()
            if str(self._resource.query("*OPC?")).strip() != "1":
                raise InstrumentProtocolError("OSA did not acknowledge sweep completion")
            wavelength_m = self._instrument.get_xdata(trace_name)
            power_dbm = self._instrument.get_ydata(trace_name)
            spectrum = Spectrum.create(
                np.asarray(wavelength_m, dtype=float) * 1e9,
                power_dbm,
                trace_name,
                self.identity,
            )
            self.state = DriverState.READY
            return spectrum
        except Exception as error:
            last_error = error
            try:
                self._instrument.abort()
            except Exception:
                self.log.exception("Failed to abort OSA after acquisition error")
            if attempt < self.retries:
                continue
    self.state = DriverState.FAULT
    self.close()
    if isinstance(last_error, InstrumentProtocolError):
        raise last_error
    raise InstrumentTimeoutError(
        f"OSA acquisition failed after {self.retries + 1} attempts"
    ) from last_error
```

Import `require_state` and `InstrumentTimeoutError`. Do not add any measurement-setting writes.

- [ ] **Step 4: Run all OSA offline tests**

Run:

```powershell
conda run -n VISA python -m unittest Code.Debugs.test_drivers.OsaConnectionTests Code.Debugs.test_drivers.OsaAcquisitionTests -v
```

Expected: 6 tests PASS.

### Task 3: Public Export and Real-Hardware Check

**Files:**
- Modify: `Code/Utils/__init__.py`
- Create: `Code/Debugs/check_osa.py`

**Interfaces:**
- Consumes: `AQ6370.acquire()`.
- Produces: a command-line check with exit status 0 on valid acquisition and nonzero on failure.

- [ ] **Step 1: Export the driver**

Add:

```python
from .osa import AQ6370, Spectrum
```

and add `"AQ6370"` and `"Spectrum"` to `__all__`.

- [ ] **Step 2: Create the hardware check**

Create `Code/Debugs/check_osa.py`:

```python
from __future__ import annotations

import argparse
import logging

import numpy as np

from Code.Utils.osa import AQ6370


def main() -> int:
    parser = argparse.ArgumentParser(description="Check AQ6370 acquisition without changing panel settings")
    parser.add_argument("--resource", default="GPIB0::4::INSTR")
    parser.add_argument("--timeout", type=float, default=30.0)
    args = parser.parse_args()
    logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s")
    with AQ6370(resource_name=args.resource, timeout=args.timeout) as osa:
        spectrum = osa.acquire()
        print(f"identity={osa.identity}")
        print(f"points={len(spectrum.wavelength_nm)}")
        print(f"wavelength_nm={spectrum.wavelength_nm[0]:.6f}..{spectrum.wavelength_nm[-1]:.6f}")
        print(f"power_dbm={np.min(spectrum.power_dbm):.3f}..{np.max(spectrum.power_dbm):.3f}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
```

- [ ] **Step 3: Verify syntax and help without opening hardware**

Run:

```powershell
conda run -n VISA python -m py_compile Code\Utils\osa.py Code\Debugs\check_osa.py
conda run -n VISA python -m Code.Debugs.check_osa --help
```

Expected: both commands exit 0 and the help text describes `--resource` and `--timeout`.

- [ ] **Step 4: Run the real check only with explicit hardware-test approval**

Run:

```powershell
conda run -n VISA python -m Code.Debugs.check_osa
```

Expected: AQ6370 identity plus a nonzero point count and finite wavelength/power ranges. Confirm from the OSA panel before and after that measurement settings were not changed.

- [ ] **Step 5: Record the checkpoint**

Record offline test count, hardware identity, point count, and whether panel settings stayed unchanged. Do not commit because the workspace is not a Git repository.
