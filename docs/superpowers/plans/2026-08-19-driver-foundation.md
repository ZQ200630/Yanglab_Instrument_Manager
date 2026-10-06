# Driver Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Establish the permanent workspace rules, Python package structure, shared driver state/errors, logging, and deterministic USB serial discovery used by all three instrument drivers.

**Architecture:** Keep shared code intentionally small. `common.py` owns only cross-driver state, exceptions, logger creation, and exact-match port discovery; instrument protocols remain in their own modules.

**Tech Stack:** Python 3.10, standard-library `enum`, `logging`, `pathlib`, `unittest`, PySerial port enumeration, Anaconda environment `VISA`.

**Spec:** `docs/superpowers/specs/2026-08-19-instrument-drivers-design.md`

## Global Constraints

- Reusable drivers live in `Code/Utils`; hardware diagnostics live in `Code/Debugs`.
- Experiments live in `Code/Experiments/<name>` and outputs live in `Result/<name>`.
- `reference_code` remains unchanged.
- Experiment code must never bypass the driver layer to access serial or VISA devices.
- Names remain short and descriptive.
- The current workspace is not a Git repository; do not initialize Git or run commit commands without user authorization. Treat each passing task test as the review checkpoint.

## Locked File Structure

- Create `AGENTS.md`: persistent workspace organization and safety rules.
- Create `Code/__init__.py`: package marker.
- Create `Code/Utils/__init__.py`: public driver exports, initially common exports only.
- Create `Code/Utils/common.py`: shared state, exceptions, logger, state validation, and USB port discovery.
- Create `Code/Debugs/__init__.py`: package marker.
- Create `Code/Debugs/test_drivers.py`: one standard-library test module extended by later plans.

---

### Task 1: Persist Workspace Rules and Package Boundaries

**Files:**
- Create: `AGENTS.md`
- Create: `Code/__init__.py`
- Create: `Code/Utils/__init__.py`
- Create: `Code/Debugs/__init__.py`

**Interfaces:**
- Consumes: Approved project layout and safety policy from the spec.
- Produces: Importable `Code`, `Code.Utils`, and `Code.Debugs` packages; persistent instructions for future AI workers.

- [ ] **Step 1: Create the root workspace policy**

Write `AGENTS.md` with the exact enforceable rules below:

```markdown
# Workspace Rules

## Structure

- Put reusable instrument drivers in `Code/Utils`.
- Put hardware bring-up and diagnostic scripts in `Code/Debugs`.
- Put each experiment in `Code/Experiments/<name>`.
- Put each experiment's generated output in `Result/<name>`.
- Treat `reference_code` as read-only protocol reference.
- Keep file and experiment names short and descriptive.

## Driver Boundary

- Experiment code must use `Code/Utils` drivers; do not open serial ports or VISA resources directly from experiment code.
- Do not bypass voltage, current, temperature, startup, interlock, watchdog, or shutdown limits enforced by a driver.
- Do not put generated data, caches, or temporary files in `Code/Utils`.

## Hardware Safety

- Begin hardware diagnostics with identity and read-only checks.
- Clearly disclose and confirm any diagnostic that can change an output state before running it.
- Voltage Source commands are limited to 0-14 V. Normal changes use steps no larger than 0.1 V with at least 50 ms between steps. Handled failures and shutdown send all channels to 0 V immediately.
- Gain Driver commands are limited to 0-200 mA and target temperatures of 15-40 degC. Shutdown disables current before TEC.
- Gain current may be enabled only while TEC is on and measured temperature has remained within target +/-0.2 degC for five consecutive seconds.
- OSA acquisition preserves front-panel measurement settings.

## Verification

- Run offline tests in the Anaconda `VISA` environment before touching hardware.
- Use the scripts under `Code/Debugs` for real-device validation.
- Always release serial/VISA resources in `finally` blocks or context managers.
```

- [ ] **Step 2: Create package markers**

