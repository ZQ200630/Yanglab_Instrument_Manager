from __future__ import annotations

import csv
import datetime
import math
import operator
import re
import threading
import time
from dataclasses import dataclass
from enum import Enum
from numbers import Integral, Real
from typing import Callable, ClassVar

import pyvisa
from pyvisa.constants import StatusCode
from pyvisa.errors import InvalidSession, VisaIOError

from Code.Utils.common import (
    DeviceFault,
    DriverError,
    DriverState,
    InstrumentCapabilityError,
    InstrumentConnectionError,
    InstrumentProtocolError,
    InstrumentSafetyError,
    InstrumentTimeoutError,
    get_logger,
    ProbeReport, _identity_probe, _probe_connect_guard,
)
from Code.Utils.visa import (
    VisaManagerLease,
    VisaResourceReservation,
    acquire_visa_manager,
)


class MeasurementKind(Enum):
    POWER = ("POWer", "W")
    CURRENT = ("CURRent:DC", "A")
    VOLTAGE = ("VOLTage:DC", "V")
    ENERGY = ("ENERgy", "J")
    FREQUENCY = ("FREQuency", "Hz")
    POWER_DENSITY = ("PDENsity", "W/cm^2")
    ENERGY_DENSITY = ("EDENsity", "J/cm^2")
    RESISTANCE = ("RESistance", "ohm")
    TEMPERATURE = ("TEMPerature", "degC")

    @property
    def scpi_suffix(self) -> str:
        return self.value[0]

    @property
    def default_unit(self) -> str:
        return self.value[1]


class PowerUnit(Enum):
    WATTS = "W"
    DBM = "DBM"


class AdapterType(Enum):
    PHOTODIODE = "PHOTodiode"
    THERMAL = "THERmal"
    PYRO = "PYRo"


class StatusGroup(Enum):
    MEASUREMENT = "MEASurement"
    AUXILIARY = "AUXiliary"
    OPERATION = "OPERation"
    QUESTIONABLE = "QUEStionable"


class LimitSelector(Enum):
    MINIMUM = "MINimum"
    MAXIMUM = "MAXimum"
    DEFAULT = "DEFault"


def _parse_csv_response(response: str, expected_fields: int, context: str) -> list[str]:
    if not isinstance(response, str):
        raise InstrumentProtocolError(f"{context} response must be text")
    try:
        rows = list(csv.reader(response.splitlines(), strict=True))
    except csv.Error as exc:
        raise InstrumentProtocolError(f"malformed {context} response") from exc
    if len(rows) != 1 or len(rows[0]) != expected_fields:
        raise InstrumentProtocolError(f"unexpected {context} response field count")
    if any(not field.strip() for field in rows[0]):
        raise InstrumentProtocolError(f"{context} response contains an empty required field")
    return rows[0]


def _parse_integer(value: object, context: str) -> int:
    if isinstance(value, bool):
        raise InstrumentProtocolError(f"{context} must be an integer")
    try:
        return int(value)
    except (TypeError, ValueError) as exc:
        raise InstrumentProtocolError(f"{context} must be an integer") from exc


def _parse_finite_float(value: object, context: str) -> float:
    if isinstance(value, bool):
        raise InstrumentProtocolError(f"{context} must be a finite number")
    try:
        parsed = float(value)
    except (TypeError, ValueError) as exc:
        raise InstrumentProtocolError(f"{context} must be a finite number") from exc
    if not math.isfinite(parsed):
        raise InstrumentProtocolError(f"{context} must be a finite number")
    return parsed


@dataclass(frozen=True)
class InstrumentInfo:
    manufacturer: str
    model: str
    serial_number: str
    firmware: str
    raw: str

    @classmethod
    def parse(cls, response: str) -> InstrumentInfo:
        manufacturer, model, serial_number, firmware = _parse_csv_response(
            response, 4, "identity"
        )
        return cls(manufacturer, model, serial_number, firmware, response)


@dataclass(frozen=True)
class SensorCapabilities:
    power: bool
    energy: bool
    response_settable: bool
    wavelength_settable: bool
    tau_settable: bool
    temperature_sensor: bool


@dataclass(frozen=True)
class SensorInfo:
    name: str
    serial_number: str
    calibration_message: str
    sensor_type: int
    subtype: int
    raw_flags: int
    capabilities: SensorCapabilities
    raw: str

    _POWER: ClassVar[int] = 1
    _ENERGY: ClassVar[int] = 2
    _RESPONSE_SETTABLE: ClassVar[int] = 16
    _WAVELENGTH_SETTABLE: ClassVar[int] = 32
    _TAU_SETTABLE: ClassVar[int] = 64
    _TEMPERATURE_SENSOR: ClassVar[int] = 256

    @classmethod
    def parse(cls, response: str) -> SensorInfo:
        (
            name,
            serial_number,
            calibration_message,
            sensor_type_text,
            subtype_text,
            flags_text,
        ) = _parse_csv_response(response, 6, "sensor")
        sensor_type = _parse_integer(sensor_type_text, "sensor type")
        subtype = _parse_integer(subtype_text, "sensor subtype")
        raw_flags = _parse_integer(flags_text, "sensor flags")
        if raw_flags < 0:
            raise InstrumentProtocolError(
                "sensor capability flags must be a nonnegative integer"
            )
        capabilities = SensorCapabilities(
            power=bool(raw_flags & cls._POWER),
            energy=bool(raw_flags & cls._ENERGY),
            response_settable=bool(raw_flags & cls._RESPONSE_SETTABLE),
            wavelength_settable=bool(raw_flags & cls._WAVELENGTH_SETTABLE),
            tau_settable=bool(raw_flags & cls._TAU_SETTABLE),
            temperature_sensor=bool(raw_flags & cls._TEMPERATURE_SENSOR),
        )
        return cls(
            name,
            serial_number,
            calibration_message,
            sensor_type,
            subtype,
            raw_flags,
            capabilities,
            response,
        )


@dataclass(frozen=True)
class Measurement:
    kind: MeasurementKind
    value: float
    unit: str
    measured_at: float
    raw: str

    @classmethod
    def parse(
        cls, kind: MeasurementKind, response: object, measured_at: object
    ) -> Measurement:
        if not isinstance(kind, MeasurementKind):
            raise InstrumentProtocolError("measurement kind is invalid")
        value = _parse_finite_float(response, "measurement value")
        if abs(value) >= 9.9e37:
            raise InstrumentProtocolError("measurement value is an invalid-device sentinel")
        timestamp = _parse_finite_float(measured_at, "measurement timestamp")
        return cls(kind, value, kind.default_unit, timestamp, str(response))


@dataclass(frozen=True)
class SystemError:
    code: int
    message: str
    raw: str

    @classmethod
    def parse(cls, response: str) -> SystemError:
        code_text, message = _parse_csv_response(response, 2, "system error")
        return cls(_parse_integer(code_text, "system error code"), message, response)


def _require_register(value: object, *, minimum: int, maximum: int, context: str) -> int:
    if isinstance(value, bool) or not isinstance(value, Integral):
        raise InstrumentSafetyError(f"{context} must be an integer")
    parsed = int(value)
    if not minimum <= parsed <= maximum:
        raise InstrumentSafetyError(f"{context} must be within {minimum}..{maximum}")
    return parsed


def _require_finite_value(value: object, context: str) -> float:
    if isinstance(value, bool) or not isinstance(value, Real):
        raise InstrumentSafetyError(f"{context} must be a finite number")
    parsed = float(value)
    if not math.isfinite(parsed):
        raise InstrumentSafetyError(f"{context} must be a finite number")
    return parsed


def _format_scpi_number(value: float) -> str:
    return repr(value)


def _parse_register_response(response: str, *, minimum: int, maximum: int, context: str) -> int:
    if re.fullmatch(r"[+-]?\d+", response) is None:
        raise InstrumentProtocolError(f"{context} response must be an integer")
    parsed = int(response)
    if not minimum <= parsed <= maximum:
        raise InstrumentProtocolError(
            f"{context} response {parsed} is outside {minimum}..{maximum}"
        )
    return parsed


def _parse_integer_response(response: str, context: str) -> int:
    if re.fullmatch(r"[+-]?\d+", response) is None:
        raise InstrumentProtocolError(f"{context} response must be an integer")
    return int(response)


def _parse_date_response(response: str) -> datetime.date:
    fields = _parse_csv_response(response, 3, "system date")
    if any(re.fullmatch(r"\d+", field) is None for field in fields):
        raise InstrumentProtocolError("system date response must be year,month,day integers")
    try:
        return datetime.date(*(int(field) for field in fields))
    except ValueError as exc:
        raise InstrumentProtocolError("system date response is not a valid calendar date") from exc


def _parse_time_response(response: str) -> datetime.time:
    hour_text, minute_text, second_text = _parse_csv_response(response, 3, "system time")
    if re.fullmatch(r"\d+", hour_text) is None or re.fullmatch(r"\d+", minute_text) is None:
        raise InstrumentProtocolError("system time response must contain integer hour and minute")
    match = re.fullmatch(r"(\d+)(?:\.(\d{1,6}))?", second_text)
    if match is None:
        raise InstrumentProtocolError("system time response has an invalid second value")
    second = int(match.group(1))
    microsecond = int((match.group(2) or "").ljust(6, "0") or "0")
    try:
        return datetime.time(int(hour_text), int(minute_text), second, microsecond)
    except ValueError as exc:
        raise InstrumentProtocolError("system time response is not a valid clock time") from exc


