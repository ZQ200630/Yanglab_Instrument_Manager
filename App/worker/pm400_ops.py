"""Finite, typed UI registry for the PM400 public driver API.

Request strings select only entries declared here. They never become raw SCPI
or arbitrary method names. The PM400 driver remains the final safety gate.
"""

from __future__ import annotations

import datetime
import math
from dataclasses import dataclass
from typing import Any

from Code.Utils import AdapterType, LimitSelector, MeasurementKind, PowerUnit, StatusGroup


@dataclass(frozen=True)
class Setting:
    key: str
    label: str
    kind: str
    reader: str | None
    writer: str | None
    read_caps: tuple[str, ...] = ()
    write_caps: tuple[str, ...] = ()
    selectors: tuple[str, ...] = ()
    sensitive: bool = False
    grouped: bool = False

    @property
    def section(self) -> str:
        return self.key.split(".", 1)[0]


def _s(key: str, label: str, kind: str, *, access: str = "rw",
       caps: tuple[str, ...] = (), read_caps: tuple[str, ...] | None = None,
       write_caps: tuple[str, ...] | None = None,
       selectors: str = "", sensitive: bool = False, grouped: bool = False,
       reader: str | None = None, writer: str | None = None) -> Setting:
    name = key.split(".", 1)[1]
    return Setting(
        key, label, kind,
        reader or (f"get_{name}" if "r" in access else None),
        writer or (f"set_{name}" if "w" in access else None),
        caps if read_caps is None else read_caps,
        caps if write_caps is None else write_caps,
        tuple(selectors.split()) if selectors else (), sensitive, grouped,
    )


