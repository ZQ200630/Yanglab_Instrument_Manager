# Voltage Source Driver Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement a thread-safe eight-channel voltage-source driver with strict protocol validation, 0-14 V enforcement, verified startup zero, smooth normal ramps, continuous telemetry, and immediate fail-safe zeroing.

**Architecture:** Separate pure frame encode/decode functions from the stateful `VoltageSource`. One reader thread owns incoming telemetry; one reentrant operation lock owns all output sequences; immutable snapshots cross the thread boundary.

**Tech Stack:** Python 3.10, PySerial 3.5, standard-library `dataclasses`, `threading`, `time`, and `unittest`, Anaconda environment `VISA`.

**Spec:** `docs/superpowers/specs/2026-08-19-instrument-drivers-design.md`

## Global Constraints

- Complete `docs/superpowers/plans/2026-08-19-driver-foundation.md` first.
- Auto-discovery uses CH340 VID:PID `1A86:7523`; serial is 115200 8N1 with bounded timeouts.
- Commands always contain eight channels and end in CRLF.
- The project hard limit is 0-14 V; normal steps are at most 0.1 V with at least 50 ms spacing.
- Startup and handled shutdown require immediate all-zero output and measured confirmation below 0.05 V at startup.
- The current workspace is not a Git repository; use passing tests as checkpoints.

## Locked File Structure

- Create `Code/Utils/voltage.py`: protocol, status, reader, control, fault, and cleanup.
- Modify `Code/Utils/__init__.py`: public exports.
- Modify `Code/Debugs/test_drivers.py`: protocol and lifecycle tests.
- Create `Code/Debugs/check_voltage.py`: real CH340 telemetry/zero/optional 0.1 V check.

---

### Task 1: Pure Voltage Command and Telemetry Protocol

**Files:**
- Create: `Code/Utils/voltage.py`
- Modify: `Code/Debugs/test_drivers.py`

**Interfaces:**
- Consumes: common protocol/safety errors.
- Produces: `VoltageStatus`, `encode_voltages(values, limit=14.0) -> bytes`, `decode_telemetry(frame, received_at=None) -> VoltageStatus`.

- [ ] **Step 1: Add failing protocol tests**

Append:

```python
import math

from Code.Utils.voltage import VoltageStatus, decode_telemetry, encode_voltages


class VoltageProtocolTests(unittest.TestCase):
    def test_encode_zero_and_full_safe_range(self):
        frame = encode_voltages([0.0, 14.0, 0, 0, 0, 0, 0, 0])
        self.assertEqual(len(frame), 18)
        self.assertEqual(frame[:2], b"\x00\x00")
        expected = int(14.0 * 65535 / 28)
        self.assertEqual(frame[2:4], expected.to_bytes(2, "big"))
        self.assertTrue(frame.endswith(b"\r\n"))

    def test_encode_rejects_count_and_unsafe_values(self):
        for values in ([0.0] * 7, [0.0] * 7 + [14.01], [0.0] * 7 + [math.nan], [-0.01] + [0.0] * 7):
            with self.subTest(values=values):
                with self.assertRaises(InstrumentSafetyError):
                    encode_voltages(values)

    def test_decode_eight_voltage_current_pairs(self):
        payload = bytearray()
        for channel in range(8):
            voltage_raw = 1000 + channel
            current_raw = -20 + channel
            payload.extend(voltage_raw.to_bytes(2, "big"))
            payload.extend(current_raw.to_bytes(2, "big", signed=True))
        status = decode_telemetry(bytes(payload) + b"\r\n", received_at=123.0)
        self.assertAlmostEqual(status.voltage_v[0], 1.6)
        self.assertAlmostEqual(status.current_ma[0], -0.05)
        self.assertEqual(status.received_at, 123.0)

    def test_decode_rejects_short_or_unterminated_frame(self):
        for frame in (b"\x00" * 32, b"\x00" * 31 + b"\r\n"):
            with self.assertRaises(InstrumentProtocolError):
                decode_telemetry(frame)
```

- [ ] **Step 2: Run and verify the missing-module failure**

Run:

```powershell
conda run -n VISA python -m unittest Code.Debugs.test_drivers.VoltageProtocolTests -v
```