class PM400System:
    """Typed access to the PM400 SCPI ``SYSTem`` subsystem."""

    def __init__(self, pm400: PM400) -> None:
        self._pm400 = pm400

    def beep(self) -> None:
        self._pm400._write_action("SYSTem:BEEPer")

    def set_beeper_enabled(self, enabled: bool) -> bool:
        if not isinstance(enabled, bool):
            raise InstrumentSafetyError("beeper enabled state must be a boolean")
        return bool(
            self._pm400._set_and_confirm(
                f"SYSTem:BEEPer:STATe {int(enabled)}",
                "SYSTem:BEEPer:STATe?",
                enabled,
                lambda response: self._pm400._parse_bool_response(response),
            )
        )

    def get_beeper_enabled(self) -> bool:
        return self._pm400._query_bool("SYSTem:BEEPer:STATe?")

    def next_error(self) -> SystemError:
        return SystemError.parse(self._pm400._query_text("SYSTem:ERRor?"))

    def drain_errors(self, max_errors: int = 32) -> tuple[SystemError, ...]:
        if isinstance(max_errors, bool) or not isinstance(max_errors, Integral) or max_errors < 1:
            raise InstrumentSafetyError("maximum error count must be a positive integer")
        maximum = int(max_errors)
        errors: list[SystemError] = []
        with self._pm400._request_lock:
            for _ in range(maximum):
                error = self.next_error()
                if error.code == 0:
                    return tuple(errors)
                errors.append(error)
        raise InstrumentProtocolError("PM400 system error queue did not terminate")

    def get_scpi_version(self) -> str:
        return self._pm400._query_text("SYSTem:VERSion?")

    def set_date(self, value: datetime.date) -> datetime.date:
        if not isinstance(value, datetime.date) or isinstance(value, datetime.datetime):
            raise InstrumentSafetyError("system date must be a datetime.date")
        return self._pm400._set_and_confirm(
            f"SYSTem:DATE {value.year},{value.month},{value.day}",
            "SYSTem:DATE?",
            value,
            _parse_date_response,
        )

    def get_date(self) -> datetime.date:
        return _parse_date_response(self._pm400._query_text("SYSTem:DATE?"))

    def set_time(self, value: datetime.time) -> datetime.time:
        if not isinstance(value, datetime.time) or isinstance(value, datetime.datetime):
            raise InstrumentSafetyError("system time must be a datetime.time")
        if value.tzinfo is not None:
            raise InstrumentSafetyError("system time must not include a timezone")
        second = str(value.second)
        if value.microsecond:
            second = f"{second}.{value.microsecond:06d}".rstrip("0")
        return self._pm400._set_and_confirm(
            f"SYSTem:TIME {value.hour},{value.minute},{second}",
            "SYSTem:TIME?",
            value,
            _parse_time_response,
        )

    def get_time(self) -> datetime.time:
        return _parse_time_response(self._pm400._query_text("SYSTem:TIME?"))

    def set_line_frequency_hz(self, value: int) -> int:
        frequency = _require_register(
            value, minimum=50, maximum=60, context="line frequency"
        )
        if frequency not in {50, 60}:
            raise InstrumentSafetyError("line frequency must be either 50 or 60 Hz")
        return self._pm400._set_and_confirm(
            f"SYSTem:LFRequency {frequency}",
            "SYSTem:LFRequency?",
            frequency,
            self._parse_line_frequency,
        )

    @staticmethod
    def _parse_line_frequency(response: str) -> int:
        value = _parse_register_response(
            response, minimum=50, maximum=60, context="line frequency"
        )
        if value not in {50, 60}:
            raise InstrumentProtocolError("line frequency response must be either 50 or 60 Hz")
        return value

    def get_line_frequency_hz(self) -> int:
        return self._parse_line_frequency(self._pm400._query_text("SYSTem:LFRequency?"))

    def get_sensor_info(self) -> SensorInfo:
        return SensorInfo.parse(self._pm400._query_text("SYSTem:SENSor:IDN?"))


class PM400Status:
    """Typed access to the PM400 16-bit status-register groups."""

    _REGISTER_MINIMUM = 0
    _REGISTER_MAXIMUM = 65535

    def __init__(self, pm400: PM400) -> None:
        self._pm400 = pm400

    @staticmethod
    def _prefix(group: StatusGroup) -> str:
        if not isinstance(group, StatusGroup):
            raise InstrumentSafetyError("status group must be a StatusGroup")
        return f"STATus:{group.value}"

    @classmethod
    def _validate_value(cls, value: int) -> int:
        return _require_register(
            value,
            minimum=cls._REGISTER_MINIMUM,
            maximum=cls._REGISTER_MAXIMUM,
            context="status register value",
        )

    @classmethod
    def _parse_value(cls, response: str) -> int:
        return _parse_register_response(
            response,
            minimum=cls._REGISTER_MINIMUM,
            maximum=cls._REGISTER_MAXIMUM,
            context="status register",
        )

    def read_event(self, group: StatusGroup) -> int:
        return self._parse_value(self._pm400._query_text(f"{self._prefix(group)}:EVENt?"))

    def read_condition(self, group: StatusGroup) -> int:
        return self._parse_value(self._pm400._query_text(f"{self._prefix(group)}:CONDition?"))

    def set_positive_transition(self, group: StatusGroup, value: int) -> int:
        prefix = self._prefix(group)
        expected = self._validate_value(value)
        return self._pm400._set_and_confirm(
            f"{prefix}:PTRansition {expected}",
            f"{prefix}:PTRansition?",
            expected,
            self._parse_value,
        )

    def get_positive_transition(self, group: StatusGroup) -> int:
        return self._parse_value(self._pm400._query_text(f"{self._prefix(group)}:PTRansition?"))

    def set_negative_transition(self, group: StatusGroup, value: int) -> int:
        prefix = self._prefix(group)
        expected = self._validate_value(value)
        return self._pm400._set_and_confirm(
            f"{prefix}:NTRansition {expected}",
            f"{prefix}:NTRansition?",
            expected,
            self._parse_value,
        )

    def get_negative_transition(self, group: StatusGroup) -> int:
        return self._parse_value(self._pm400._query_text(f"{self._prefix(group)}:NTRansition?"))

    def set_enable(self, group: StatusGroup, value: int) -> int:
        prefix = self._prefix(group)
        expected = self._validate_value(value)
        return self._pm400._set_and_confirm(
            f"{prefix}:ENABle {expected}",
            f"{prefix}:ENABle?",
            expected,
            self._parse_value,
        )

    def get_enable(self, group: StatusGroup) -> int:
        return self._parse_value(self._pm400._query_text(f"{self._prefix(group)}:ENABle?"))

    def preset(self, *, confirm: bool = False) -> None:
        self._pm400._require_confirm(confirm, "status preset")
        self._pm400._write_action("STATus:PRESet")


class PM400Display:
    """Typed access to finite-valued display settings without invented bounds."""

    def __init__(self, pm400: PM400) -> None:
        self._pm400 = pm400

    def _set(self, header: str, value: float) -> float:
        expected = _require_finite_value(value, header)
        return self._pm400._set_and_confirm(
            f"{header} {_format_scpi_number(expected)}",
            f"{header}?",
            expected,
            lambda response: _parse_finite_float(response, f"{header} response"),
            lambda observed, requested: math.isclose(
                float(observed), float(requested), rel_tol=1e-12, abs_tol=1e-12
            ),
        )

    def _get(self, header: str) -> float:
        return self._pm400._query_float(f"{header}?")

    def set_brightness(self, value: float) -> float:
        return self._set("DISPlay:BRIGhtness", value)

    def get_brightness(self) -> float:
        return self._get("DISPlay:BRIGhtness")

    def set_contrast(self, value: float) -> float:
        return self._set("DISPlay:CONTrast", value)

    def get_contrast(self) -> float:
        return self._get("DISPlay:CONTrast")


class PM400Calibration:
    """Read-only calibration-string query facade."""

    def __init__(self, pm400: PM400) -> None:
        self._pm400 = pm400

    def get_string(self) -> str:
        return self._pm400._query_text("CALibration:STRing?")


class _PM400PropertyFacade:
    """Shared strict property mechanics for the SENSe and INPut facades."""

    _FLOAT_REL_TOLERANCE = 1e-9
    _FLOAT_ABS_TOLERANCE = 1e-12

    def __init__(self, pm400: PM400) -> None:
        self._pm400 = pm400

    @classmethod
    def _float_equivalent(cls, observed: object, requested: object) -> bool:
        return math.isclose(
            float(observed),
            float(requested),
            rel_tol=cls._FLOAT_REL_TOLERANCE,
            abs_tol=cls._FLOAT_ABS_TOLERANCE,
        )

    @staticmethod
    def _validate_selector(
        selector: LimitSelector,
        allowed_selectors: tuple[LimitSelector, ...],
        context: str,
    ) -> LimitSelector:
        if not isinstance(selector, LimitSelector):
            raise InstrumentSafetyError(f"{context} limit selector must be a LimitSelector")
        if selector not in allowed_selectors:
            raise InstrumentSafetyError(
                f"{context} does not support the {selector.name} limit selector"
            )
        return selector

    def _set_numeric(
        self,
        header: str,
        value: float | LimitSelector,
        allowed_selectors: tuple[LimitSelector, ...],
        context: str,
    ) -> float:
        if isinstance(value, LimitSelector):
            selector = self._validate_selector(value, allowed_selectors, context)
            with self._pm400._request_lock:
                expected = self._pm400._query_limit(header, selector)
                return float(
                    self._pm400._set_and_confirm(
                        f"{header} {selector.value}",
                        f"{header}?",
                        expected,
                        lambda response: _parse_finite_float(response, f"{context} response"),
                        self._float_equivalent,
                    )
                )

        expected = _require_finite_value(value, context)
        with self._pm400._request_lock:
            minimum = self._pm400._query_limit(header, LimitSelector.MINIMUM)
            maximum = self._pm400._query_limit(header, LimitSelector.MAXIMUM)
            if minimum > maximum:
                raise InstrumentProtocolError(
                    f"{context} device limits are inverted: minimum={minimum}, maximum={maximum}"
                )
            if not minimum <= expected <= maximum:
                raise InstrumentSafetyError(
                    f"{context} must be within the connected sensor limits "
                    f"{minimum}..{maximum}"
                )
            return float(
                self._pm400._set_and_confirm(
                    f"{header} {_format_scpi_number(expected)}",
                    f"{header}?",
                    expected,
                    lambda response: _parse_finite_float(response, f"{context} response"),
                    self._float_equivalent,
                )
            )

    def _get_numeric(
        self,
        header: str,
        selector: LimitSelector | None,
        allowed_selectors: tuple[LimitSelector, ...],
        context: str,
    ) -> float:
        if selector is None:
            return self._pm400._query_float(f"{header}?")
        selected = self._validate_selector(selector, allowed_selectors, context)
        return self._pm400._query_limit(header, selected)

    def _set_bool(self, header: str, enabled: bool, context: str) -> bool:
        if not isinstance(enabled, bool):
            raise InstrumentSafetyError(f"{context} must be a boolean")
        return bool(
            self._pm400._set_and_confirm(
                f"{header} {int(enabled)}",
                f"{header}?",
                enabled,
                self._pm400._parse_bool_response,
            )
        )

    def _get_bool(self, header: str) -> bool:
        return self._pm400._query_bool(f"{header}?")

    def _require_power(self, action: str) -> None:
        self._pm400._require_capability("power", action)

    def _require_energy(self, action: str) -> None:
        self._pm400._require_capability("energy", action)