SETTINGS: tuple[Setting, ...] = (
    _s("root.identity", "Instrument identity", "text", access="r", reader="identify"),
    _s("root.standard_event_enable", "Standard event enable", "register8"),
    _s("root.standard_event_status", "Standard event status", "register8", access="r",
       reader="read_standard_event_status"),
    _s("root.service_request_enable", "Service request enable", "register8"),
    _s("root.status_byte", "Status byte", "register8", access="r", reader="read_status_byte"),
    _s("root.self_test", "Self-test result", "int", access="r", reader="self_test"),
    _s("system.beeper_enabled", "Beeper enabled", "bool"),
    _s("system.date", "System date", "date"),
    _s("system.time", "System time", "time"),
    _s("system.line_frequency_hz", "Line frequency", "line_frequency"),
    _s("system.scpi_version", "SCPI version", "text", access="r"),
    _s("system.sensor_info", "Sensor identity", "object", access="r"),
    _s("system.next_error", "Next system error", "object", access="r", reader="next_error"),
    _s("status.event", "Event register", "register16", access="r", grouped=True,
       reader="read_event"),
    _s("status.condition", "Condition register", "register16", access="r", grouped=True,
       reader="read_condition"),
    _s("status.positive_transition", "Positive transition register", "register16", grouped=True),
    _s("status.negative_transition", "Negative transition register", "register16", grouped=True),
    _s("status.enable", "Status enable register", "register16", grouped=True),
    _s("display.brightness", "Display brightness", "float"),
    _s("display.contrast", "Display contrast", "float"),
    _s("calibration.string", "Calibration string", "text", access="r"),
    _s("sense.average_count", "Average count", "int"),
    _s("sense.loss_db", "Input loss · dB", "float", selectors="minimum maximum default"),
    _s("sense.zero_state", "Sensor zero state", "bool", access="r"),
    _s("sense.zero_magnitude", "Sensor zero magnitude", "float", access="r"),
    _s("sense.beam_diameter_mm", "Beam diameter · mm", "float",
       selectors="minimum maximum default"),
    _s("sense.wavelength_nm", "Wavelength · nm", "float", read_caps=(),
       write_caps=("wavelength_settable",), selectors="minimum maximum"),
    _s("sense.photodiode_response_a_per_w", "Photodiode responsivity · A/W", "float",
       read_caps=("power",), write_caps=("power", "response_settable"),
       selectors="minimum maximum default", sensitive=True),
    _s("sense.thermopile_response_v_per_w", "Thermopile responsivity · V/W", "float",
       read_caps=("power",), write_caps=("power", "response_settable"),
       selectors="minimum maximum default", sensitive=True),
    _s("sense.pyro_response_v_per_j", "Pyroelectric responsivity · V/J", "float",
       read_caps=("energy",), write_caps=("energy", "response_settable"),
       selectors="minimum maximum default", sensitive=True),
    _s("sense.current_auto_range", "Current autorange", "bool", caps=("power",)),
    _s("sense.current_range_a", "Current range · A", "float", caps=("power",),
       selectors="minimum maximum"),
    _s("sense.current_reference_a", "Current reference · A", "float", caps=("power",),
       selectors="minimum maximum default"),
    _s("sense.current_delta_enabled", "Current delta mode", "bool", caps=("power",)),
    _s("sense.energy_range_j", "Energy range · J", "float", caps=("energy",),
       selectors="minimum maximum"),
    _s("sense.energy_reference_j", "Energy reference · J", "float", caps=("energy",),
       selectors="minimum maximum default"),
    _s("sense.energy_delta_enabled", "Energy delta mode", "bool", caps=("energy",)),
    _s("sense.frequency_upper_hz", "Upper frequency limit · Hz", "float", access="r"),
    _s("sense.frequency_lower_hz", "Lower frequency limit · Hz", "float", access="r"),
    _s("sense.power_auto_range", "Power autorange", "bool", caps=("power",)),
    _s("sense.power_range_w", "Power range · W", "float", caps=("power",),
       selectors="minimum maximum"),
    _s("sense.power_reference_w", "Power reference · W", "float", caps=("power",),
       selectors="minimum maximum default"),
    _s("sense.power_delta_enabled", "Power delta mode", "bool", caps=("power",)),
    _s("sense.power_unit", "Power unit", "power_unit", caps=("power",)),
    _s("sense.voltage_auto_range", "Voltage autorange", "bool", caps=("power",)),
    _s("sense.voltage_range_v", "Voltage range · V", "float", caps=("power",),
       selectors="minimum maximum"),
    _s("sense.voltage_reference_v", "Voltage reference · V", "float", caps=("power",),
       selectors="minimum maximum default"),
    _s("sense.voltage_delta_enabled", "Voltage delta mode", "bool", caps=("power",)),
    _s("sense.peak_threshold_percent", "Peak threshold · %", "float", caps=("energy",),
       selectors="minimum maximum default"),
    _s("input.photodiode_lowpass_enabled", "Photodiode low-pass filter", "bool", caps=("power",)),
    _s("input.thermopile_accelerator_enabled", "Thermopile accelerator", "bool", caps=("power",)),
    _s("input.thermopile_accelerator_auto", "Accelerator auto mode", "bool", caps=("power",)),
    _s("input.thermopile_tau_s", "Thermopile time constant · s", "float",
       read_caps=("power",), write_caps=("power", "tau_settable"),
       selectors="minimum maximum default"),
    _s("input.adapter_type", "Adapter type", "adapter_type", sensitive=True),
    _s("measurement.configuration", "Measurement configuration", "measurement_kind",
       reader="get_configuration", writer="configure"),
)


@dataclass(frozen=True)
class CommandSpec:
    key: str
    label: str
    method: str
    confirm: bool = False
    capabilities: tuple[str, ...] = ()
    driver_confirm: bool = False

    @property
    def section(self) -> str:
        return self.key.split(".", 1)[0]


COMMANDS: tuple[CommandSpec, ...] = (
    CommandSpec("root.clear_status", "Clear status registers", "clear_status", confirm=True),
    CommandSpec("root.mark_operation_complete", "Mark operation complete", "mark_operation_complete"),
    CommandSpec("root.wait_operation_complete", "Wait for operation complete", "wait_operation_complete"),
    CommandSpec("root.reset", "Instrument reset", "reset", confirm=True, driver_confirm=True),
    CommandSpec("root.wait_to_continue", "Wait to continue", "wait_to_continue"),
    CommandSpec("root.cancel_measurement", "Cancel current measurement", "cancel_measurement"),
    CommandSpec("system.beep", "Beep once", "beep"),
    CommandSpec("system.drain_errors", "Read error queue", "drain_errors"),
    CommandSpec("status.preset", "Status preset", "preset", confirm=True, driver_confirm=True),
    CommandSpec("sense.start_zero_collection", "Sensor zero calibration", "start_zero_collection",
                confirm=True, driver_confirm=True),
    CommandSpec("sense.abort_zero_collection", "Abort sensor zero calibration", "abort_zero_collection"),
    CommandSpec("measurement.initiate", "Initiate measurement", "initiate"),
    CommandSpec("measurement.abort", "Abort measurement", "abort"),
    CommandSpec("measurement.fetch", "Fetch last measurement", "fetch"),
    CommandSpec("measurement.read", "Read configured measurement", "read"),
)


