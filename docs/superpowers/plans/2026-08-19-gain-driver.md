# Gain Chip Driver Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement a serialized request/response Gain Chip Driver with strict 15-40 degC and 0-200 mA limits, temperature stabilization interlock, continuous watchdog, latched severe faults, and current-before-TEC shutdown.

**Architecture:** Keep ASCII protocol formatting/parsing pure and testable. A single reentrant lock owns every serial command so foreground calls and the watchdog cannot consume one another's replies; immutable status and a condition variable carry watchdog observations to interlock checks.

**Tech Stack:** Python 3.10, PySerial 3.5, standard-library `dataclasses`, `threading`, `time`, and `unittest`, Anaconda environment `VISA`.

**Spec:** `docs/superpowers/specs/2026-08-19-instrument-drivers-design.md`

## Global Constraints

- Complete `docs/superpowers/plans/2026-08-19-driver-foundation.md` first.
- Auto-discovery uses CP210x VID:PID `10C4:EA60`; prefer serial `E42E432326A3ED118B3D99412981D5C7` when present; serial is 115200 8N1.
- Commands and responses use ASCII CRLF framing.
- Hard target-temperature range is 15-40 degC; hard current range is 0-200 mA.
- Current enable requires TEC on and five consecutive one-second samples within target +/-0.2 degC.
- Shutdown always attempts current off before TEC off.
- The current workspace is not a Git repository; use passing tests as checkpoints.

## Locked File Structure

- Create `Code/Utils/gain.py`: protocol, status, request serialization, interlock, watchdog, and cleanup.
- Modify `Code/Utils/__init__.py`: exports.
- Modify `Code/Debugs/test_drivers.py`: protocol, safety, watchdog, and cleanup tests.
- Create `Code/Debugs/check_gain.py`: real CP210x read/no-op/optional output diagnostic.

---

### Task 1: ASCII Protocol and Strict Reply Parsing

**Files:**
- Create: `Code/Utils/gain.py`
- Modify: `Code/Debugs/test_drivers.py`

**Interfaces:**
- Consumes: common protocol/safety errors.
- Produces: `format_fixed(value, minimum, maximum, label) -> str`, `build_command(name, value=None, ...) -> bytes`, `parse_reply(line, expected_field) -> str`, `parse_ack(line) -> str`.

- [ ] **Step 1: Add failing protocol tests**

Append:

```python
from Code.Utils.gain import build_command, format_fixed, parse_ack, parse_reply


class GainProtocolTests(unittest.TestCase):
    def test_formats_fixed_three_decimal_value(self):
        self.assertEqual(format_fixed(22.0, 15.0, 40.0, "temperature"), "022000")
        self.assertEqual(format_fixed(200.0, 0.0, 200.0, "current"), "200000")

    def test_rejects_out_of_range_and_nonfinite_value(self):
        for value in (14.999, 40.001, float("nan"), float("inf")):
            with self.assertRaises(InstrumentSafetyError):
                format_fixed(value, 15.0, 40.0, "temperature")

    def test_builds_read_write_and_boolean_commands(self):
        self.assertEqual(build_command("RDTA"), b"RDTA\r\n")
        self.assertEqual(build_command("STEA", 22.0, 15.0, 40.0, "temperature"), b"STEA022000\r\n")
        self.assertEqual(build_command("STQA", True), b"STQA000001\r\n")

    def test_parses_expected_ready_field(self):
        self.assertEqual(parse_reply(b"READY;T=21.456\r\n", "T"), "21.456")

    def test_rejects_wrong_field_bad_prefix_and_missing_terminator(self):
        for reply in (b"READY;E=22.000\r\n", b"ERROR;T=21.456\r\n", b"READY;T=21.456"):
            with self.subTest(reply=reply):
                with self.assertRaises(InstrumentProtocolError):
                    parse_reply(reply, "T")

    def test_parses_generic_ready_acknowledgement(self):
        self.assertEqual(parse_ack(b"READY\r\n"), "READY")
        self.assertEqual(parse_ack(b"READY;P=0.350\r\n"), "READY;P=0.350")
        with self.assertRaises(InstrumentProtocolError):
            parse_ack(b"ERROR\r\n")
```