Create empty `Code/__init__.py` and `Code/Debugs/__init__.py`. Create `Code/Utils/__init__.py` with exports that Task 2 will satisfy:

```python
from .common import (
    DeviceFault,
    DriverError,
    DriverState,
    InstrumentConnectionError,
    InstrumentProtocolError,
    InstrumentSafetyError,
    InstrumentTimeoutError,
    find_serial_port,
)

__all__ = [
    "DeviceFault",
    "DriverError",
    "DriverState",
    "InstrumentConnectionError",
    "InstrumentProtocolError",
    "InstrumentSafetyError",
    "InstrumentTimeoutError",
    "find_serial_port",
]
```

- [ ] **Step 3: Verify paths and policy text**

Run:

```powershell
Get-Item AGENTS.md,Code\__init__.py,Code\Utils\__init__.py,Code\Debugs\__init__.py
rg -n "0-14 V|0-200 mA|15-40 degC|current before TEC|front-panel" AGENTS.md
```

Expected: all four files exist and all five safety phrases are present. `Code.Utils` is not imported until Task 2 creates `common.py`.

### Task 2: Shared State, Errors, and State Validation

**Files:**
- Create: `Code/Utils/common.py`
- Create: `Code/Debugs/test_drivers.py`

**Interfaces:**
- Consumes: Python standard library.
- Produces: `DriverState`; `DriverError` hierarchy; `require_state(actual, allowed, action)`; `get_logger(name)`.

- [ ] **Step 1: Write failing common-type tests**

Create `Code/Debugs/test_drivers.py`:

```python
import logging
import unittest

from Code.Utils.common import (
    DriverState,
    InstrumentSafetyError,
    get_logger,
    require_state,
)


class CommonTests(unittest.TestCase):
    def test_require_state_accepts_allowed_state(self):
        require_state(DriverState.READY, {DriverState.READY}, "read")

    def test_require_state_rejects_wrong_state(self):
        with self.assertRaises(InstrumentSafetyError) as caught:
            require_state(DriverState.FAULT, {DriverState.READY}, "read")
        self.assertIn("read", str(caught.exception))
        self.assertIn("FAULT", str(caught.exception))

    def test_logger_is_namespaced_and_has_null_handler(self):
        logger = get_logger("voltage")
        self.assertEqual(logger.name, "sil.voltage")
        self.assertTrue(any(isinstance(item, logging.NullHandler) for item in logger.handlers))


if __name__ == "__main__":
    unittest.main()
```

- [ ] **Step 2: Run the tests and confirm the missing-module failure**

Run:

```powershell
conda run -n VISA python -m unittest Code.Debugs.test_drivers.CommonTests -v
```

Expected: FAIL because `Code.Utils.common` does not exist.

- [ ] **Step 3: Implement the shared types**

Create `Code/Utils/common.py`:

```python
from __future__ import annotations

import logging
from enum import Enum, auto
from typing import Collection


class DriverState(Enum):
    DISCONNECTED = auto()
    CONNECTING = auto()
    READY = auto()
    ACTIVE = auto()
    CLOSING = auto()
    FAULT = auto()


class DriverError(RuntimeError):
    """Base error for instrument-driver failures."""


class InstrumentConnectionError(DriverError):
    """A device could not be uniquely found, opened, or identified."""


class InstrumentProtocolError(DriverError):
    """A device frame or response violated its protocol."""


class InstrumentSafetyError(DriverError):
    """An operation violated a state, range, or interlock rule."""


class InstrumentTimeoutError(DriverError):
    """A bounded device operation did not complete in time."""


class DeviceFault(DriverError):
    """A driver entered a latched fault state."""


def require_state(actual: DriverState, allowed: Collection[DriverState], action: str) -> None:
    if actual not in allowed:
        names = ", ".join(sorted(state.name for state in allowed))
        raise InstrumentSafetyError(
            f"Cannot {action} while driver state is {actual.name}; allowed states: {names}"
        )


def get_logger(name: str) -> logging.Logger:
    logger = logging.getLogger(f"sil.{name}")
    if not logger.handlers:
        logger.addHandler(logging.NullHandler())
    return logger
```