MEASUREMENTS: dict[str, tuple[MeasurementKind, tuple[str, ...], str]] = {
    "power": (MeasurementKind.POWER, ("power",), "Optical power"),
    "current": (MeasurementKind.CURRENT, (), "Current"),
    "voltage": (MeasurementKind.VOLTAGE, (), "Voltage"),
    "energy": (MeasurementKind.ENERGY, ("energy",), "Energy"),
    "frequency": (MeasurementKind.FREQUENCY, (), "Frequency"),
    "power_density": (MeasurementKind.POWER_DENSITY, ("power",), "Power density"),
    "energy_density": (MeasurementKind.ENERGY_DENSITY, ("energy",), "Energy density"),
    "resistance": (MeasurementKind.RESISTANCE, (), "Resistance"),
    "temperature": (MeasurementKind.TEMPERATURE, ("temperature_sensor",), "Temperature"),
}

_SETTING_BY_KEY = {setting.key: setting for setting in SETTINGS}
_COMMAND_BY_KEY = {command.key: command for command in COMMANDS}


class PM400InvocationError(RuntimeError):
    """The public driver method was entered; its effect may be unknown."""


def _invoke(method: Any, *args: Any, **kwargs: Any) -> Any:
    try:
        return method(*args, **kwargs)
    except Exception as error:
        raise PM400InvocationError(f"{type(error).__name__}: {error}") from error


def _capabilities(device: object) -> object | None:
    sensor = getattr(device, "sensor_info", None)
    return getattr(sensor, "capabilities", None)


def _supported(device: object, required: tuple[str, ...]) -> bool:
    if not required:
        return True
    capabilities = _capabilities(device)
    return capabilities is not None and all(
        getattr(capabilities, name, False) is True for name in required
    )


def _require_supported(device: object, required: tuple[str, ...]) -> None:
    if not _supported(device, required):
        raise ValueError(f"PM400 sensor capability unavailable: {', '.join(required)}")


def catalog(device: object) -> dict[str, Any]:
    """Pure metadata; does not query or write the instrument."""
    measurements = [
        {"key": key, "label": label, "unit": kind.default_unit,
         "supported": _supported(device, required), "requirements": list(required)}
        for key, (kind, required, label) in MEASUREMENTS.items()
    ]
    return {
        "measurements": measurements,
        "settings": [
            {"key": spec.key, "section": spec.section, "label": spec.label,
             "kind": spec.kind, "reader": spec.reader, "writer": spec.writer,
             "readable": spec.reader is not None, "writable": spec.writer is not None,
             "read_supported": spec.reader is not None and _supported(device, spec.read_caps),
             "write_supported": spec.writer is not None and _supported(device, spec.write_caps),
             "supported": _supported(device, spec.write_caps if spec.writer else spec.read_caps),
             "read_requirements": list(spec.read_caps),
             "write_requirements": list(spec.write_caps),
             "selectors": list(spec.selectors), "sensitive": spec.sensitive,
             "grouped": spec.grouped,
             "choices": measurements if spec.kind == "measurement_kind" else []}
            for spec in SETTINGS
        ],
        "commands": [
            {"key": spec.key, "section": spec.section, "label": spec.label,
             "method": spec.method, "confirm": spec.confirm,
             "supported": _supported(device, spec.capabilities),
             "requirements": list(spec.capabilities)}
            for spec in COMMANDS
        ],
    }


def _owner(device: object, section: str) -> object:
    return device if section == "root" else getattr(device, section)


def _group(params: dict[str, Any]) -> StatusGroup:
    value = params.get("group")
    if type(value) is not str:
        raise ValueError("PM400 status group is required")
    try:
        return StatusGroup[value.upper()]
    except KeyError as error:
        raise ValueError("unknown PM400 status group") from error


def _selector(params: dict[str, Any], allowed: tuple[str, ...]) -> LimitSelector | None:
    value = params.get("selector")
    if value is None:
        return None
    if type(value) is not str or value.lower() not in allowed:
        raise ValueError("unsupported PM400 limit selector")
    return LimitSelector[value.upper()]