- [ ] **Step 2: Run and verify the missing-module failure**

Run:

```powershell
conda run -n VISA python -m unittest Code.Debugs.test_drivers.GainProtocolTests -v
```

Expected: FAIL because `Code.Utils.gain` does not exist.

- [ ] **Step 3: Implement strict pure protocol helpers**

Create `Code/Utils/gain.py` with:

```python
from __future__ import annotations

import math
import re

from .common import InstrumentProtocolError, InstrumentSafetyError

_REPLY = re.compile(r"^READY;([A-Z])=([^;\r\n]+)\r\n$")


def format_fixed(value: float, minimum: float, maximum: float, label: str) -> str:
    numeric = float(value)
    if not math.isfinite(numeric) or not minimum <= numeric <= maximum:
        raise InstrumentSafetyError(f"{label} {numeric!r} is outside {minimum}-{maximum}")
    scaled = int(round(numeric * 1000))
    if scaled > 999999:
        raise InstrumentSafetyError(f"{label} cannot fit the six-digit device field")
    return f"{scaled:06d}"


def build_command(
    name: str,
    value: float | bool | None = None,
    minimum: float | None = None,
    maximum: float | None = None,
    label: str = "value",
) -> bytes:
    if value is None:
        payload = name
    elif isinstance(value, bool):
        payload = f"{name}{int(value):06d}"
    else:
        if minimum is None or maximum is None:
            raise ValueError("numeric commands require minimum and maximum")
        payload = name + format_fixed(value, minimum, maximum, label)
    return (payload + "\r\n").encode("ascii")


def parse_reply(line: bytes, expected_field: str) -> str:
    try:
        text = line.decode("ascii")
    except UnicodeDecodeError as error:
        raise InstrumentProtocolError("Gain reply is not ASCII") from error
    match = _REPLY.fullmatch(text)
    if match is None:
        raise InstrumentProtocolError(f"Malformed Gain reply: {text!r}")
    field, value = match.groups()
    if field != expected_field:
        raise InstrumentProtocolError(
            f"Expected Gain reply field {expected_field}, received {field}"
        )
    return value


def parse_ack(line: bytes) -> str:
    try:
        text = line.decode("ascii")
    except UnicodeDecodeError as error:
        raise InstrumentProtocolError("Gain acknowledgement is not ASCII") from error
    stripped = text.removesuffix("\r\n")
    if stripped == text or not (stripped == "READY" or _REPLY.fullmatch(text)):
        raise InstrumentProtocolError(f"Malformed Gain acknowledgement: {text!r}")
    return stripped
```

- [ ] **Step 4: Run protocol tests**

Run:

```powershell
conda run -n VISA python -m unittest Code.Debugs.test_drivers.GainProtocolTests -v
```

Expected: 6 tests PASS.

### Task 2: Serialized Connection and Typed Read/Write API

**Files:**
- Modify: `Code/Utils/gain.py`
- Modify: `Code/Debugs/test_drivers.py`

**Interfaces:**
- Produces: `GainStatus`; `GainDriver.connect()`; temperature/TEC/current reads and writes; `read_pid()`, `set_pid()`, `reset_pid()`, and `clear_integral()`.

- [ ] **Step 1: Add fake serial and failing API tests**

Create `FakeGainSerial` with queued CRLF replies, captured writes, `is_open`, and `close()`. Add:

```python
class GainConnectionTests(unittest.TestCase):
    def test_connect_reads_complete_status(self):
        fake = FakeGainSerial([
            b"READY;T=21.456\r\n",
            b"READY;E=22.000\r\n",
            b"READY;D=1\r\n",
            b"READY;C=150.000\r\n",
            b"READY;Q=0\r\n",
        ])
        driver = GainDriver(port="COM5", serial_factory=lambda **kwargs: fake, start_watchdog=False)
        driver.connect()
        self.assertAlmostEqual(driver.status.temperature_c, 21.456)
        self.assertAlmostEqual(driver.status.target_c, 22.0)
        self.assertTrue(driver.status.tec_enabled)
        self.assertFalse(driver.status.current_enabled)
        self.assertEqual(fake.writes[:5], [b"RDTA\r\n", b"RDEA\r\n", b"RDRA\r\n", b"RDCA\r\n", b"RDQA\r\n"])
        driver.close()
        self.assertEqual(fake.writes[-2:], [b"STQA000000\r\n", b"STRA000000\r\n"])

    def test_wrong_reply_field_closes_connection(self):
        fake = FakeGainSerial([b"READY;E=22.000\r\n"])
        driver = GainDriver(port="COM5", serial_factory=lambda **kwargs: fake, start_watchdog=False)
        with self.assertRaises(InstrumentProtocolError):
            driver.connect()
        self.assertFalse(fake.is_open)
```

- [ ] **Step 2: Run and confirm `GainDriver` is missing**

Run:

```powershell
conda run -n VISA python -m unittest Code.Debugs.test_drivers.GainConnectionTests -v
```

Expected: FAIL because the stateful driver is not defined.

- [ ] **Step 3: Implement status, serialized query, and public reads/writes**

Use this immutable status:

```python
@dataclass(frozen=True)
class GainStatus:
    temperature_c: float
    target_c: float
    tec_enabled: bool
    current_ma: float
    current_enabled: bool
    received_at: float
```

Use this constructor:

```python
def __init__(
    self,
    port: str | None = None,
    usb_serial: str | None = "E42E432326A3ED118B3D99412981D5C7",
    io_timeout: float = 1.0,
    poll_interval: float = 1.0,
    start_watchdog: bool = True,
    serial_factory=serial.Serial,
    ports_provider=list_ports.comports,
    clock=time.monotonic,
    sleep=time.sleep,
) -> None:
```

Connect with CP210x `0x10C4:0xEA60`, falling back from the preferred USB serial to a unique VID/PID match only when no device with that serial exists. Open 115200 8N1 with read/write timeouts. `_request(command, field)` must hold one `threading.RLock`, write the command, call `read_until(b"\r\n")`, and parse the exact expected field.

Implement typed conversions and field validation:

- `RDTA -> float T`
- `RDEA -> float E`
- `RDRA -> bool R`, accepting only `0` or `1` (confirmed on real hardware; `STRA` write acknowledgements remain field `D`)
- `RDCA -> float C`
- `RDQA -> bool Q`, accepting only `0` or `1`
- `RDPA -> float P`, `RDIA -> float I`, and `RDDA -> float D`

`connect()` queries those five fields in order, constructs `GainStatus`, enters `READY`, and starts the watchdog only when `start_watchdog=True`. On partial failure, attempt current off and TEC off only when the serial port was opened, then close.

Write methods issue the matching command and require the matching reply:

- `set_temperature(value)` sends `STEA` and expects `E`.
- `set_current(value)` sends `STCA` and expects `C`.
- `enable_tec()` sends `STRA000001` and expects `D=1`.
- `disable_tec()` sends `STRA000000` and expects `D=0`.
- `disable_current()` sends `STQA000000` and expects `Q=0`.
- `read_pid()` sends `RDPA`, `RDIA`, and `RDDA`, requires `P`, `I`, and `D`, and returns a three-float tuple.
- `set_pid(p, i, d)` validates each coefficient in the six-digit protocol range 0-999.999, sends `STPA`, `STIA`, and `STDA`, requires matching `P`, `I`, and `D` replies, updates only after all three succeed, and returns the applied tuple.
- `reset_pid()` sends `RST`, consumes one `parse_ack()` response, then returns a fresh `read_pid()` result instead of assuming the firmware's reset values.
- `clear_integral()` sends `CLR`, consumes one `parse_ack()` response, and returns normally only after the acknowledgement validates.

