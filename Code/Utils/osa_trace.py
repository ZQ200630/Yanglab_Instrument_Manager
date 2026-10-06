"""Validated native AQ6370 trace data, independent of transport and storage."""

from __future__ import annotations

import math
import re
from dataclasses import dataclass
from datetime import datetime, timedelta
from numbers import Integral, Real

import numpy as np

from .common import InstrumentProtocolError


MAX_TRACE_POINTS = 200001
TRACE_CHUNK_POINTS = 1024
MAX_TRACE_REPLY_BYTES = 65536
_FORMATS = {"ASCII", "REAL,32", "REAL,64"}
_NUMBER = re.compile(rb"[ \t]*[+-]?(?:[0-9]+(?:\.[0-9]*)?|\.[0-9]+)"
                     rb"(?:[eE][+-]?[0-9]+)?[ \t]*")


def _integer(value, name: str, lower: int, upper: int) -> int:
    if isinstance(value, bool) or not isinstance(value, Integral) or not lower <= value <= upper:
        raise InstrumentProtocolError(f"OSA {name} must be an integer in {lower}..{upper}")
    return int(value)


def _finite(value, name: str, *, positive: bool = False) -> float:
    if (isinstance(value, bool) or not isinstance(value, Real) or not math.isfinite(value)
            or (value <= 0 if positive else value < 0)):
        raise InstrumentProtocolError(f"OSA {name} must be finite and {'positive' if positive else 'non-negative'}")
    return float(value)


def _format(value) -> str:
    if not isinstance(value, str):
        raise InstrumentProtocolError("OSA transfer format is not text")
    result = ",".join(part.strip().upper() for part in value.strip().split(","))
    if result not in _FORMATS:
        raise InstrumentProtocolError(f"Unsupported OSA transfer format: {value!r}")
    return result


def _immutable(values) -> np.ndarray:
    try:
        result = np.array(values, dtype=float, copy=True)
    except (TypeError, ValueError, OverflowError) as error:
        raise InstrumentProtocolError("OSA samples must be numeric") from error
    if result.ndim != 1 or not 1 <= len(result) <= MAX_TRACE_POINTS:
        raise InstrumentProtocolError("OSA samples must be a bounded non-empty one-dimensional array")
    if not np.all(np.isfinite(result)):
        raise InstrumentProtocolError("OSA samples contain non-finite values")
    return np.frombuffer(result.tobytes(), dtype=result.dtype)


def decode_trace_reply(reply: bytes, transfer_format: str, expected_count: int) -> np.ndarray:
    """Decode one bounded range; never accept a partial or oversized response."""
    expected = _integer(expected_count, "chunk sample count", 1, TRACE_CHUNK_POINTS)
    fmt = _format(transfer_format)
    if not isinstance(reply, bytes) or len(reply) > MAX_TRACE_REPLY_BYTES:
        raise InstrumentProtocolError("OSA trace response is not bounded bytes")
    if fmt == "ASCII":
        body = reply.removesuffix(b"\r\n") if reply.endswith(b"\r\n") else reply.removesuffix(b"\n")
        fields = body.split(b",")
        if len(fields) != expected or any(_NUMBER.fullmatch(field) is None for field in fields):
            raise InstrumentProtocolError("OSA ASCII trace has malformed samples or wrong count")
        return _immutable([float(field) for field in fields])

    if len(reply) < 2 or reply[:1] != b"#" or reply[1:2] not in b"123456789":
        raise InstrumentProtocolError("OSA binary trace requires a definite-length IEEE header")
    digits = int(reply[1:2])
    header_end = 2 + digits
    length_field = reply[2:header_end]
    if len(length_field) != digits or not length_field.isdigit():
        raise InstrumentProtocolError("OSA binary trace has a malformed byte count")
    width = 4 if fmt == "REAL,32" else 8
    byte_count = int(length_field)
    if byte_count != expected * width:
        raise InstrumentProtocolError("OSA binary trace byte count does not match the requested range")
    data_end = header_end + byte_count
    if len(reply) < data_end or reply[data_end:] not in (b"", b"\n", b"\r\n"):
        raise InstrumentProtocolError("OSA binary trace is truncated or has an invalid suffix")
    return _immutable(np.frombuffer(reply[header_end:data_end], dtype=f"<f{width}", count=expected))