class PM400Sense(_PM400PropertyFacade):
    """Typed access to every PM400 ``SENSe`` property and zero action."""

    _MIN_MAX = (LimitSelector.MINIMUM, LimitSelector.MAXIMUM)
    _MIN_MAX_DEFAULT = _MIN_MAX + (LimitSelector.DEFAULT,)

    _AVERAGE_COUNT = "SENSe:AVERage:COUNt"
    _LOSS = "SENSe:CORRection:LOSS:INPut:MAGNitude"
    _ZERO = "SENSe:CORRection:COLLect:ZERO"
    _BEAM_DIAMETER = "SENSe:CORRection:BEAMdiameter"
    _WAVELENGTH = "SENSe:CORRection:WAVelength"
    _PHOTODIODE_RESPONSE = "SENSe:CORRection:POWer:PDIOde:RESPonse"
    _THERMOPILE_RESPONSE = "SENSe:CORRection:POWer:THERmopile:RESPonse"
    _PYRO_RESPONSE = "SENSe:CORRection:ENERgy:PYRO:RESPonse"
    _CURRENT_AUTO_RANGE = "SENSe:CURRent:DC:RANGe:AUTO"
    _CURRENT_RANGE = "SENSe:CURRent:DC:RANGe:UPPer"
    _CURRENT_REFERENCE = "SENSe:CURRent:DC:REFerence"
    _CURRENT_DELTA = "SENSe:CURRent:DC:REFerence:STATe"
    _ENERGY_RANGE = "SENSe:ENERgy:RANGe:UPPer"
    _ENERGY_REFERENCE = "SENSe:ENERgy:REFerence"
    _ENERGY_DELTA = "SENSe:ENERgy:REFerence:STATe"
    _FREQUENCY_UPPER = "SENSe:FREQuency:RANGe:UPPer"
    _FREQUENCY_LOWER = "SENSe:FREQuency:RANGe:LOWer"
    _POWER_AUTO_RANGE = "SENSe:POWer:DC:RANGe:AUTO"
    _POWER_RANGE = "SENSe:POWer:DC:RANGe:UPPer"
    _POWER_REFERENCE = "SENSe:POWer:DC:REFerence"
    _POWER_DELTA = "SENSe:POWer:DC:REFerence:STATe"
    _POWER_UNIT = "SENSe:POWer:DC:UNIT"
    _VOLTAGE_AUTO_RANGE = "SENSe:VOLTage:DC:RANGe:AUTO"
    _VOLTAGE_RANGE = "SENSe:VOLTage:DC:RANGe:UPPer"
    _VOLTAGE_REFERENCE = "SENSe:VOLTage:DC:REFerence"
    _VOLTAGE_DELTA = "SENSe:VOLTage:DC:REFerence:STATe"
    _PEAK_THRESHOLD = "SENSe:PEAKdetector:THReshold"

    @staticmethod
    def _parse_average_count(response: str) -> int:
        count = _parse_integer_response(response, "average count")
        if count < 1:
            raise InstrumentProtocolError("average count response must be positive")
        return count

    def set_average_count(self, value: int) -> int:
        if isinstance(value, bool) or not isinstance(value, Integral) or value < 1:
            raise InstrumentSafetyError("average count must be a positive integer")
        expected = int(value)
        return int(
            self._pm400._set_and_confirm(
                f"{self._AVERAGE_COUNT} {expected}",
                f"{self._AVERAGE_COUNT}?",
                expected,
                self._parse_average_count,
            )
        )

    def get_average_count(self) -> int:
        return self._parse_average_count(self._pm400._query_text(f"{self._AVERAGE_COUNT}?"))

    def set_loss_db(self, value: float | LimitSelector) -> float:
        return self._set_numeric(self._LOSS, value, self._MIN_MAX_DEFAULT, "input loss")

    def get_loss_db(self, selector: LimitSelector | None = None) -> float:
        return self._get_numeric(self._LOSS, selector, self._MIN_MAX_DEFAULT, "input loss")

    def start_zero_collection(self, *, confirm: bool = False) -> None:
        self._pm400._require_confirm(confirm, "zero collection")
        self._pm400._write_action(f"{self._ZERO}:INITiate")

    def abort_zero_collection(self) -> None:
        self._pm400._write_action(f"{self._ZERO}:ABORt")

    def get_zero_state(self) -> bool:
        return self._pm400._query_bool(f"{self._ZERO}:STATe?")

    def get_zero_magnitude(self) -> float:
        return self._pm400._query_float(f"{self._ZERO}:MAGNitude?")

    def set_beam_diameter_mm(self, value: float | LimitSelector) -> float:
        return self._set_numeric(
            self._BEAM_DIAMETER, value, self._MIN_MAX_DEFAULT, "beam diameter"
        )

    def get_beam_diameter_mm(self, selector: LimitSelector | None = None) -> float:
        return self._get_numeric(
            self._BEAM_DIAMETER, selector, self._MIN_MAX_DEFAULT, "beam diameter"
        )

    def set_wavelength_nm(self, value: float | LimitSelector) -> float:
        self._pm400._require_capability("wavelength_settable", "set wavelength")
        return self._set_numeric(self._WAVELENGTH, value, self._MIN_MAX, "wavelength")

    def get_wavelength_nm(self, selector: LimitSelector | None = None) -> float:
        return self._get_numeric(self._WAVELENGTH, selector, self._MIN_MAX, "wavelength")

    def _set_response(
        self,
        header: str,
        value: float | LimitSelector,
        context: str,
        capability: str,
        *,
        confirm: bool,
    ) -> float:
        self._pm400._require_confirm(confirm, context)
        self._pm400._require_capability("response_settable", context)
        self._pm400._require_capability(capability, context)
        return self._set_numeric(header, value, self._MIN_MAX_DEFAULT, context)

    def set_photodiode_response_a_per_w(
        self, value: float | LimitSelector, *, confirm: bool = False
    ) -> float:
        return self._set_response(
            self._PHOTODIODE_RESPONSE,
            value,
            "set photodiode response",
            "power",
            confirm=confirm,
        )

    def get_photodiode_response_a_per_w(
        self, selector: LimitSelector | None = None
    ) -> float:
        self._require_power("get photodiode response")
        return self._get_numeric(
            self._PHOTODIODE_RESPONSE,
            selector,
            self._MIN_MAX_DEFAULT,
            "photodiode response",
        )

    def set_thermopile_response_v_per_w(
        self, value: float | LimitSelector, *, confirm: bool = False
    ) -> float:
        return self._set_response(
            self._THERMOPILE_RESPONSE,
            value,
            "set thermopile response",
            "power",
            confirm=confirm,
        )

    def get_thermopile_response_v_per_w(
        self, selector: LimitSelector | None = None
    ) -> float:
        self._require_power("get thermopile response")
        return self._get_numeric(
            self._THERMOPILE_RESPONSE,
            selector,
            self._MIN_MAX_DEFAULT,
            "thermopile response",
        )

    def set_pyro_response_v_per_j(
        self, value: float | LimitSelector, *, confirm: bool = False
    ) -> float:
        return self._set_response(
            self._PYRO_RESPONSE,
            value,
            "set pyro response",
            "energy",
            confirm=confirm,
        )

    def get_pyro_response_v_per_j(self, selector: LimitSelector | None = None) -> float:
        self._require_energy("get pyro response")
        return self._get_numeric(
            self._PYRO_RESPONSE, selector, self._MIN_MAX_DEFAULT, "pyro response"
        )

    def set_current_auto_range(self, enabled: bool) -> bool:
        self._require_power("set current auto range")
        return self._set_bool(self._CURRENT_AUTO_RANGE, enabled, "current auto range")

    def get_current_auto_range(self) -> bool:
        self._require_power("get current auto range")
        return self._get_bool(self._CURRENT_AUTO_RANGE)

    def set_current_range_a(self, value: float | LimitSelector) -> float:
        self._require_power("set current range")
        return self._set_numeric(self._CURRENT_RANGE, value, self._MIN_MAX, "current range")

    def get_current_range_a(self, selector: LimitSelector | None = None) -> float:
        self._require_power("get current range")
        return self._get_numeric(self._CURRENT_RANGE, selector, self._MIN_MAX, "current range")

    def set_current_reference_a(self, value: float | LimitSelector) -> float:
        self._require_power("set current reference")
        return self._set_numeric(
            self._CURRENT_REFERENCE, value, self._MIN_MAX_DEFAULT, "current reference"
        )

    def get_current_reference_a(self, selector: LimitSelector | None = None) -> float:
        self._require_power("get current reference")
        return self._get_numeric(
            self._CURRENT_REFERENCE, selector, self._MIN_MAX_DEFAULT, "current reference"
        )

    def set_current_delta_enabled(self, enabled: bool) -> bool:
        self._require_power("set current delta mode")
        return self._set_bool(self._CURRENT_DELTA, enabled, "current delta state")

    def get_current_delta_enabled(self) -> bool:
        self._require_power("get current delta mode")
        return self._get_bool(self._CURRENT_DELTA)

    def set_energy_range_j(self, value: float | LimitSelector) -> float:
        self._require_energy("set energy range")
        return self._set_numeric(self._ENERGY_RANGE, value, self._MIN_MAX, "energy range")

    def get_energy_range_j(self, selector: LimitSelector | None = None) -> float:
        self._require_energy("get energy range")
        return self._get_numeric(self._ENERGY_RANGE, selector, self._MIN_MAX, "energy range")

    def set_energy_reference_j(self, value: float | LimitSelector) -> float:
        self._require_energy("set energy reference")
        return self._set_numeric(
            self._ENERGY_REFERENCE, value, self._MIN_MAX_DEFAULT, "energy reference"
        )

    def get_energy_reference_j(self, selector: LimitSelector | None = None) -> float:
        self._require_energy("get energy reference")
        return self._get_numeric(
            self._ENERGY_REFERENCE, selector, self._MIN_MAX_DEFAULT, "energy reference"
        )

    def set_energy_delta_enabled(self, enabled: bool) -> bool:
        self._require_energy("set energy delta mode")
        return self._set_bool(self._ENERGY_DELTA, enabled, "energy delta state")

    def get_energy_delta_enabled(self) -> bool:
        self._require_energy("get energy delta mode")
        return self._get_bool(self._ENERGY_DELTA)

    def get_frequency_upper_hz(self) -> float:
        return self._pm400._query_float(f"{self._FREQUENCY_UPPER}?")

    def get_frequency_lower_hz(self) -> float:
        return self._pm400._query_float(f"{self._FREQUENCY_LOWER}?")

    def set_power_auto_range(self, enabled: bool) -> bool:
        self._require_power("set power auto range")
        return self._set_bool(self._POWER_AUTO_RANGE, enabled, "power auto range")

    def get_power_auto_range(self) -> bool:
        self._require_power("get power auto range")
        return self._get_bool(self._POWER_AUTO_RANGE)

    def set_power_range_w(self, value: float | LimitSelector) -> float:
        self._require_power("set power range")
        return self._set_numeric(self._POWER_RANGE, value, self._MIN_MAX, "power range")

    def get_power_range_w(self, selector: LimitSelector | None = None) -> float:
        self._require_power("get power range")
        return self._get_numeric(self._POWER_RANGE, selector, self._MIN_MAX, "power range")

    def set_power_reference_w(self, value: float | LimitSelector) -> float:
        self._require_power("set power reference")
        return self._set_numeric(
            self._POWER_REFERENCE, value, self._MIN_MAX_DEFAULT, "power reference"
        )

    def get_power_reference_w(self, selector: LimitSelector | None = None) -> float:
        self._require_power("get power reference")
        return self._get_numeric(
            self._POWER_REFERENCE, selector, self._MIN_MAX_DEFAULT, "power reference"
        )

    def set_power_delta_enabled(self, enabled: bool) -> bool:
        self._require_power("set power delta mode")
        return self._set_bool(self._POWER_DELTA, enabled, "power delta state")

    def get_power_delta_enabled(self) -> bool:
        self._require_power("get power delta mode")
        return self._get_bool(self._POWER_DELTA)

    @staticmethod
    def _parse_power_unit(response: str) -> PowerUnit:
        for unit in PowerUnit:
            if response == unit.value:
                return unit
        raise InstrumentProtocolError(f"invalid power unit response: {response!r}")

    def set_power_unit(self, unit: PowerUnit) -> PowerUnit:
        self._require_power("set power unit")
        if not isinstance(unit, PowerUnit):
            raise InstrumentSafetyError("power unit must be a PowerUnit")
        return self._pm400._set_and_confirm(
            f"{self._POWER_UNIT} {unit.value}",
            f"{self._POWER_UNIT}?",
            unit,
            self._parse_power_unit,
        )

    def get_power_unit(self) -> PowerUnit:
        self._require_power("get power unit")
        return self._parse_power_unit(self._pm400._query_text(f"{self._POWER_UNIT}?"))

    def set_voltage_auto_range(self, enabled: bool) -> bool:
        self._require_power("set voltage auto range")
        return self._set_bool(self._VOLTAGE_AUTO_RANGE, enabled, "voltage auto range")

    def get_voltage_auto_range(self) -> bool:
        self._require_power("get voltage auto range")
        return self._get_bool(self._VOLTAGE_AUTO_RANGE)

    def set_voltage_range_v(self, value: float | LimitSelector) -> float:
        self._require_power("set voltage range")
        return self._set_numeric(self._VOLTAGE_RANGE, value, self._MIN_MAX, "voltage range")

    def get_voltage_range_v(self, selector: LimitSelector | None = None) -> float:
        self._require_power("get voltage range")
        return self._get_numeric(self._VOLTAGE_RANGE, selector, self._MIN_MAX, "voltage range")

    def set_voltage_reference_v(self, value: float | LimitSelector) -> float:
        self._require_power("set voltage reference")
        return self._set_numeric(
            self._VOLTAGE_REFERENCE, value, self._MIN_MAX_DEFAULT, "voltage reference"
        )

    def get_voltage_reference_v(self, selector: LimitSelector | None = None) -> float:
        self._require_power("get voltage reference")
        return self._get_numeric(
            self._VOLTAGE_REFERENCE, selector, self._MIN_MAX_DEFAULT, "voltage reference"
        )

    def set_voltage_delta_enabled(self, enabled: bool) -> bool:
        self._require_power("set voltage delta mode")
        return self._set_bool(self._VOLTAGE_DELTA, enabled, "voltage delta state")

    def get_voltage_delta_enabled(self) -> bool:
        self._require_power("get voltage delta mode")
        return self._get_bool(self._VOLTAGE_DELTA)

    def set_peak_threshold_percent(self, value: float | LimitSelector) -> float:
        self._require_energy("set peak threshold")
        return self._set_numeric(
            self._PEAK_THRESHOLD, value, self._MIN_MAX_DEFAULT, "peak threshold"
        )

    def get_peak_threshold_percent(self, selector: LimitSelector | None = None) -> float:
        self._require_energy("get peak threshold")
        return self._get_numeric(
            self._PEAK_THRESHOLD, selector, self._MIN_MAX_DEFAULT, "peak threshold"
        )