Expected: FAIL because `Code.Utils.voltage` does not exist.

- [ ] **Step 3: Implement the protocol**

Create `Code/Utils/voltage.py` with:

```python
from __future__ import annotations

import math
import time
from dataclasses import dataclass
from typing import Sequence

from .common import InstrumentProtocolError, InstrumentSafetyError

CHANNELS = 8
BOARD_SCALE_V = 28.0
DEFAULT_LIMIT_V = 14.0


@dataclass(frozen=True)
class VoltageStatus:
    voltage_v: tuple[float, ...]
    current_ma: tuple[float, ...]
    received_at: float


def encode_voltages(values: Sequence[float], limit: float = DEFAULT_LIMIT_V) -> bytes:
    if len(values) != CHANNELS:
        raise InstrumentSafetyError("Voltage command requires exactly eight channels")
    payload = bytearray()
    for index, value in enumerate(values, start=1):
        numeric = float(value)
        if not math.isfinite(numeric) or not 0.0 <= numeric <= limit:
            raise InstrumentSafetyError(f"Channel {index} voltage {numeric!r} is outside 0-{limit} V")
        raw = int(numeric * 65535 / BOARD_SCALE_V)
        payload.extend(raw.to_bytes(2, "big", signed=False))
    return bytes(payload + b"\r\n")


def decode_telemetry(frame: bytes, received_at: float | None = None) -> VoltageStatus:
    if len(frame) != 34 or not frame.endswith(b"\r\n"):
        raise InstrumentProtocolError(f"Expected 34-byte voltage telemetry frame, got {len(frame)}")
    payload = frame[:-2]
    voltages = []
    currents = []
    for offset in range(0, 32, 4):
        voltages.append(int.from_bytes(payload[offset:offset + 2], "big") * 0.0016)
        currents.append(int.from_bytes(payload[offset + 2:offset + 4], "big", signed=True) * 0.0025)
    return VoltageStatus(tuple(voltages), tuple(currents), time.monotonic() if received_at is None else received_at)
```

- [ ] **Step 4: Run protocol tests**

Run:

```powershell
conda run -n VISA python -m unittest Code.Debugs.test_drivers.VoltageProtocolTests -v
```

Expected: 4 tests PASS.

### Task 2: Connection, Reader Thread, and Verified Startup Zero

**Files:**
- Modify: `Code/Utils/voltage.py`
- Modify: `Code/Debugs/test_drivers.py`

**Interfaces:**
- Consumes: `find_serial_port`, `decode_telemetry`, `encode_voltages`.
- Produces: `VoltageSource.connect()`, `VoltageSource.read_status(max_age=1.0)`, context-manager entry; constructor injection points `serial_factory`, `ports_provider`, `clock`, and `sleep`.

- [ ] **Step 1: Add a deterministic fake serial transport and failing tests**

Append a `FakeVoltageSerial` that stores writes, returns queued frames from `read_until`, exposes `is_open`, and unblocks reads after `close()`. Add tests that:

```python
class VoltageConnectionTests(unittest.TestCase):
    def test_connect_reads_frame_zeros_and_confirms_new_zero_status(self):
        initial = make_voltage_frame([1.0] * 8, [0.0] * 8)
        zeroed = make_voltage_frame([0.0] * 8, [0.0] * 8)
        fake = FakeVoltageSerial([initial, zeroed])
        driver = VoltageSource(port="COM4", serial_factory=lambda **kwargs: fake, startup_timeout=1.0)
        driver.connect()
        self.assertEqual(fake.writes[-1], encode_voltages([0.0] * 8))
        self.assertTrue(all(abs(value) < 0.05 for value in driver.read_status().voltage_v))
        driver.close()
        self.assertFalse(fake.is_open)

    def test_connect_fails_if_zero_is_not_confirmed(self):
        nonzero = make_voltage_frame([1.0] * 8, [0.0] * 8)
        fake = FakeVoltageSerial([nonzero, nonzero])
        driver = VoltageSource(port="COM4", serial_factory=lambda **kwargs: fake, startup_timeout=0.05)
        with self.assertRaises(InstrumentTimeoutError):
            driver.connect()
        self.assertEqual(driver.state, DriverState.DISCONNECTED)
```