def _value(kind: str, raw: Any) -> Any:
    if kind == "bool":
        if type(raw) is not bool:
            raise ValueError("PM400 value must be boolean")
        return raw
    if kind in {"int", "register8", "register16", "line_frequency"}:
        if type(raw) is not int:
            raise ValueError("PM400 value must be an integer")
        return raw
    if kind == "float":
        if type(raw) not in (int, float) or not math.isfinite(raw):
            raise ValueError("PM400 value must be a finite number")
        return float(raw)
    if kind == "date":
        if type(raw) is not str:
            raise ValueError("PM400 date must be ISO text")
        return datetime.date.fromisoformat(raw)
    if kind == "time":
        if type(raw) is not str:
            raise ValueError("PM400 time must be ISO text")
        value = datetime.time.fromisoformat(raw)
        if value.tzinfo is not None:
            raise ValueError("PM400 time must be local, without timezone")
        return value
    if kind == "power_unit":
        if type(raw) is not str or raw not in {"W", "DBM"}:
            raise ValueError("PM400 power unit must be W or DBM")
        return PowerUnit(raw)
    if kind == "adapter_type":
        mapping = {"photodiode": AdapterType.PHOTODIODE,
                   "thermal": AdapterType.THERMAL, "pyro": AdapterType.PYRO}
        if type(raw) is not str or raw.lower() not in mapping:
            raise ValueError("unknown PM400 adapter type")
        return mapping[raw.lower()]
    if kind == "measurement_kind":
        if type(raw) is not str or raw not in MEASUREMENTS:
            raise ValueError("unknown PM400 measurement kind")
        return MEASUREMENTS[raw][0]
    raise ValueError("PM400 setting is not writable")


def execute(device: object, name: str, params: dict[str, Any]) -> Any:
    """Execute exactly one declared public-driver action, after local gates."""
    if type(params) is not dict:
        raise ValueError("PM400 parameters must be an object")
    if name == "measure":
        if set(params) != {"kind"} or type(params.get("kind")) is not str:
            raise ValueError("PM400 measurement kind is required")
        entry = MEASUREMENTS.get(params["kind"])
        if entry is None:
            raise ValueError("unknown PM400 measurement kind")
        kind, requirements, _label = entry
        _require_supported(device, requirements)
        return _invoke(device.measurement.measure, kind)
    if name in {"read", "write"}:
        key = params.get("setting")
        spec = _SETTING_BY_KEY.get(key) if type(key) is str else None
        if spec is None:
            raise ValueError("unknown PM400 setting")
        if name == "read":
            if set(params) - {"setting", "group", "selector"}:
                raise ValueError("unexpected PM400 read parameter")
            if spec.reader is None:
                raise ValueError("PM400 setting is not readable")
            _require_supported(device, spec.read_caps)
            method = getattr(_owner(device, spec.section), spec.reader)
            if spec.grouped:
                return _invoke(method, _group(params))
            selector = _selector(params, spec.selectors)
            return _invoke(method, selector) if selector is not None else _invoke(method)
        if set(params) - {"setting", "value", "group", "confirm", "selector"}:
            raise ValueError("unexpected PM400 write parameter")
        if spec.writer is None:
            raise ValueError("PM400 setting is not writable")
        if spec.sensitive and params.get("confirm") is not True:
            raise ValueError("PM400 setting requires explicit confirm=True")
        _require_supported(device, spec.write_caps)
        selector = _selector(params, spec.selectors)
        if selector is not None and "value" in params:
            raise ValueError("choose either a PM400 value or limit selector")
        value = selector if selector is not None else _value(spec.kind, params.get("value"))
        if spec.kind == "measurement_kind":
            _require_supported(device, MEASUREMENTS[params["value"]][1])
        method = getattr(_owner(device, spec.section), spec.writer)
        if spec.grouped:
            return _invoke(method, _group(params), value)
        if spec.sensitive:
            return _invoke(method, value, confirm=True)
        return _invoke(method, value)
    if name == "command":
        if set(params) - {"command", "confirm"}:
            raise ValueError("unexpected PM400 command parameter")
        key = params.get("command")
        spec = _COMMAND_BY_KEY.get(key) if type(key) is str else None
        if spec is None:
            raise ValueError("unknown PM400 command")
        if spec.confirm and params.get("confirm") is not True:
            raise ValueError("PM400 command requires explicit confirm=True")
        _require_supported(device, spec.capabilities)
        method = getattr(_owner(device, spec.section), spec.method)
        return _invoke(method, confirm=True) if spec.driver_confirm else _invoke(method)
    raise ValueError("PM400 action is not exposed")