class PM400Input(_PM400PropertyFacade):
    """Typed access to the PM400 ``INPut`` subsystem."""

    _MIN_MAX_DEFAULT = (
        LimitSelector.MINIMUM,
        LimitSelector.MAXIMUM,
        LimitSelector.DEFAULT,
    )
    _PHOTODIODE_LOWPASS = "INPut:PDIOde:FILTer:LPASs:STATe"
    _THERMOPILE_ACCELERATOR = "INPut:THERmopile:ACCelerator:STATe"
    _THERMOPILE_ACCELERATOR_AUTO = "INPut:THERmopile:ACCelerator:AUTO"
    _THERMOPILE_TAU = "INPut:THERmopile:ACCelerator:TAU"
    _ADAPTER_TYPE = "INPut:ADAPter:TYPE"

    def set_photodiode_lowpass_enabled(self, enabled: bool) -> bool:
        self._require_power("set photodiode low-pass filter")
        return self._set_bool(
            self._PHOTODIODE_LOWPASS, enabled, "photodiode low-pass filter state"
        )

    def get_photodiode_lowpass_enabled(self) -> bool:
        self._require_power("get photodiode low-pass filter")
        return self._get_bool(self._PHOTODIODE_LOWPASS)

    def set_thermopile_accelerator_enabled(self, enabled: bool) -> bool:
        self._require_power("set thermopile accelerator")
        return self._set_bool(
            self._THERMOPILE_ACCELERATOR, enabled, "thermopile accelerator state"
        )

    def get_thermopile_accelerator_enabled(self) -> bool:
        self._require_power("get thermopile accelerator")
        return self._get_bool(self._THERMOPILE_ACCELERATOR)

    def set_thermopile_accelerator_auto(self, enabled: bool) -> bool:
        self._require_power("set thermopile accelerator auto mode")
        return self._set_bool(
            self._THERMOPILE_ACCELERATOR_AUTO,
            enabled,
            "thermopile accelerator auto state",
        )

    def get_thermopile_accelerator_auto(self) -> bool:
        self._require_power("get thermopile accelerator auto mode")
        return self._get_bool(self._THERMOPILE_ACCELERATOR_AUTO)

    def set_thermopile_tau_s(self, value: float | LimitSelector) -> float:
        self._pm400._require_capability("tau_settable", "set thermopile tau")
        self._require_power("set thermopile tau")
        return self._set_numeric(
            self._THERMOPILE_TAU,
            value,
            self._MIN_MAX_DEFAULT,
            "thermopile tau",
        )

    def get_thermopile_tau_s(self, selector: LimitSelector | None = None) -> float:
        self._require_power("get thermopile tau")
        return self._get_numeric(
            self._THERMOPILE_TAU,
            selector,
            self._MIN_MAX_DEFAULT,
            "thermopile tau",
        )

    @staticmethod
    def _parse_adapter_type(response: str) -> AdapterType:
        adapter_types = {
            "PHOT": AdapterType.PHOTODIODE,
            "PHOTODIODE": AdapterType.PHOTODIODE,
            "THER": AdapterType.THERMAL,
            "THERMAL": AdapterType.THERMAL,
            "PYR": AdapterType.PYRO,
            "PYRO": AdapterType.PYRO,
        }
        if response in adapter_types:
            return adapter_types[response]
        raise InstrumentProtocolError(f"invalid adapter type response: {response!r}")

    def set_adapter_type(
        self, value: AdapterType, *, confirm: bool = False
    ) -> AdapterType:
        self._pm400._require_confirm(confirm, "set adapter type")
        if not isinstance(value, AdapterType):
            raise InstrumentSafetyError("adapter type must be an AdapterType")
        return self._pm400._set_and_confirm(
            f"{self._ADAPTER_TYPE} {value.value}",
            f"{self._ADAPTER_TYPE}?",
            value,
            self._parse_adapter_type,
        )

    def get_adapter_type(self) -> AdapterType:
        return self._parse_adapter_type(self._pm400._query_text(f"{self._ADAPTER_TYPE}?"))


class PM400Measurement:
    """Typed PM400 scalar-measurement commands and result parsing."""

    def __init__(self, pm400: PM400) -> None:
        self._pm400 = pm400

    @staticmethod
    def _require_kind(kind: object) -> MeasurementKind:
        if not isinstance(kind, MeasurementKind):
            raise InstrumentSafetyError("measurement kind must be a MeasurementKind")
        return kind

    def _require_supported(self, kind: MeasurementKind) -> None:
        requirements = {
            MeasurementKind.POWER: ("power", "measure optical power"),
            MeasurementKind.POWER_DENSITY: ("power", "measure optical power density"),
            MeasurementKind.ENERGY: ("energy", "measure optical energy"),
            MeasurementKind.ENERGY_DENSITY: ("energy", "measure optical energy density"),
            MeasurementKind.TEMPERATURE: (
                "temperature_sensor",
                "measure temperature",
            ),
        }
        requirement = requirements.get(kind)
        if requirement is not None:
            self._pm400._require_capability(*requirement)

    @staticmethod
    def _parse_configuration(response: str) -> MeasurementKind:
        normalized = response.upper()
        for kind in MeasurementKind:
            full = kind.scpi_suffix.upper()
            short = "".join(
                character
                for character in kind.scpi_suffix
                if character.isupper() or character.isdigit() or character == ":"
            )
            if normalized in {full, short}:
                return kind
        raise InstrumentProtocolError(
            f"invalid configured measurement kind response: {response!r}"
        )

    def _configuration_locked(self) -> MeasurementKind:
        kind = self._parse_configuration(self._pm400._query_text("CONFigure?"))
        self._require_supported(kind)
        return kind

    def _unit_locked(self, kind: MeasurementKind) -> str:
        if kind is MeasurementKind.POWER:
            return self._pm400.sense.get_power_unit().value
        return kind.default_unit

    def _parse_result(self, kind: MeasurementKind, response: str, unit: str) -> Measurement:
        parsed = Measurement.parse(kind, response, self._pm400._clock())
        if parsed.unit == unit:
            return parsed
        return Measurement(
            kind=parsed.kind,
            value=parsed.value,
            unit=unit,
            measured_at=parsed.measured_at,
            raw=parsed.raw,
        )

    def initiate(self) -> None:
        self._pm400._write_action("INITiate:IMMediate")

    def abort(self) -> None:
        self._pm400._write_action("ABORt")

    def configure(self, kind: MeasurementKind) -> MeasurementKind:
        kind = self._require_kind(kind)
        self._require_supported(kind)
        self._pm400._write_action(f"CONFigure:SCALar:{kind.scpi_suffix}")
        return kind

    def get_configuration(self) -> MeasurementKind:
        with self._pm400._request_lock:
            return self._configuration_locked()

    def measure(
        self, kind: MeasurementKind, timeout: float | None = None
    ) -> Measurement:
        kind = self._require_kind(kind)
        self._require_supported(kind)
        command = f"MEASure:SCALar:{kind.scpi_suffix}?"

        def operation(mark_query_started: Callable[[], None]) -> Measurement:
            mark_query_started()
            unit = self._unit_locked(kind)
            response = self._pm400._query_text(command, timeout=timeout)
            return self._parse_result(kind, response, unit)

        return self._pm400._run_active_measurement(operation, timeout=timeout)

    def fetch(self, kind: MeasurementKind | None = None) -> Measurement:
        if kind is not None:
            kind = self._require_kind(kind)
            self._require_supported(kind)
        with self._pm400._request_lock:
            if kind is None:
                kind = self._configuration_locked()
            unit = self._unit_locked(kind)
            response = self._pm400._query_text("FETCh?")
            return self._parse_result(kind, response, unit)

    def read(
        self, kind: MeasurementKind | None = None, timeout: float | None = None
    ) -> Measurement:
        if kind is not None:
            kind = self._require_kind(kind)
            self._require_supported(kind)

        def operation(mark_query_started: Callable[[], None]) -> Measurement:
            current_kind = kind
            if current_kind is None:
                mark_query_started()
                current_kind = self._configuration_locked()
            else:
                mark_query_started()
                self._pm400._write_action(
                    f"CONFigure:SCALar:{current_kind.scpi_suffix}"
                )
            unit = self._unit_locked(current_kind)
            mark_query_started()
            response = self._pm400._query_text("READ?", timeout=timeout)
            return self._parse_result(current_kind, response, unit)

        return self._pm400._run_active_measurement(operation, timeout=timeout)