Implement `make_voltage_frame()` using inverse telemetry scales; do not call the real port provider in these tests.

- [ ] **Step 2: Run and verify `VoltageSource` is missing**

Run:

```powershell
conda run -n VISA python -m unittest Code.Debugs.test_drivers.VoltageConnectionTests -v
```

Expected: FAIL because `VoltageSource` is not defined.

- [ ] **Step 3: Implement connection and the reader loop**

Add `VoltageSource` with this exact constructor:

```python
def __init__(
    self,
    port: str | None = None,
    limit: float = 14.0,
    ramp_step: float = 0.1,
    ramp_interval: float = 0.05,
    io_timeout: float = 0.5,
    startup_timeout: float = 3.0,
    telemetry_timeout: float = 1.0,
    serial_factory=serial.Serial,
    ports_provider=list_ports.comports,
    clock=time.monotonic,
    sleep=time.sleep,
) -> None:
```

Validate `0 < limit <= 14`, positive steps/timeouts, and store injected dependencies. Use a `threading.RLock`, `threading.Condition`, stop event, reader-thread reference, latest-status reference, malformed-frame counter, and `_commanded = [0.0] * 8`.

`connect()` must:

1. Return immediately if already `READY` or `ACTIVE`.
2. Resolve `port` with `find_serial_port(0x1A86, 0x7523, ports_provider=...)` when omitted.
3. Open serial using named parameters `port`, `baudrate=115200`, `bytesize=8`, `parity="N"`, `stopbits=1`, `timeout=io_timeout`, `write_timeout=io_timeout`.
4. Start the reader thread.
5. Wait for one valid status under the condition.
6. Record the send timestamp, write `encode_voltages([0.0] * 8, limit)`, then wait for a newer status whose eight absolute voltages are below 0.05 V.
7. Enter `READY`; on any failure call `close()` and re-raise a typed driver error.

The reader loop calls `read_until(b"\r\n")`, parses only exact frames, publishes status under the condition, and tolerates isolated malformed frames. Three consecutive malformed frames invoke `_enter_fault()`.

`read_status(max_age=1.0)` returns the latest immutable snapshot only in `READY` or `ACTIVE`; it raises `InstrumentTimeoutError` when missing or stale.

- [ ] **Step 4: Run connection tests**

Run:

```powershell
conda run -n VISA python -m unittest Code.Debugs.test_drivers.VoltageConnectionTests -v
```

Expected: 2 tests PASS and reader threads terminate.

### Task 3: Smooth Control, Emergency Fault, and Idempotent Cleanup

**Files:**
- Modify: `Code/Utils/voltage.py`
- Modify: `Code/Debugs/test_drivers.py`

**Interfaces:**
- Consumes: connected `VoltageSource` from Task 2.
- Produces: `set_channel(channel, voltage)`, `set_all(values)`, `ramp_to(values)`, `zero(emergency=False)`, `close()`.

- [ ] **Step 1: Add failing control and cleanup tests**

Add tests for:

```python
class VoltageControlTests(unittest.TestCase):
    def test_ramp_never_exceeds_step_and_reaches_target(self):
        driver, fake = connected_voltage_driver(ramp_step=0.1, ramp_interval=0.0)
        fake.writes.clear()
        driver.set_channel(1, 0.25)
        decoded = [decode_command(frame)[0] for frame in fake.writes]
        self.assertGreaterEqual(len(decoded), 3)
        self.assertTrue(all(0 < b - a <= 0.1005 for a, b in zip([0.0] + decoded[:-1], decoded)))
        self.assertAlmostEqual(decoded[-1], 0.25, places=3)
        driver.close()

    def test_invalid_target_writes_nothing(self):
        driver, fake = connected_voltage_driver()
        fake.writes.clear()
        with self.assertRaises(InstrumentSafetyError):
            driver.set_channel(1, 14.1)
        self.assertEqual(fake.writes, [])
        driver.close()

    def test_close_sends_immediate_zero_twice_safely(self):
        driver, fake = connected_voltage_driver()
        driver.close()
        writes_after_first = list(fake.writes)
        driver.close()
        self.assertEqual(fake.writes, writes_after_first)
        self.assertEqual(writes_after_first[-1], encode_voltages([0.0] * 8))
```