After every successful setting or state command, replace the immutable `GainStatus` with a copy containing the confirmed target, current, TEC state, or current-output state. After every temperature poll, replace its temperature and timestamp. This keeps interlocks and callers from using stale state.

- [ ] **Step 4: Run connection/API tests**

Run:

```powershell
conda run -n VISA python -m unittest Code.Debugs.test_drivers.GainConnectionTests -v
```

Expected: 2 tests PASS, replies never cross requests, and close order is current then TEC.

### Task 3: Stabilization Interlock and Watchdog Faults

**Files:**
- Modify: `Code/Utils/gain.py`
- Modify: `Code/Debugs/test_drivers.py`

**Interfaces:**
- Produces: `wait_stable(timeout=60.0)`, `enable_current()`, watchdog loop, latched fault behavior.

- [ ] **Step 1: Add failing interlock/watchdog tests**

Add deterministic tests with injected clock/sleep and reply queues:

```python
class GainSafetyTests(unittest.TestCase):
    def test_enable_current_requires_tec_and_five_stable_samples(self):
        driver, fake = ready_gain_driver(start_watchdog=False, tec_enabled=True, target=22.0)
        driver._stable_samples = 4
        driver._latest_temperature = 22.05
        with self.assertRaises(InstrumentSafetyError):
            driver.enable_current()
        driver._stable_samples = 5
        fake.queue(b"READY;Q=1\r\n")
        driver.enable_current()
        self.assertEqual(fake.writes[-1], b"STQA000001\r\n")
        driver.close()

    def test_three_moderate_deviations_disable_current_only(self):
        driver, fake = ready_gain_driver(start_watchdog=False, tec_enabled=True, current_enabled=True, target=22.0)
        fake.queue_temperature(23.2, 23.2, 23.2)
        driver._watchdog_iteration(); driver._watchdog_iteration(); driver._watchdog_iteration()
        self.assertIn(b"STQA000000\r\n", fake.writes)
        self.assertNotIn(b"STRA000000\r\n", fake.writes)
        self.assertNotEqual(driver.state, DriverState.FAULT)
        driver.close()

    def test_severe_deviation_latches_fault_and_orders_shutdown(self):
        driver, fake = ready_gain_driver(start_watchdog=False, tec_enabled=True, current_enabled=True, target=22.0)
        fake.queue_temperature(25.1)
        driver._watchdog_iteration()
        self.assertEqual(driver.state, DriverState.FAULT)
        shutdown_writes = [
            item for item in fake.writes
            if item in {b"STQA000000\r\n", b"STRA000000\r\n"}
        ]
        self.assertEqual(shutdown_writes[-2:], [b"STQA000000\r\n", b"STRA000000\r\n"])
```

The fake must queue matching `Q=0` and `D=0` replies after trip commands.

- [ ] **Step 2: Run and confirm safety methods are missing**

Run:

```powershell
conda run -n VISA python -m unittest Code.Debugs.test_drivers.GainSafetyTests -v
```

Expected: FAIL because interlock/watchdog behavior is incomplete.

- [ ] **Step 3: Implement stabilization and watchdog state**

Maintain under a condition lock:

- `_latest_temperature`
- `_stable_samples`
- `_moderate_deviation_samples`
- `_read_failures`
- `_stop_watchdog`
- `_watchdog_thread`

One `_watchdog_iteration()` performs `RDTA` and applies these exact rules:

1. Successful read resets `_read_failures`.
2. `abs(T-target) <= 0.2` increments `_stable_samples`; any larger deviation resets it.
3. `abs(T-target) > 1.0` increments `_moderate_deviation_samples`; otherwise reset it.
4. One `abs(T-target) > 3.0` immediately calls severe trip.
5. Three moderate samples call `disable_current()` but leave TEC enabled and do not auto-reenable current.
6. Three consecutive read/protocol failures call severe trip.
7. Severe trip attempts `STQA000000` then `STRA000000`, latches `FAULT`, and stops the watchdog.