class _PM400TransportHelper:
    """One retained, bounded wait around a backend call that may block forever."""

    def __init__(
        self,
        sequence: int,
        kind: str,
        subject: object,
        target: Callable[[], object],
    ) -> None:
        self.kind = kind
        self.subject = subject
        self.done = threading.Event()
        self.error: BaseException | None = None
        self.thread = threading.Thread(
            target=self._run,
            args=(target,),
            name=f"PM400Transport-{kind}-{sequence}",
            daemon=True,
        )

    def _run(self, target: Callable[[], object]) -> None:
        try:
            target()
        except BaseException as error:
            self.error = error
        finally:
            self.done.set()


class PM400:
    """PM400 VISA session owner and typed public SCPI API root."""

    _INTERRUPT_RETRIES = 64
    _INTERRUPT_RETRY_SECONDS = 0.01
    _ACTIVE_CLOSE_LOCK_SECONDS = 0.75
    _TRANSPORT_HELPER_SECONDS = 0.05

    def __init__(
        self,
        resource_name: str,
        timeout: float = 5.0,
        resource_manager: object | None = None,
        resource_manager_factory: Callable[[], object] = pyvisa.ResourceManager,
        clock: Callable[[], float] = time.monotonic,
        sleep: Callable[[float], None] = time.sleep,
    ) -> None:
        if not isinstance(resource_name, str) or not resource_name.strip():
            raise ValueError("resource_name must be a non-empty string")
        if isinstance(timeout, bool) or not isinstance(timeout, Real):
            raise ValueError("timeout must be finite and positive")
        if not math.isfinite(timeout) or timeout <= 0:
            raise ValueError("timeout must be finite and positive")
        if not callable(resource_manager_factory):
            raise TypeError("resource_manager_factory must be callable")
        if not callable(clock) or not callable(sleep):
            raise TypeError("clock and sleep must be callable")

        self.resource_name = resource_name
        self.timeout = float(timeout)
        self._canonical_timeout_ms = max(1, math.ceil(self.timeout * 1000))
        self._injected_manager = resource_manager
        self._resource_manager_factory = resource_manager_factory
        self._clock = clock
        self._sleep = sleep
        self._manager: object | None = None
        self._manager_owned = False
        self._visa_lease: VisaManagerLease | None = None
        self._resource_reservation: VisaResourceReservation | None = None
        self._resource: object | None = None
        self._request_lock = threading.RLock()
        self._lifecycle_lock = threading.RLock()
        self._lifecycle_condition = threading.Condition(self._lifecycle_lock)
        self._measurement_cancel_event = threading.Event()
        self._measurement_cancel_generation = 0
        self._measurement_waiters = 0
        self._active_measurement_thread_id: int | None = None
        self._active_measurement_generation: int | None = None
        self._measurement_io_armed = False
        self._measurement_timeout_restore_error: BaseException | None = None
        self._terminal_fault_generation = 0
        self._transport_helpers: list[_PM400TransportHelper] = []
        self._transport_helper_sequence = 0
        self._resource_close_completed: object | None = None
        self._manager_close_completed: object | None = None
        self.state = DriverState.DISCONNECTED
        self.instrument_info: InstrumentInfo | None = None
        self.sensor_info: SensorInfo | None = None
        self.cleanup_error: BaseException | None = None
        self._last_preexisting_errors: tuple[SystemError, ...] = ()
        self.log = get_logger("pm400")
        self.system = PM400System(self)
        self.status = PM400Status(self)
        self.display = PM400Display(self)
        self.calibration = PM400Calibration(self)
        self.sense = PM400Sense(self)
        self.input = PM400Input(self)
        self.measurement = PM400Measurement(self)

    @property
    def last_preexisting_errors(self) -> tuple[SystemError, ...]:
        """Device errors drained before the most recent error-checked action."""
        return self._last_preexisting_errors

    def _release_resource_name(self) -> None:
        if self._resource_reservation is not None:
            self._resource_reservation.release()
            self._resource_reservation = None

    def _configure_resource(self, resource: object) -> None:
        resource.timeout = self._canonical_timeout_ms
        resource.read_termination = "\n"
        resource.write_termination = "\n"

    @staticmethod
    def _validate_command(command: str) -> str:
        if not isinstance(command, str) or not command or command != command.strip():
            raise InstrumentSafetyError("SCPI command must be non-empty text without padding")
        if "\n" in command or "\r" in command:
            raise InstrumentSafetyError("SCPI command must not contain a line terminator")
        return command

    @staticmethod
    def _validate_action_name(action: str) -> str:
        if not isinstance(action, str) or not action.strip():
            raise InstrumentSafetyError("action must be non-empty text")
        return action

    @staticmethod
    def _validate_timeout(timeout: float | None) -> float | None:
        if timeout is None:
            return None
        if isinstance(timeout, bool) or not isinstance(timeout, Real):
            raise InstrumentSafetyError("timeout must be finite and positive")
        if not math.isfinite(timeout) or timeout <= 0:
            raise InstrumentSafetyError("timeout must be finite and positive")
        return float(timeout)

    @staticmethod
    def _is_timeout_error(error: BaseException) -> bool:
        return isinstance(error, TimeoutError) or (
            isinstance(error, VisaIOError) and error.error_code == StatusCode.error_timeout
        )

    @classmethod
    def _translated_transport_error(cls, error: BaseException, action: str) -> BaseException:
        if cls._is_timeout_error(error):
            return InstrumentTimeoutError(f"PM400 timed out while {action}")
        if isinstance(error, (VisaIOError, InvalidSession, OSError)):
            return InstrumentConnectionError(f"PM400 connection failed while {action}")
        return error

    @classmethod
    def _raise_transport_error(cls, error: BaseException, action: str) -> None:
        translated = cls._translated_transport_error(error, action)
        raise translated from error

    def _is_active_measurement_thread_locked(self) -> bool:
        return self._active_measurement_thread_id == threading.get_ident()

    def _check_measurement_cancelled_before_io(self) -> None:
        with self._lifecycle_lock:
            if not self._is_active_measurement_thread_locked():
                return
            if (
                self._active_measurement_generation
                != self._measurement_cancel_generation
                or self._measurement_cancel_event.is_set()
                or self.state != DriverState.ACTIVE
            ):
                raise InstrumentTimeoutError("PM400 measurement was cancelled before I/O")

    def _arm_measurement_io(self) -> bool:
        with self._lifecycle_lock:
            if not self._is_active_measurement_thread_locked():
                return False
            if (
                self._active_measurement_generation
                != self._measurement_cancel_generation
                or self._measurement_cancel_event.is_set()
                or self.state != DriverState.ACTIVE
            ):
                raise InstrumentTimeoutError("PM400 measurement was cancelled before I/O")
            if self._measurement_io_armed:
                raise DeviceFault("PM400 measurement attempted nested VISA I/O")
            self._measurement_io_armed = True
            self._lifecycle_condition.notify_all()
            return True

    def _disarm_measurement_io(self, armed: bool) -> None:
        if not armed:
            return
        with self._lifecycle_lock:
            if self._is_active_measurement_thread_locked():
                self._measurement_io_armed = False
            self._lifecycle_condition.notify_all()

    def _measurement_query_transport(self, resource: object, command: str) -> object:
        armed = self._arm_measurement_io()
        try:
            return resource.query(command)
        finally:
            self._disarm_measurement_io(armed)

    def _measurement_read_transport(self, resource: object) -> object:
        armed = self._arm_measurement_io()
        try:
            return resource.read()
        finally:
            self._disarm_measurement_io(armed)

    def _measurement_write_transport(self, resource: object, command: str) -> object:
        armed = self._arm_measurement_io()
        try:
            return resource.write(command)
        finally:
            self._disarm_measurement_io(armed)

    def _resource_for_transaction_locked(self) -> object:
        self._check_measurement_cancelled_before_io()
        if self._resource is None:
            raise InstrumentConnectionError("PM400 VISA resource is not open")
        if self.state not in {DriverState.CONNECTING, DriverState.READY, DriverState.ACTIVE}:
            raise InstrumentSafetyError(
                f"Cannot perform a PM400 transaction while state is {self.state.name}"
            )
        return self._resource

    @staticmethod
    def _parse_single_response(response: object, command: str) -> str:
        if not isinstance(response, str):
            raise InstrumentProtocolError(f"{command} response must be text")
        lines = response.splitlines()
        if len(lines) != 1 or not lines[0].strip():
            raise InstrumentProtocolError(f"{command} response must contain exactly one value")
        return lines[0]

    def _reject_pending_response_data_locked(
        self,
        resource: object,
        command: str,
        *,
        guard_measurement_io: bool = True,
    ) -> None:
        """Reject a second termination-bounded response without clearing I/O buffers.

        PyVISA's ``query()`` consumes one ``read()`` only.  A one-millisecond
        follow-up read is therefore a bounded, backend-neutral probe: its
        timeout proves the buffer is clean, while any successful read proves a
        second response had been queued.  The successful probe consumes only
        that evidence and the driver faults rather than letting it contaminate
        a later transaction.
        """
        prior_timeout: object | None = None
        timeout_saved = False
        clean_timeout = False
        pending_response: object | None = None
        primary_error: BaseException | None = None
        try:
            prior_timeout = resource.timeout
            timeout_saved = True
            resource.timeout = 1
            try:
                if guard_measurement_io:
                    pending_response = self._measurement_read_transport(resource)
                else:
                    pending_response = resource.read()
            except BaseException as error:
                if self._is_timeout_error(error):
                    clean_timeout = True
                else:
                    primary_error = self._translated_transport_error(
                        error, f"checking pending data after {command!r}"
                    )
        except BaseException as error:
            primary_error = self._translated_transport_error(
                error, f"checking pending data after {command!r}"
            )
        finally:
            if timeout_saved:
                try:
                    resource.timeout = prior_timeout
                except BaseException as error:
                    cleanup_error = self._translated_transport_error(
                        error, "restoring the VISA timeout after a pending-data check"
                    )
                    if self.cleanup_error is None:
                        self.cleanup_error = cleanup_error
                    with self._lifecycle_lock:
                        if self._is_active_measurement_thread_locked():
                            self._measurement_timeout_restore_error = cleanup_error
                    if primary_error is None:
                        primary_error = cleanup_error
                    else:
                        self.log.error(
                            "PM400 pending-data timeout restoration failed: %s",
                            type(cleanup_error).__name__,
                        )
        if primary_error is not None:
            raise primary_error
        if clean_timeout:
            return
        self.log.error(
            "PM400 rejected trailing response data after %s: %r", command, pending_response
        )
        with self._lifecycle_lock:
            if self.state in {DriverState.CONNECTING, DriverState.READY, DriverState.ACTIVE}:
                self.state = DriverState.FAULT
                self._lifecycle_condition.notify_all()
        raise DeviceFault(f"PM400 has trailing unconsumed response data after {command!r}")

    def _query_text(self, command: str, *, timeout: float | None = None) -> str:
        """Run one bounded SCPI query and return its sole, non-empty response line."""
        command = self._validate_command(command)
        requested_timeout = self._validate_timeout(timeout)
        with self._request_lock:
            resource = self._resource_for_transaction_locked()
            prior_timeout = None
            timeout_changed = requested_timeout is not None
            primary_error: BaseException | None = None
            primary_cause: BaseException | None = None
            try:
                if timeout_changed:
                    prior_timeout = resource.timeout
                    resource.timeout = max(1, math.ceil(requested_timeout * 1000))
                response = self._measurement_query_transport(resource, command)
            except BaseException as error:
                primary_error = self._translated_transport_error(
                    error, f"querying {command!r}"
                )
                primary_cause = error
            finally:
                if timeout_changed:
                    try:
                        resource.timeout = prior_timeout
                    except BaseException as error:
                        cleanup_error = self._translated_transport_error(
                            error, "restoring the VISA timeout"
                        )
                        self.cleanup_error = cleanup_error
                        with self._lifecycle_lock:
                            if self._is_active_measurement_thread_locked():
                                self._measurement_timeout_restore_error = cleanup_error
                        if primary_error is None:
                            primary_error = cleanup_error
                            primary_cause = error
                        else:
                            self.log.error(
                                "PM400 VISA timeout restoration failed after %s: %s",
                                command,
                                type(cleanup_error).__name__,
                            )
            if primary_error is not None:
                raise primary_error from primary_cause
            try:
                parsed = self._parse_single_response(response, command)
            except InstrumentProtocolError as primary_error:
                try:
                    self._reject_pending_response_data_locked(resource, command)
                except BaseException as pending_error:
                    self.log.error(
                        "PM400 trailing-data check failed after malformed %s response: %s",
                        command,
                        type(pending_error).__name__,
                    )
                raise primary_error
            self._reject_pending_response_data_locked(resource, command)
            return parsed

    def _query_int(self, command: str, *, minimum: int, maximum: int) -> int:
        self._validate_command(command)
        if (
            isinstance(minimum, bool)
            or isinstance(maximum, bool)
            or not isinstance(minimum, Integral)
            or not isinstance(maximum, Integral)
            or minimum > maximum
        ):
            raise InstrumentSafetyError("integer query bounds must be ordered integers")
        text = self._query_text(command)
        if re.fullmatch(r"[+-]?\d+", text) is None:
            raise InstrumentProtocolError(f"{command} response must be an integer")
        value = int(text)
        if not minimum <= value <= maximum:
            raise InstrumentProtocolError(
                f"{command} response {value} is outside {minimum}..{maximum}"
            )
        return value

    def _query_float(self, command: str) -> float:
        text = self._query_text(command)
        return _parse_finite_float(text, f"{command} response")

    def _query_bool(self, command: str) -> bool:
        text = self._query_text(command)
        return self._parse_bool_response(text)

    @staticmethod
    def _parse_bool_response(text: str) -> bool:
        if text == "0":
            return False
        if text == "1":
            return True
        raise InstrumentProtocolError("boolean response must be 0 or 1")

    def _write_locked(self, command: str) -> None:
        resource = self._resource_for_transaction_locked()
        try:
            self._measurement_write_transport(resource, command)
        except BaseException as error:
            self._raise_transport_error(error, f"writing {command!r}")

    def _drain_error_queue_locked(self) -> tuple[SystemError, ...]:
        errors: list[SystemError] = []
        for _ in range(128):
            error = SystemError.parse(self._query_text("SYSTem:ERRor?"))
            if error.code == 0:
                return tuple(errors)
            errors.append(error)
        raise InstrumentProtocolError("PM400 system error queue did not terminate")

    def _write_action(self, command: str, *, check_errors: bool = True) -> None:
        """Write one action, attributing only post-write device errors to it."""
        command = self._validate_command(command)
        if not isinstance(check_errors, bool):
            raise InstrumentSafetyError("check_errors must be a boolean")
        with self._request_lock:
            if check_errors:
                preexisting = self._drain_error_queue_locked()
                self._last_preexisting_errors = preexisting
                if preexisting:
                    self.log.warning("PM400 pre-existing device errors: %r", preexisting)
            self._write_locked(command)
            if check_errors:
                new_errors = self._drain_error_queue_locked()
                if new_errors:
                    self.log.error("PM400 device errors after %s: %r", command, new_errors)
                    raise DeviceFault(
                        f"PM400 reported errors after {command!r}: {new_errors!r}"
                    )

    def _set_and_confirm(
        self,
        set_command: str,
        query_command: str,
        expected: object,
        parser: Callable[[str], object],
        equivalent: Callable[[object, object], bool] = operator.eq,
    ) -> object:
        """Atomically write and read back a property.

        Callers with finite-valued properties supply a scale-aware ``equivalent``
        function (for example, ``math.isclose`` with an explicit tolerance).
        """
        set_command = self._validate_command(set_command)
        query_command = self._validate_command(query_command)
        set_header, separator, set_value = set_command.partition(" ")
        if not separator or not set_value or set_header.endswith("?"):
            raise InstrumentSafetyError("setter command must contain a property header and value")
        if (
            not query_command.endswith("?")
            or " " in query_command
            or query_command[:-1].upper() != set_header.upper()
        ):
            raise InstrumentSafetyError(
                "setter confirmation query must address the same SCPI property"
            )
        if not callable(parser) or not callable(equivalent):
            raise InstrumentSafetyError("setter parser and equivalence check must be callable")
        with self._request_lock:
            self._write_action(set_command, check_errors=False)
            response = self._query_text(query_command)
            try:
                observed = parser(response)
            except InstrumentProtocolError:
                raise
            except Exception as error:
                raise InstrumentProtocolError(
                    f"{query_command} response could not be parsed for confirmation"
                ) from error
            try:
                matches = equivalent(observed, expected)
            except Exception as error:
                raise InstrumentProtocolError("setter confirmation comparison failed") from error
            if not matches:
                raise DeviceFault(
                    f"PM400 setter confirmation mismatch: requested={expected!r}, "
                    f"readback={observed!r}"
                )
            return observed

    def _query_limit(self, query_prefix: str, selector: LimitSelector) -> float:
        query_prefix = self._validate_command(query_prefix)
        if query_prefix.endswith("?"):
            raise InstrumentSafetyError("limit query prefix must not already contain '?' ")
        if not isinstance(selector, LimitSelector):
            raise InstrumentSafetyError("limit selector must be a LimitSelector")
        return self._query_float(f"{query_prefix}? {selector.value}")

    def _require_capability(self, attribute: str, action: str) -> None:
        self._validate_action_name(action)
        if not isinstance(attribute, str) or not attribute.strip():
            raise InstrumentSafetyError("capability attribute must be non-empty text")
        if self.sensor_info is None:
            raise InstrumentConnectionError("PM400 sensor capabilities are not available")
        if not hasattr(self.sensor_info.capabilities, attribute):
            raise InstrumentSafetyError(f"Unknown PM400 sensor capability: {attribute}")
        if not getattr(self.sensor_info.capabilities, attribute):
            raise InstrumentCapabilityError(
                f"PM400 sensor does not support {action}: capability {attribute} is unavailable"
            )

    def _require_confirm(self, confirm: bool, action: str) -> None:
        self._validate_action_name(action)
        if confirm is not True:
            raise InstrumentSafetyError(f"{action} requires confirm=True")

    def _query(self, command: str) -> str:
        return self._query_text(command)

    def identify(self) -> InstrumentInfo:
        """Query the IEEE-488.2 identity without changing instrument state."""
        return InstrumentInfo.parse(self._query_text("*IDN?"))

    def clear_status(self) -> None:
        self._write_action("*CLS")

    @staticmethod
    def _parse_root_register(response: str) -> int:
        return _parse_register_response(
            response, minimum=0, maximum=255, context="IEEE-488.2 register"
        )

    @staticmethod
    def _validate_root_register(value: int, context: str) -> int:
        return _require_register(value, minimum=0, maximum=255, context=context)

    def set_standard_event_enable(self, value: int) -> int:
        expected = self._validate_root_register(value, "standard event enable")
        return self._set_and_confirm(
            f"*ESE {expected}", "*ESE?", expected, self._parse_root_register
        )

    def get_standard_event_enable(self) -> int:
        return self._query_int("*ESE?", minimum=0, maximum=255)

    def read_standard_event_status(self) -> int:
        return self._query_int("*ESR?", minimum=0, maximum=255)

    def mark_operation_complete(self) -> None:
        self._write_action("*OPC")

    def wait_operation_complete(self, timeout: float | None = None) -> None:
        if not self._parse_bool_response(self._query_text("*OPC?", timeout=timeout)):
            raise InstrumentProtocolError("*OPC? response must report operation complete")

    def reset(self, *, confirm: bool = False) -> None:
        self._require_confirm(confirm, "reset")
        self._write_action("*RST")

    def set_service_request_enable(self, value: int) -> int:
        expected = self._validate_root_register(value, "service request enable")
        return self._set_and_confirm(
            f"*SRE {expected}", "*SRE?", expected, self._parse_root_register
        )

    def get_service_request_enable(self) -> int:
        return self._query_int("*SRE?", minimum=0, maximum=255)

    def read_status_byte(self) -> int:
        return self._query_int("*STB?", minimum=0, maximum=255)

    def self_test(self) -> int:
        return _parse_integer_response(self._query_text("*TST?"), "*TST?")

    def wait_to_continue(self) -> None:
        self._write_action("*WAI")

    def _measurement_was_cancelled(self, generation: int) -> bool:
        with self._lifecycle_lock:
            return (
                generation != self._measurement_cancel_generation
                or self._measurement_cancel_event.is_set()
                or self.state == DriverState.CLOSING
            )

    def _publish_terminal_fault(self, evidence: BaseException) -> None:
        with self._lifecycle_lock:
            if self.cleanup_error is None:
                self.cleanup_error = evidence
            self._terminal_fault_generation += 1
            self.state = DriverState.FAULT
            self._measurement_cancel_event.set()
            self._lifecycle_condition.notify_all()

    def _finalize_helper_ownership_locked(self) -> None:
        """Release ownership only after every helper using that object is done."""
        resource = self._resource_close_completed
        if resource is not None and not any(
            helper.subject is resource for helper in self._transport_helpers
        ):
            if self._resource is resource:
                self._resource = None
                self._release_resource_name()
            self._resource_close_completed = None

        manager = self._manager_close_completed
        if manager is not None and not any(
            helper.subject is manager for helper in self._transport_helpers
        ):
            if self._manager is manager:
                self._manager = None
                self._manager_owned = False
                self._visa_lease = None
            self._manager_close_completed = None

    def _remove_completed_helper_locked(self, helper: _PM400TransportHelper) -> None:
        self._transport_helpers.remove(helper)
        if helper.error is None:
            if helper.kind == "close":
                self._resource_close_completed = helper.subject
            elif helper.kind == "manager-close":
                self._manager_close_completed = helper.subject
        self._finalize_helper_ownership_locked()

    def _reap_transport_helpers(self) -> tuple[_PM400TransportHelper, ...]:
        with self._lifecycle_lock:
            helpers = tuple(self._transport_helpers)
        completed = [helper for helper in helpers if helper.done.is_set()]
        for helper in completed:
            helper.thread.join(0)

        late_errors: list[tuple[str, BaseException]] = []
        with self._lifecycle_lock:
            for helper in completed:
                if helper not in self._transport_helpers:
                    continue
                self._remove_completed_helper_locked(helper)
                if helper.error is not None:
                    late_errors.append((helper.kind, helper.error))
            unresolved = tuple(self._transport_helpers)
            self._lifecycle_condition.notify_all()
        for kind, error in late_errors:
            translated = self._translated_transport_error(
                error, f"completing a bounded VISA {kind} helper"
            )
            self._publish_terminal_fault(translated)
        return unresolved

    def _consume_transport_helper(self, helper: _PM400TransportHelper) -> None:
        helper.thread.join(0)
        with self._lifecycle_lock:
            if helper not in self._transport_helpers:
                return
            self._remove_completed_helper_locked(helper)
            self._lifecycle_condition.notify_all()

    def _wait_and_reap_transport_helpers(self) -> tuple[_PM400TransportHelper, ...]:
        with self._lifecycle_lock:
            helpers = tuple(self._transport_helpers)
        deadline = time.monotonic() + self._TRANSPORT_HELPER_SECONDS
        for helper in helpers:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                break
            helper.done.wait(remaining)
        return self._reap_transport_helpers()

    def _bounded_transport_call(
        self,
        kind: str,
        subject: object,
        target: Callable[[], object],
    ) -> tuple[bool, BaseException | None]:
        self._reap_transport_helpers()
        with self._lifecycle_lock:
            if kind == "close" and (
                self._resource is not subject
                or self._resource_close_completed is subject
            ):
                return True, None
            if kind == "clear" and (
                self._resource is not subject
                or self._resource_close_completed is subject
            ):
                return True, None
            if kind == "manager-close" and (
                self._manager is not subject
                or self._visa_lease is None
                or self._manager_close_completed is subject
            ):
                return True, None
            helper = next(
                (
                    existing
                    for existing in self._transport_helpers
                    if existing.kind == kind and existing.subject is subject
                ),
                None,
            )
            if helper is None:
                self._transport_helper_sequence += 1
                helper = _PM400TransportHelper(
                    self._transport_helper_sequence, kind, subject, target
                )
                self._transport_helpers.append(helper)
                try:
                    helper.thread.start()
                except RuntimeError:
                    # Fresh CPython Thread native launch failure precedes ident
                    # publication. Keep interruptions and any observed launched
                    # thread owned; only the ordinary unstarted failure retries.
                    if helper.thread.ident is None:
                        self._transport_helpers.remove(helper)
                    raise
        if not helper.done.wait(self._TRANSPORT_HELPER_SECONDS):
            return False, None
        error = helper.error
        self._consume_transport_helper(helper)
        return True, error

    def _terminal_close_resource_without_request_lock(
        self, evidence: BaseException
    ) -> bool:
        """Last-resort unblock after bounded interrupt/request-lock attempts."""
        self._publish_terminal_fault(evidence)
        with self._lifecycle_lock:
            resource = self._resource
        if resource is None:
            return True
        completed, close_error = self._bounded_transport_call(
            "close", resource, resource.close
        )
        if not completed:
            self.log.error("PM400 terminal resource-close helper remains unresolved")
            return False
        if close_error is not None:
            self.log.error(
                "PM400 terminal resource-close fallback failed: %s",
                type(close_error).__name__,
            )
            translated = self._translated_transport_error(
                close_error, "terminally closing the VISA resource"
            )
            self._publish_terminal_fault(translated)
            return False
        with self._lifecycle_lock:
            cleanup_complete = self._resource is not resource
        if not cleanup_complete:
            self.log.error(
                "PM400 resource ownership retained for an unresolved sibling helper"
            )
        return cleanup_complete

    def _bounded_manager_lease_close(
        self,
    ) -> tuple[bool, BaseException | None]:
        with self._lifecycle_lock:
            manager = self._manager
            lease = self._visa_lease
        if manager is None or lease is None:
            return True, None
        return self._bounded_transport_call(
            "manager-close", manager, lease.close
        )

    def _interrupt_armed_measurement_io(self, resource: object, action: str) -> bool:
        """Repeat VISA clear until the armed call acknowledges exit or fault terminally."""
        last_error: BaseException | None = None
        for _ in range(self._INTERRUPT_RETRIES):
            with self._lifecycle_lock:
                if not self._measurement_io_armed:
                    return True
            completed, error = self._bounded_transport_call(
                "clear", resource, resource.clear
            )
            if not completed:
                evidence = DeviceFault(
                    "PM400 VISA clear did not complete within the interrupt bound"
                )
                self._terminal_close_resource_without_request_lock(evidence)
                return False
            if error is not None:
                last_error = self._translated_transport_error(error, action)
            with self._lifecycle_lock:
                if not self._measurement_io_armed:
                    return True
                self._lifecycle_condition.wait(self._INTERRUPT_RETRY_SECONDS)

        evidence = last_error or DeviceFault(
            "PM400 active VISA I/O did not acknowledge repeated interrupts"
        )
        self._terminal_close_resource_without_request_lock(evidence)
        return False

    def _finish_active_measurement(self) -> None:
        with self._lifecycle_lock:
            if self._is_active_measurement_thread_locked():
                self._measurement_io_armed = False
                self._active_measurement_thread_id = None
                self._active_measurement_generation = None
            if self.state == DriverState.ACTIVE:
                self.state = DriverState.READY
                self._measurement_cancel_event.clear()
            self._lifecycle_condition.notify_all()

    def _resynchronize_measurement_locked(
        self, resource: object, *, known_pending_response: bool
    ) -> None:
        """Abort and prove a clean response boundary while holding the request lock."""
        try:
            resource.write("ABORt")
        except BaseException as error:
            self._raise_transport_error(error, "aborting an interrupted measurement")

        if known_pending_response:
            prior_timeout: object | None = None
            timeout_saved = False
            primary_error: BaseException | None = None
            try:
                prior_timeout = resource.timeout
                timeout_saved = True
                resource.timeout = 1
                try:
                    resource.read()
                except BaseException as error:
                    if not self._is_timeout_error(error):
                        primary_error = self._translated_transport_error(
                            error, "draining the interrupted measurement response"
                        )
            except BaseException as error:
                primary_error = self._translated_transport_error(
                    error, "preparing to drain the interrupted measurement response"
                )
            finally:
                if timeout_saved:
                    try:
                        resource.timeout = prior_timeout
                    except BaseException as error:
                        restoration_error = self._translated_transport_error(
                            error,
                            "restoring the VISA timeout after measurement resynchronization",
                        )
                        if primary_error is None:
                            primary_error = restoration_error
                        else:
                            self.log.error(
                                "PM400 resynchronization timeout restoration failed: %s",
                                type(restoration_error).__name__,
                            )
            if primary_error is not None:
                raise primary_error

        try:
            response = resource.query("*OPC?")
        except BaseException as error:
            self._raise_transport_error(error, "querying the resynchronization sentinel")
        parsed = self._parse_single_response(response, "*OPC?")
        if parsed != "1":
            raise InstrumentProtocolError(
                f"PM400 resynchronization sentinel must be exact '1', received {parsed!r}"
            )
        self._reject_pending_response_data_locked(
            resource, "*OPC?", guard_measurement_io=False
        )
        with self._lifecycle_lock:
            timeout_restore_error = self._measurement_timeout_restore_error
        if timeout_restore_error is not None:
            try:
                resource.timeout = self._canonical_timeout_ms
                observed_timeout = resource.timeout
            except BaseException as error:
                self._raise_transport_error(
                    error, "restoring the canonical VISA timeout after resynchronization"
                )
            if observed_timeout != self._canonical_timeout_ms:
                raise InstrumentConnectionError(
                    "PM400 canonical VISA timeout could not be verified after resynchronization"
                )
            with self._lifecycle_lock:
                self._measurement_timeout_restore_error = None
                if self.cleanup_error is timeout_restore_error:
                    self.cleanup_error = None

    def _fault_and_close_measurement_locked(self, evidence: BaseException) -> None:
        """Latch a resynchronization failure and truthfully close what can be closed."""
        resource_closed = self._terminal_close_resource_without_request_lock(evidence)
        if not resource_closed:
            return

        manager_completed, manager_error = self._bounded_manager_lease_close()
        if not manager_completed:
            self.log.error(
                "PM400 manager lease-close helper remains unresolved after resynchronization"
            )
            return
        if manager_error is not None:
            self.log.error(
                "PM400 manager lease close failed after resynchronization: %s",
                type(manager_error).__name__,
            )
            translated = self._translated_transport_error(
                manager_error, "closing the VISA manager lease after resynchronization"
            )
            self._publish_terminal_fault(translated)
            return

    def _run_active_measurement(
        self,
        operation: Callable[[Callable[[], None]], Measurement],
        *,
        timeout: float | None,
    ) -> Measurement:
        """Serialize one long operation and recover its response boundary on interruption."""
        self._validate_timeout(timeout)
        if not callable(operation):
            raise InstrumentSafetyError("measurement operation must be callable")
        with self._lifecycle_lock:
            if self.state not in {DriverState.READY, DriverState.ACTIVE}:
                raise InstrumentSafetyError(
                    f"Cannot start a PM400 measurement while state is {self.state.name}"
                )
            generation = self._measurement_cancel_generation
            self._measurement_waiters += 1
            self._lifecycle_condition.notify_all()

        waiting = True
        active_started = False
        try:
            with self._request_lock:
                with self._lifecycle_lock:
                    self._measurement_waiters -= 1
                    waiting = False
                    self._lifecycle_condition.notify_all()
                    if generation != self._measurement_cancel_generation:
                        raise InstrumentTimeoutError(
                            "PM400 measurement was cancelled before it started"
                        )
                    if self.state != DriverState.READY:
                        raise InstrumentSafetyError(
                            f"Cannot start a PM400 measurement while state is {self.state.name}"
                        )
                    self.state = DriverState.ACTIVE
                    self._measurement_cancel_event.clear()
                    self._active_measurement_thread_id = threading.get_ident()
                    self._active_measurement_generation = generation
                    self._measurement_io_armed = False
                    self._measurement_timeout_restore_error = None
                    active_started = True
                    self._lifecycle_condition.notify_all()

                resource = self._resource_for_transaction_locked()
                query_started = False

                def mark_query_started() -> None:
                    nonlocal query_started
                    query_started = True

                try:
                    result = operation(mark_query_started)
                except BaseException as primary_error:
                    cancelled = self._measurement_was_cancelled(generation)
                    with self._lifecycle_lock:
                        timeout_restore_pending = (
                            self._measurement_timeout_restore_error is not None
                        )
                    interrupted = isinstance(
                        primary_error,
                        (InstrumentTimeoutError, InstrumentConnectionError, KeyboardInterrupt),
                    ) or timeout_restore_pending
                    if query_started and (cancelled or interrupted):
                        try:
                            self._resynchronize_measurement_locked(
                                resource, known_pending_response=True
                            )
                        except BaseException as resync_error:
                            self._fault_and_close_measurement_locked(resync_error)
                            if cancelled:
                                raise DeviceFault(
                                    "PM400 could not resynchronize after measurement cancellation"
                                ) from primary_error
                            raise primary_error
                    self._finish_active_measurement()
                    raise

                if self._measurement_was_cancelled(generation):
                    try:
                        self._resynchronize_measurement_locked(
                            resource, known_pending_response=False
                        )
                    except BaseException as resync_error:
                        self._fault_and_close_measurement_locked(resync_error)
                        raise DeviceFault(
                            "PM400 could not resynchronize after measurement cancellation"
                        ) from resync_error
                    self._finish_active_measurement()
                    raise InstrumentTimeoutError("PM400 measurement was cancelled")

                self._finish_active_measurement()
                return result
        finally:
            if active_started:
                self._finish_active_measurement()
            if waiting:
                with self._lifecycle_lock:
                    self._measurement_waiters -= 1
                    self._lifecycle_condition.notify_all()

    def cancel_measurement(self) -> None:
        """Cancel only the currently ACTIVE request and expire earlier queued requests."""
        with self._lifecycle_lock:
            if self.state != DriverState.ACTIVE:
                return
            self._measurement_cancel_generation += 1
            self._measurement_cancel_event.set()
            resource = self._resource
            self._lifecycle_condition.notify_all()
        if resource is not None:
            self._interrupt_armed_measurement_io(
                resource, "interrupting active measurement I/O"
            )

    def measure_power(self, timeout: float | None = None) -> Measurement:
        return self.measurement.measure(MeasurementKind.POWER, timeout=timeout)

    def measure_current(self, timeout: float | None = None) -> Measurement:
        return self.measurement.measure(MeasurementKind.CURRENT, timeout=timeout)

    def measure_voltage(self, timeout: float | None = None) -> Measurement:
        return self.measurement.measure(MeasurementKind.VOLTAGE, timeout=timeout)

    def measure_energy(self, timeout: float | None = None) -> Measurement:
        return self.measurement.measure(MeasurementKind.ENERGY, timeout=timeout)

    def measure_frequency(self, timeout: float | None = None) -> Measurement:
        return self.measurement.measure(MeasurementKind.FREQUENCY, timeout=timeout)

    def measure_power_density(self, timeout: float | None = None) -> Measurement:
        return self.measurement.measure(MeasurementKind.POWER_DENSITY, timeout=timeout)

    def measure_energy_density(self, timeout: float | None = None) -> Measurement:
        return self.measurement.measure(MeasurementKind.ENERGY_DENSITY, timeout=timeout)

    def measure_resistance(self, timeout: float | None = None) -> Measurement:
        return self.measurement.measure(MeasurementKind.RESISTANCE, timeout=timeout)

    def measure_temperature(self, timeout: float | None = None) -> Measurement:
        return self.measurement.measure(MeasurementKind.TEMPERATURE, timeout=timeout)

    @staticmethod
    def _validate_identity(instrument_info: InstrumentInfo) -> None:
        if instrument_info.manufacturer.strip().upper() != "THORLABS":
            raise InstrumentConnectionError(
                f"Unexpected PM400 manufacturer: {instrument_info.manufacturer!r}"
            )
        if instrument_info.model.strip().upper() != "PM400":
            raise InstrumentConnectionError(f"Unexpected PM400 model: {instrument_info.model!r}")

    def connect(self) -> PM400:
        with self._lifecycle_lock:
            _probe_connect_guard(self)
            if self.state in {DriverState.READY, DriverState.ACTIVE}:
                return self
            if self.state != DriverState.DISCONNECTED:
                raise InstrumentConnectionError(
                    f"Cannot connect PM400 while state is {self.state.name}"
                )
            self.state = DriverState.CONNECTING
            self.cleanup_error = None
            self._measurement_cancel_event.clear()
            try:
                self._visa_lease = acquire_visa_manager(
                    resource_manager=self._injected_manager,
                    factory=self._resource_manager_factory,
                )
                self._manager = self._visa_lease.manager
                self._manager_owned = self._injected_manager is None
                self._resource_reservation = self._visa_lease.reserve_resource(
                    self.resource_name
                )
                self._resource = self._manager.open_resource(
                    self._resource_reservation.canonical_name
                )
                self._configure_resource(self._resource)
                instrument_info = InstrumentInfo.parse(str(self._query("*IDN?")))
                self._validate_identity(instrument_info)
                sensor_info = SensorInfo.parse(str(self._query("SYSTem:SENSor:IDN?")))
                self.instrument_info = instrument_info
                self.sensor_info = sensor_info
                self.state = DriverState.READY
                self._lifecycle_condition.notify_all()
                return self
            except BaseException as error:
                self.state = DriverState.FAULT
                try:
                    self.close()
                except BaseException:
                    pass
                if not isinstance(error, Exception):
                    raise
                if isinstance(error, DriverError):
                    raise
                raise InstrumentConnectionError(
                    f"Failed to connect PM400 at {self.resource_name}"
                ) from error

    @property
    def has_resource_responsibility(self) -> bool:
        with self._lifecycle_lock:
            return (self.state is not DriverState.DISCONNECTED or self._resource is not None
                    or self._visa_lease is not None or self._resource_reservation is not None
                    or bool(self._transport_helpers))

    def probe_identity(self) -> ProbeReport:
        def read():
            from dataclasses import asdict
            info = self.instrument_info
            return {"manufacturer": info.manufacturer, "model": info.model,
                    "serial": info.serial_number, "firmware": info.firmware}, {
                    "sensor": asdict(self.sensor_info)}
        return _identity_probe(self, self.connect, read)

    def close(self) -> None:
        unresolved_at_start = self._wait_and_reap_transport_helpers()
        with self._lifecycle_lock:
            if (
                self.state == DriverState.DISCONNECTED
                and self._resource is None
                and self._manager is None
                and not unresolved_at_start
            ):
                return
            starting_fault_generation = self._terminal_fault_generation
            was_active = self._active_measurement_thread_id is not None
            self.state = DriverState.CLOSING
            if was_active:
                self._measurement_cancel_generation += 1
                self._measurement_cancel_event.set()
            resource_to_interrupt = self._resource if was_active else None
            self._lifecycle_condition.notify_all()

        if resource_to_interrupt is not None:
            self._interrupt_armed_measurement_io(
                resource_to_interrupt, "interrupting active measurement I/O for close"
            )

        if was_active:
            request_acquired = self._request_lock.acquire(
                timeout=self._ACTIVE_CLOSE_LOCK_SECONDS
            )
            if not request_acquired:
                lock_error = DeviceFault(
                    "PM400 active request did not release for bounded close"
                )
                self._terminal_close_resource_without_request_lock(lock_error)
                request_acquired = self._request_lock.acquire(
                    timeout=self._ACTIVE_CLOSE_LOCK_SECONDS
                )
                if not request_acquired:
                    raise lock_error
        else:
            self._request_lock.acquire()
            request_acquired = True

        try:
            error: BaseException | None = None
            resource_close_resolved = True
            with self._lifecycle_lock:
                resource = self._resource
            if resource is not None:
                close_completed, close_error = self._bounded_transport_call(
                    "close", resource, resource.close
                )
                if not close_completed:
                    resource_close_resolved = False
                    error = DeviceFault(
                        "PM400 VISA resource close remains unresolved after its bound"
                    )
                elif close_error is not None:
                    resource_close_resolved = False
                    error = close_error
            else:
                self._release_resource_name()

            unresolved_helpers = self._reap_transport_helpers()
            resource_helper_unresolved = resource is not None and any(
                helper.subject is resource for helper in unresolved_helpers
            )

            if (
                resource_close_resolved
                and not resource_helper_unresolved
                and self._manager is not None
                and self._resource is None
            ):
                manager_completed, manager_error = self._bounded_manager_lease_close()
                if not manager_completed:
                    if error is None:
                        error = DeviceFault(
                            "PM400 manager lease close remains unresolved after its bound"
                        )
                elif manager_error is not None and error is None:
                    error = manager_error

            unresolved_helpers = self._reap_transport_helpers()
            if error is None and unresolved_helpers:
                error = DeviceFault("PM400 transport helper remains unresolved")

            with self._lifecycle_lock:
                cleanup_incomplete = (
                    self._resource is not None
                    or self._manager is not None
                    or bool(self._transport_helpers)
                    or self._resource_close_completed is not None
                    or self._manager_close_completed is not None
                    or self._resource_reservation is not None
                    or self._visa_lease is not None
                )
            if error is None and cleanup_incomplete:
                error = DeviceFault("PM400 cleanup ownership remains unresolved")

            if error is not None:
                with self._lifecycle_lock:
                    fault_published_during_close = (
                        self._terminal_fault_generation
                        != starting_fault_generation
                    )
                if fault_published_during_close:
                    self.log.error(
                        "PM400 close failure did not replace concurrent fault evidence: %s",
                        type(error).__name__,
                    )
                    with self._lifecycle_lock:
                        self.state = DriverState.FAULT
                        self._measurement_cancel_event.set()
                        self._lifecycle_condition.notify_all()
                else:
                    self._publish_terminal_fault(error)
                raise error

            with self._lifecycle_lock:
                if self._terminal_fault_generation != starting_fault_generation:
                    self.state = DriverState.FAULT
                    self._measurement_cancel_event.set()
                else:
                    self.cleanup_error = None
                    self.state = DriverState.DISCONNECTED
                    self._measurement_cancel_event.clear()
                self._lifecycle_condition.notify_all()
        finally:
            if request_acquired:
                self._request_lock.release()

    def __enter__(self) -> PM400:
        return self.connect()

    def __exit__(self, exc_type, exc_value, traceback) -> bool:
        try:
            self.close()
        except BaseException:
            if exc_type is None:
                raise
        return False