`connected_voltage_driver()` must queue startup frames and return a ready driver. `decode_command()` reverses the command scale for test inspection.

- [ ] **Step 2: Run and confirm methods are missing**

Run:

```powershell
conda run -n VISA python -m unittest Code.Debugs.test_drivers.VoltageControlTests -v
```

Expected: FAIL because control and cleanup methods are incomplete.

- [ ] **Step 3: Implement control and cleanup**

Implement these rules:

- Validate the complete target through `encode_voltages()` before acquiring the operation lock or writing.
- `set_channel()` accepts channel numbers 1-8, updates one value from `_commanded`, and delegates to `ramp_to()`.
- `set_all()` delegates to `ramp_to()`.
- `ramp_to()` computes `steps = max(1, ceil(max_delta / ramp_step))`, linearly interpolates all eight channels, validates and writes every frame, sleeps only between frames, and updates `_commanded` after each successful write.
- `zero(emergency=True)` writes one all-zero frame immediately and resets `_commanded`; `zero(emergency=False)` ramps normally.
- `_enter_fault(error)` latches `FAULT`, attempts exactly one immediate zero under the operation lock, logs failures, and signals the stop event.
- `close()` returns when already `DISCONNECTED`; otherwise it sets `CLOSING`, attempts one immediate zero if serial is open, signals the reader stop event, closes serial to unblock reads, joins the reader for at most `io_timeout + 1` seconds, clears resources, and sets `DISCONNECTED`.
- `__exit__` always calls `close()` and never suppresses the caller's exception.

- [ ] **Step 4: Run every voltage offline test**

Run:

```powershell
conda run -n VISA python -m unittest Code.Debugs.test_drivers.VoltageProtocolTests Code.Debugs.test_drivers.VoltageConnectionTests Code.Debugs.test_drivers.VoltageControlTests -v
```

Expected: 9 tests PASS with no surviving reader thread.

### Task 4: Public Export and Hardware Check

**Files:**
- Modify: `Code/Utils/__init__.py`
- Create: `Code/Debugs/check_voltage.py`

**Interfaces:**
- Consumes: completed `VoltageSource` API.
- Produces: safe real-device telemetry and optional 0.1 V CH1 test.

- [ ] **Step 1: Export voltage symbols**

Export `VoltageSource` and `VoltageStatus` from `Code/Utils/__init__.py` and add both to `__all__`.

- [ ] **Step 2: Create the real-device diagnostic**

Create a `main()` that:

1. Accepts optional `--port`.
2. Opens `VoltageSource(port=args.port)` in a `with` block.
3. Prints all eight measured voltage/current pairs after verified startup zero.
4. Asks exactly `Apply 0.1 V to CH1 and then return to zero? [y/N]`.
5. Only for a normalized `y` response, calls `set_channel(1, 0.1)`, waits 0.2 seconds, prints a fresh status, and calls `zero(emergency=True)` in an inner `finally`.
6. Relies on outer context cleanup for a second safe zero and port close.

- [ ] **Step 3: Verify syntax/help and run offline suite**

Run:

```powershell
conda run -n VISA python -m py_compile Code\Utils\voltage.py Code\Debugs\check_voltage.py
conda run -n VISA python -m Code.Debugs.check_voltage --help
conda run -n VISA python -m unittest Code.Debugs.test_drivers -v
```

Expected: syntax/help exit 0 and all accumulated tests PASS without opening hardware.

- [ ] **Step 4: Run hardware diagnostic only with explicit state-change approval**

Run:

```powershell
conda run -n VISA python -m Code.Debugs.check_voltage
```

Expected: CH340 is uniquely found, valid eight-channel telemetry prints, startup zero is confirmed, and any approved 0.1 V test returns CH1 to zero even on interruption.

- [ ] **Step 5: Record the checkpoint**

Record port, telemetry frame validity, startup-zero result, optional 0.1 V measured result, final zero result, and test count. Do not commit because the workspace is not a Git repository.