@dataclass(frozen=True)
class TraceContext:
    transfer_format: str
    sample_count: int
    spacing: int
    level_unit: int
    x_unit: int
    trace_attribute: int
    active_trace: str
    center_m: float
    span_m: float
    resolution_m: float
    sweep_mode: int

    def __post_init__(self) -> None:
        object.__setattr__(self, "transfer_format", _format(self.transfer_format))
        for name, lower, upper in (("sample_count", 1, MAX_TRACE_POINTS),
                                   ("spacing", 0, 1), ("level_unit", 0, 3), ("x_unit", 0, 0),
                                   ("trace_attribute", 0, 5), ("sweep_mode", 1, 3)):
            object.__setattr__(self, name, _integer(getattr(self, name), name, lower, upper))
        if self.active_trace not in ("TRA", "TRB", "TRC", "TRD", "TRE", "TRF", "TRG"):
            raise InstrumentProtocolError("OSA active trace must identify TRA through TRG")
        for name in ("center_m", "span_m", "resolution_m"):
            object.__setattr__(self, name, _finite(getattr(self, name), name, positive=name != "span_m"))

    @property
    def native_unit(self) -> str:
        """Only independently interpretable absolute optical-power transfers."""
        if self.trace_attribute == 5:
            raise InstrumentProtocolError("OSA CALC trace interpretation is not validated")
        if self.level_unit in (2, 3):
            raise InstrumentProtocolError("OSA density-mode trace interpretation is not validated")
        if (self.spacing, self.level_unit) == (0, 0):
            return "dBm"
        if (self.spacing, self.level_unit) == (1, 1):
            return "W"
        raise InstrumentProtocolError("OSA level unit and main scale disagree")


@dataclass(frozen=True)
class TraceCapture:
    wavelength_nm: np.ndarray
    native_values: np.ndarray
    native_unit: str
    trace: str
    identity: str
    read_started_at: datetime
    read_finished_at: datetime
    elapsed_s: float
    context_before: TraceContext
    context_after: TraceContext
    consistency: str = "unproven"

    def __post_init__(self) -> None:
        if not isinstance(self.context_before, TraceContext) or not isinstance(self.context_after, TraceContext):
            raise InstrumentProtocolError("OSA capture requires validated panel context")
        if self.context_before != self.context_after:
            raise InstrumentProtocolError("OSA panel context changed during trace transfer")
        if self.native_unit != self.context_before.native_unit:
            raise InstrumentProtocolError("OSA native unit does not match the panel context")
        if self.consistency != "unproven":
            raise InstrumentProtocolError("Sequential OSA trace reads do not prove atomicity")
        if not isinstance(self.trace, str) or self.trace not in tuple("ABCDEFG"):
            raise InstrumentProtocolError("OSA capture trace must identify A through G")
        if not isinstance(self.identity, str) or not self.identity.strip():
            raise InstrumentProtocolError("OSA capture has no source identity")
        for stamp in (self.read_started_at, self.read_finished_at):
            if not isinstance(stamp, datetime) or stamp.utcoffset() != timedelta(0):
                raise InstrumentProtocolError("OSA read timestamps must be timezone-aware UTC")
        object.__setattr__(self, "elapsed_s", _finite(self.elapsed_s, "read elapsed time"))
        wavelength, values = _immutable(self.wavelength_nm), _immutable(self.native_values)
        if wavelength.shape != values.shape or len(values) != self.context_before.sample_count:
            raise InstrumentProtocolError("OSA sample count and array lengths disagree")
        if np.any(wavelength <= 0) or (len(wavelength) > 1 and np.any(np.diff(wavelength) <= 0)):
            raise InstrumentProtocolError("OSA wavelengths must be positive and strictly increasing")
        if self.native_unit == "W" and np.any(values < 0):
            raise InstrumentProtocolError("OSA absolute optical power cannot be negative")
        object.__setattr__(self, "wavelength_nm", wavelength)
        object.__setattr__(self, "native_values", values)

    @property
    def power_dbm(self) -> np.ndarray | None:
        """Compatibility only, not a replacement for the exact native samples."""
        if self.native_unit == "dBm":
            return self.native_values
        if np.any(self.native_values <= 0):
            return None
        # log10(W) + 3 avoids overflow/underflow from first dividing by .001.
        return _immutable(10. * (np.log10(self.native_values) + 3.))