`wait_stable(timeout=60.0)` waits on the condition until at least five stable samples or raises `InstrumentTimeoutError`. `enable_current()` requires `READY`, current setpoint in 0-200 mA, TEC reported on, latest temperature within 0.2 degC, and at least five stable samples before sending `STQA000001` and requiring `Q=1`.

The watchdog loop uses the stop event's timed wait rather than an uninterruptible sleep. It catches all request errors, counts them, and never silently dies.

- [ ] **Step 4: Run all Gain offline tests**

Run:

```powershell
conda run -n VISA python -m unittest Code.Debugs.test_drivers.GainProtocolTests Code.Debugs.test_drivers.GainConnectionTests Code.Debugs.test_drivers.GainSafetyTests -v
```

Expected: all Gain protocol, connection, and safety tests PASS with no surviving watchdog thread.

### Task 4: Idempotent Shutdown, Export, and Hardware Diagnostic

**Files:**
- Modify: `Code/Utils/gain.py`
- Modify: `Code/Utils/__init__.py`
- Modify: `Code/Debugs/test_drivers.py`
- Create: `Code/Debugs/check_gain.py`

**Interfaces:**
- Produces: final `close()`/context-manager behavior and real-device check.

- [ ] **Step 1: Add failing shutdown-order test**

Add a test that opens a ready fake driver, calls `close()` twice, and asserts:

- Exactly one close sequence is issued.
- `STQA000000` precedes `STRA000000`.
- Both replies are consumed.
- The watchdog stop event is set, its thread is joined with a bound, serial is closed, and final state is `DISCONNECTED`.
- An exception raised inside `with GainDriver(...)` remains the propagated exception after cleanup.

- [ ] **Step 2: Implement final cleanup**

`close()` must be idempotent. Mark `CLOSING` and signal the watchdog to prevent new polls, then under the serial request lock attempt current off followed by TEC off, join the watchdog for at most `io_timeout + poll_interval + 1`, close serial, clear resources, and set `DISCONNECTED`. Each cleanup step has its own `try/except` log path so TEC shutdown is attempted even if current shutdown fails. `__exit__` calls `close()` and returns `None`.

- [ ] **Step 3: Export public types**

Export `GainDriver` and `GainStatus` from `Code/Utils/__init__.py` and add both to `__all__`.

- [ ] **Step 4: Create `check_gain.py`**

The diagnostic must:

1. Accept optional `--port` and `--serial-number`.
2. Connect and print raw-confirmed temperature, target, TEC state, current setpoint, current state, and PID coefficients.
3. Confirm that responses end in CRLF; abort before any write if not.
4. Write the same target temperature, current setpoint, and PID coefficients back and verify replies, causing no intended setpoint change. Do not invoke `RST` or `CLR` during the default check because those intentionally alter controller state.
5. Ask before any TEC/current enable sequence.
6. If approved, require the user to provide a 0-200 mA test current, enable TEC, call `wait_stable()`, set current, enable current, hold for a user-specified bounded duration, then disable current in an inner `finally`.
7. Rely on outer context cleanup to disable current again, disable TEC, stop watchdog, and close serial.

- [ ] **Step 5: Run syntax and offline verification**

Run:

```powershell
conda run -n VISA python -m py_compile Code\Utils\gain.py Code\Debugs\check_gain.py
conda run -n VISA python -m Code.Debugs.check_gain --help
conda run -n VISA python -m unittest Code.Debugs.test_drivers -v
```

Expected: syntax/help exit 0 and all accumulated tests PASS without opening hardware.

- [ ] **Step 6: Run hardware diagnostic only with explicit output approval**

Run:

```powershell
conda run -n VISA python -m Code.Debugs.check_gain
```

Expected: CP210x is uniquely identified, all reads and no-op writes agree, CRLF is confirmed, and any explicitly approved output test satisfies stabilization and returns current/TEC off.

- [ ] **Step 7: Record the checkpoint**

Record port/serial, response framing, read/no-op results, watchdog behavior, final current/TEC state, and test count. Do not commit because the workspace is not a Git repository.