- [ ] **Step 4: Run common tests**

Run:

```powershell
conda run -n VISA python -m unittest Code.Debugs.test_drivers.CommonTests -v
```

Expected: 3 tests PASS.

### Task 3: Deterministic USB Serial Discovery

**Files:**
- Modify: `Code/Utils/common.py`
- Modify: `Code/Debugs/test_drivers.py`

**Interfaces:**
- Consumes: iterable objects with `.device`, `.vid`, `.pid`, and `.serial_number` attributes.
- Produces: `find_serial_port(vid: int, pid: int, serial_number: str | None = None, ports_provider: Callable[[], Iterable[object]] = list_ports.comports) -> str`.

- [ ] **Step 1: Add failing discovery tests**

Append to `Code/Debugs/test_drivers.py`:

```python
from types import SimpleNamespace

from Code.Utils.common import InstrumentConnectionError, find_serial_port


class PortDiscoveryTests(unittest.TestCase):
    def setUp(self):
        self.ports = [
            SimpleNamespace(device="COM4", vid=0x1A86, pid=0x7523, serial_number=None),
            SimpleNamespace(device="COM5", vid=0x10C4, pid=0xEA60, serial_number="GAIN-1"),
        ]

    def test_finds_unique_vid_pid(self):
        found = find_serial_port(0x1A86, 0x7523, ports_provider=lambda: self.ports)
        self.assertEqual(found, "COM4")

    def test_filters_by_serial_number(self):
        found = find_serial_port(
            0x10C4, 0xEA60, "GAIN-1", ports_provider=lambda: self.ports
        )
        self.assertEqual(found, "COM5")

    def test_rejects_no_match(self):
        with self.assertRaises(InstrumentConnectionError):
            find_serial_port(0xFFFF, 0xFFFF, ports_provider=lambda: self.ports)

    def test_rejects_ambiguous_match(self):
        duplicate = self.ports + [
            SimpleNamespace(device="COM6", vid=0x1A86, pid=0x7523, serial_number=None)
        ]
        with self.assertRaises(InstrumentConnectionError):
            find_serial_port(0x1A86, 0x7523, ports_provider=lambda: duplicate)
```

- [ ] **Step 2: Run and confirm the missing-function failure**

Run:

```powershell
conda run -n VISA python -m unittest Code.Debugs.test_drivers.PortDiscoveryTests -v
```

Expected: FAIL because `find_serial_port` is not defined.

- [ ] **Step 3: Implement exact-match discovery**

Add to `Code/Utils/common.py`:

```python
from collections.abc import Callable, Iterable
from serial.tools import list_ports


def find_serial_port(
    vid: int,
    pid: int,
    serial_number: str | None = None,
    ports_provider: Callable[[], Iterable[object]] = list_ports.comports,
) -> str:
    matches = []
    for port in ports_provider():
        if getattr(port, "vid", None) != vid or getattr(port, "pid", None) != pid:
            continue
        if serial_number is not None and getattr(port, "serial_number", None) != serial_number:
            continue
        matches.append(str(port.device))
    if len(matches) != 1:
        detail = "none" if not matches else ", ".join(matches)
        raise InstrumentConnectionError(
            f"Expected exactly one USB serial device {vid:04X}:{pid:04X}; found {detail}"
        )
    return matches[0]
```

- [ ] **Step 4: Run the complete foundation suite**

Run:

```powershell
conda run -n VISA python -m unittest Code.Debugs.test_drivers.CommonTests Code.Debugs.test_drivers.PortDiscoveryTests -v
```

Expected: 7 tests PASS and no hardware port is opened.

- [ ] **Step 5: Record the checkpoint**

Record the passing command and test count in the task handoff. Do not run `git commit` because this workspace has no Git repository.
