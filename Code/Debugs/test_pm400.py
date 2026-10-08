from __future__ import annotations

import datetime
import io
import math
import sys
import threading
import unittest
from contextlib import redirect_stderr, redirect_stdout
from dataclasses import FrozenInstanceError
from types import SimpleNamespace
from unittest.mock import patch

from pyvisa.constants import StatusCode
from pyvisa.errors import VisaIOError

from Code.Utils.common import (
    DeviceFault,
    DriverError,
    DriverState,
    InstrumentConnectionError,
    InstrumentProtocolError,
    InstrumentSafetyError,
    InstrumentTimeoutError,
)

from Code.Utils.pm400 import (
    AdapterType,
    InstrumentCapabilityError,
    InstrumentInfo,
    LimitSelector,
    Measurement,
    MeasurementKind,
    PowerUnit,
    PM400,
    SensorInfo,
    StatusGroup,
    SystemError,
)

from Code.Debugs import check_pm400
from Code.Utils.visa import acquire_visa_manager


PM400_ROOT_COMMANDS = {
    "*CLS", "*ESE", "*ESE?", "*ESR?", "*IDN?", "*OPC", "*OPC?",
    "*RST", "*SRE", "*SRE?", "*STB?", "*TST?", "*WAI",
}
PM400_SYSTEM_COMMANDS = {
    "SYSTem:BEEPer", "SYSTem:BEEPer:STATe", "SYSTem:BEEPer:STATe?",
    "SYSTem:ERRor?", "SYSTem:VERSion?", "SYSTem:DATE", "SYSTem:DATE?",
    "SYSTem:TIME", "SYSTem:TIME?", "SYSTem:LFRequency",
    "SYSTem:LFRequency?", "SYSTem:SENSor:IDN?",
}
PM400_STATUS_COMMANDS = {
    "STATus:PRESet",
    *(f"STATus:{group.value}:{suffix}" for group in StatusGroup for suffix in (
        "EVENt?", "CONDition?", "PTRansition", "PTRansition?",
        "NTRansition", "NTRansition?", "ENABle", "ENABle?",
    )),
}
PM400_DISPLAY_COMMANDS = {
    "DISPlay:BRIGhtness", "DISPlay:BRIGhtness?",
    "DISPlay:CONTrast", "DISPlay:CONTrast?",
}
PM400_CALIBRATION_COMMANDS = {"CALibration:STRing?"}


class PM400TypeTests(unittest.TestCase):
    def test_capability_error_belongs_to_driver_taxonomy(self):
        self.assertTrue(issubclass(InstrumentCapabilityError, DriverError))

    def test_sensor_flags_decode_independently(self):
        info = SensorInfo.parse('"S130C","SN1","2025-01",1,2,371')
        self.assertEqual(info.name, "S130C")
        self.assertTrue(info.capabilities.power)
        self.assertTrue(info.capabilities.energy)
        self.assertTrue(info.capabilities.response_settable)
        self.assertTrue(info.capabilities.wavelength_settable)
        self.assertTrue(info.capabilities.tau_settable)
        self.assertTrue(info.capabilities.temperature_sensor)

    def test_sensor_capability_flags_decode_each_supported_bit(self):
        cases = (
            (0, (False, False, False, False, False, False)),
            (1, (True, False, False, False, False, False)),
            (2, (False, True, False, False, False, False)),
            (16, (False, False, True, False, False, False)),
            (32, (False, False, False, True, False, False)),
            (64, (False, False, False, False, True, False)),
            (256, (False, False, False, False, False, True)),
            (371, (True, True, True, True, True, True)),
        )
        for flags, expected in cases:
            with self.subTest(flags=flags):
                info = SensorInfo.parse(f'"S","SN","cal",1,2,{flags}')
                self.assertEqual(
                    (
                        info.capabilities.power,
                        info.capabilities.energy,
                        info.capabilities.response_settable,
                        info.capabilities.wavelength_settable,
                        info.capabilities.tau_settable,
                        info.capabilities.temperature_sensor,
                    ),
                    expected,
                )

    def test_sensor_info_preserves_unknown_flag_bits(self):
        info = SensorInfo.parse('"S","SN","cal",1,2,1025')
        self.assertEqual(info.raw_flags, 1025)
        self.assertTrue(info.capabilities.power)

    def test_sensor_parser_handles_quoted_fields_containing_commas(self):
        info = SensorInfo.parse('"S,130C","SN,1","cal, 2025",1,2,0')
        self.assertEqual(info.name, "S,130C")
        self.assertEqual(info.serial_number, "SN,1")
        self.assertEqual(info.calibration_message, "cal, 2025")

    def test_instrument_info_parses_exactly_four_identity_fields(self):
        info = InstrumentInfo.parse('"Thorlabs","PM400","P123","1.2.3"')
        self.assertEqual(info.manufacturer, "Thorlabs")
        self.assertEqual(info.model, "PM400")
        self.assertEqual(info.serial_number, "P123")
        self.assertEqual(info.firmware, "1.2.3")
        self.assertEqual(info.raw, '"Thorlabs","PM400","P123","1.2.3"')

    def test_identity_parser_rejects_wrong_field_count_and_empty_required_fields(self):
        for response in ("A,B,C", "A,B,C,D,E", "A,,C,D"):
            with self.subTest(response=response):
                with self.assertRaises(InstrumentProtocolError):
                    InstrumentInfo.parse(response)

    def test_sensor_parser_rejects_invalid_integer_fields(self):
        for response in (
            '"S","SN","cal",one,2,0',
            '"S","SN","cal",True,2,0',
            '"S","SN","cal",1,2.5,0',
            '"S","SN","cal",1,2,',
        ):
            with self.subTest(response=response):
                with self.assertRaises(InstrumentProtocolError):
                    SensorInfo.parse(response)


class PM400ValueTypeTests(unittest.TestCase):
    """Task 14 fail-closed sensor capability parsing regressions."""

    def test_sensor_parser_rejects_negative_capability_flags(self):
        with self.assertRaises(InstrumentProtocolError):
            SensorInfo.parse('"S","SN","cal",1,2,-1')

    def test_sensor_parser_preserves_unknown_nonnegative_bits(self):
        info = SensorInfo.parse('"S","SN","cal",1,2,1025')
        self.assertEqual(info.raw_flags, 1025)
        self.assertTrue(info.capabilities.power)
        self.assertFalse(info.capabilities.energy)
        self.assertFalse(info.capabilities.response_settable)
        self.assertFalse(info.capabilities.wavelength_settable)
        self.assertFalse(info.capabilities.tau_settable)
        self.assertFalse(info.capabilities.temperature_sensor)

    def test_value_types_are_immutable(self):
        info = InstrumentInfo.parse("A,B,C,D")
        with self.assertRaises(FrozenInstanceError):
            info.model = "other"

    def test_measurement_rejects_nonfinite_device_value(self):
        with self.assertRaises(InstrumentProtocolError):
            Measurement.parse(MeasurementKind.POWER, "9.9E37", 10.0)

    def test_measurement_rejects_invalid_values_and_timestamps(self):
        for value, timestamp in (
            ("nan", 1.0),
            ("inf", 1.0),
            (True, 1.0),
            (1.0, math.nan),
            (1.0, math.inf),
            (1.0, True),
        ):
            with self.subTest(value=value, timestamp=timestamp):
                with self.assertRaises(InstrumentProtocolError):
                    Measurement.parse(MeasurementKind.POWER, value, timestamp)

    def test_measurement_parses_all_kinds_with_wire_units(self):
        expected = (
            (MeasurementKind.POWER, "POWer", "W"),
            (MeasurementKind.CURRENT, "CURRent:DC", "A"),
            (MeasurementKind.VOLTAGE, "VOLTage:DC", "V"),
            (MeasurementKind.ENERGY, "ENERgy", "J"),
            (MeasurementKind.FREQUENCY, "FREQuency", "Hz"),
            (MeasurementKind.POWER_DENSITY, "PDENsity", "W/cm^2"),
            (MeasurementKind.ENERGY_DENSITY, "EDENsity", "J/cm^2"),
            (MeasurementKind.RESISTANCE, "RESistance", "ohm"),
            (MeasurementKind.TEMPERATURE, "TEMPerature", "degC"),
        )
        for kind, suffix, unit in expected:
            with self.subTest(kind=kind):
                measurement = Measurement.parse(kind, "1.25", 10.0)
                self.assertEqual(kind.scpi_suffix, suffix)
                self.assertEqual(kind.default_unit, unit)
                self.assertEqual(measurement.unit, unit)
                self.assertEqual(measurement.value, 1.25)
                self.assertEqual(measurement.measured_at, 10.0)

    def test_remaining_enum_wire_values(self):
        self.assertEqual(PowerUnit.WATTS.value, "W")
        self.assertEqual(PowerUnit.DBM.value, "DBM")
        self.assertEqual(AdapterType.PHOTODIODE.value, "PHOTodiode")
        self.assertEqual(AdapterType.THERMAL.value, "THERmal")
        self.assertEqual(AdapterType.PYRO.value, "PYRo")
        self.assertEqual(StatusGroup.MEASUREMENT.value, "MEASurement")
        self.assertEqual(StatusGroup.AUXILIARY.value, "AUXiliary")
        self.assertEqual(StatusGroup.OPERATION.value, "OPERation")
        self.assertEqual(StatusGroup.QUESTIONABLE.value, "QUEStionable")
        self.assertEqual(LimitSelector.MINIMUM.value, "MINimum")
        self.assertEqual(LimitSelector.MAXIMUM.value, "MAXimum")
        self.assertEqual(LimitSelector.DEFAULT.value, "DEFault")

    def test_system_error_parses_and_rejects_malformed_responses(self):
        error = SystemError.parse('-222,"Data out of range"')
        self.assertEqual(error.code, -222)
        self.assertEqual(error.message, "Data out of range")
        self.assertEqual(error.raw, '-222,"Data out of range"')
        for response in ("0", "0,", "true,message", "0,message,extra"):
            with self.subTest(response=response):
                with self.assertRaises(InstrumentProtocolError):
                    SystemError.parse(response)


class FakeVisaResource:
    """Deterministic VISA double that records every driver-visible operation."""

    def __init__(self, replies, *, failures=None):
        self.replies = dict(replies)
        self.failures = dict(failures or {})
        self.queued_replies = {}
        self.pending_reads = []
        self.events = []
        self.transactions = []
        self.mutating_commands = []
        self.closed = False
        self.close_calls = 0
        self._timeout = None
        self._read_termination = None
        self._write_termination = None

    @staticmethod
    def _result(value):
        if isinstance(value, BaseException):
            raise value
        if callable(value):
            return value()
        return value

    @property
    def timeout(self):
        return self._timeout

    @timeout.setter
    def timeout(self, value):
        self.events.append(("timeout", value))
        self._result(self.failures.get(("timeout", value)))
        self._timeout = value

    @property
    def read_termination(self):
        return self._read_termination

    @read_termination.setter
    def read_termination(self, value):
        self.events.append(("read_termination", value))
        self._read_termination = value

    @property
    def write_termination(self):
        return self._write_termination

    @write_termination.setter
    def write_termination(self, value):
        self.events.append(("write_termination", value))
        self._write_termination = value

    def write(self, command):
        self.events.append(("write", command))
        self.transactions.append(command)
        self.mutating_commands.append(command)
        return self._result(self.failures.get(("write", command), 1))

    def read(self):
        self.events.append(("read",))
        if "read" in self.failures:
            return self._result(self.failures["read"])
        if self.pending_reads:
            return self._result(self.pending_reads.pop(0))
        raise VisaIOError(StatusCode.error_timeout)

    def query(self, command):
        self.events.append(("query", command))
        self.transactions.append(command)
        if ("query", command) in self.failures:
            return self._result(self.failures[("query", command)])
        queued = self.queued_replies.get(command)
        if queued:
            return self._result(queued.pop(0))
        if command == "SYSTem:ERRor?":
            return '0,"No error"'
        return self._result(self.replies[command])

    def queue(self, command, *responses):
        self.queued_replies.setdefault(command, []).extend(responses)

    def queue_pending_read(self, *responses):
        self.pending_reads.extend(responses)

    def clear(self):
        self.events.append(("clear",))
        return self._result(self.failures.get("clear"))

    def close(self):
        self.events.append(("close",))
        self.close_calls += 1
        result = self._result(self.failures.get("close"))
        self.closed = True
        return result


class GatedVisaResource(FakeVisaResource):
    """Fake whose selected query remains in-flight until VISA clear interrupts it."""

    def __init__(
        self,
        replies,
        *,
        blocked_command="READ?",
        blocked_result=None,
        resync_fails=False,
        failures=None,
    ):
        super().__init__(replies, failures=failures)
        self.blocked_command = blocked_command
        self.blocked_result = blocked_result
        self.resync_fails = resync_fails
        self.read_blocked = threading.Event()
        self.query_released = threading.Event()
        self.query_exited = threading.Event()
        self.close_attempted = False
        self.closed_while_query_blocked = False

    def query(self, command):
        if command != self.blocked_command:
            return super().query(command)
        self.events.append(("query", command))
        self.transactions.append(command)
        self.read_blocked.set()
        if not self.query_released.wait(2.0):
            self.query_exited.set()
            raise AssertionError("gated PM400 query was never interrupted")
        if self.blocked_result is not None:
            self.query_exited.set()
            return self.blocked_result
        self.pending_reads.append("discarded interrupted measurement")
        self.query_exited.set()
        raise VisaIOError(StatusCode.error_timeout)

    def clear(self):
        result = super().clear()
        self.query_released.set()
        return result

    def close(self):
        self.close_attempted = True
        self.closed_while_query_blocked = not self.query_exited.is_set()
        self.query_released.set()
        return super().close()


class LostInterruptVisaResource(FakeVisaResource):
    """Makes the first clear happen after arming but before the read can block."""

    def __init__(self, replies, *, blocked_command="READ?"):
        super().__init__(replies)
        self.blocked_command = blocked_command
        self.query_invoked = threading.Event()
        self.allow_blocking_read = threading.Event()
        self.blocking_read_started = threading.Event()
        self.read_released = threading.Event()
        self.first_early_clear = threading.Event()
        self.clear_calls = 0

    def query(self, command):
        if command != self.blocked_command:
            return super().query(command)
        self.events.append(("query", command))
        self.transactions.append(command)
        self.query_invoked.set()
        if not self.allow_blocking_read.wait(2.0):
            raise AssertionError("blocking VISA read was never allowed to start")
        self.blocking_read_started.set()
        if not self.read_released.wait(2.0):
            raise AssertionError("repeated clear never interrupted the armed VISA read")
        self.pending_reads.append("discarded interrupted measurement")
        raise VisaIOError(StatusCode.error_timeout)

    def clear(self):
        self.events.append(("clear",))
        self.clear_calls += 1
        if self.blocking_read_started.is_set():
            self.read_released.set()
        else:
            self.first_early_clear.set()


class GatedCloseVisaResource(GatedVisaResource):
    def __init__(self, replies, **kwargs):
        super().__init__(replies, **kwargs)
        self.close_started = threading.Event()
        self.allow_close = threading.Event()

    def close(self):
        self.close_started.set()
        if not self.allow_close.wait(2.0):
            raise AssertionError("terminal VISA close was never released")
        return super().close()


class BlockingClearVisaResource(GatedVisaResource):
    def __init__(self, replies, **kwargs):
        super().__init__(replies, **kwargs)
        self.clear_started = threading.Event()
        self.allow_clear = threading.Event()

    def clear(self):
        self.events.append(("clear",))
        self.clear_started.set()
        if not self.allow_clear.wait(5.0):
            raise AssertionError("blocking VISA clear was never released")
        self.query_released.set()


class BlockingCloseVisaResource(GatedVisaResource):
    def __init__(self, replies, **kwargs):
        super().__init__(replies, **kwargs)
        self.close_started = threading.Event()
        self.allow_close = threading.Event()

    def close(self):
        self.close_attempted = True
        self.close_started.set()
        if not self.allow_close.wait(5.0):
            raise AssertionError("blocking VISA close was never released")
        return super().close()


class GatedPM400(PM400):
    """Driver test double with gates around otherwise real transaction code."""

    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.gate_before_first_io = False
        self.first_io_gate_entered = threading.Event()
        self.release_first_io_gate = threading.Event()
        self.gate_after_command = None
        self.between_io_gate_entered = threading.Event()
        self.release_between_io_gate = threading.Event()

    def _resource_for_transaction_locked(self):
        resource = super()._resource_for_transaction_locked()
        if self.gate_before_first_io:
            self.gate_before_first_io = False
            self.first_io_gate_entered.set()
            if not self.release_first_io_gate.wait(2.0):
                raise AssertionError("pre-I/O driver gate was never released")
        return resource

    def _query_text(self, command, *, timeout=None):
        response = super()._query_text(command, timeout=timeout)
        if command == self.gate_after_command:
            self.gate_after_command = None
            self.between_io_gate_entered.set()
            if not self.release_between_io_gate.wait(2.0):
                raise AssertionError("between-I/O driver gate was never released")
        return response


class ThreadCall:
    """Small deterministic future used by the gated fake-I/O tests."""

    def __init__(self, target, *, name):
        self._target = target
        self._value = None
        self._error = None
        self.started = threading.Event()
        self.thread = threading.Thread(target=self._run, name=name)
        self.thread.start()

    def _run(self):
        self.started.set()
        try:
            self._value = self._target()
        except BaseException as error:
            self._error = error

    def result(self, timeout=2.0):
        self.thread.join(timeout)
        if self.thread.is_alive():
            raise AssertionError(f"thread {self.thread.name!r} did not terminate")
        if self._error is not None:
            raise self._error
        return self._value


class FakeResourceManager:
    def __init__(self, resource=None, *, open_failure=None, close_failure=None):
        self.resource = resource
        self.open_failure = open_failure
        self.close_failure = close_failure
        self.opened_names = []
        self.closed = False
        self.close_calls = 0

    def list_resources(self):
        return ()

    def open_resource(self, resource_name):
        self.opened_names.append(resource_name)
        if self.open_failure is not None:
            raise self.open_failure
        return self.resource

    def close(self):
        self.close_calls += 1
        if self.close_failure is not None:
            failure = self.close_failure
            if isinstance(failure, BaseException):
                raise failure
            if callable(failure):
                return failure()
        self.closed = True


class BlockingResourceManager(FakeResourceManager):
    """Owned manager whose close remains blocked until the test releases it."""

    def __init__(self, resource=None):
        super().__init__(resource)
        self.close_started = threading.Event()
        self.allow_close = threading.Event()

    def close(self):
        self.close_calls += 1
        self.close_started.set()
        if not self.allow_close.wait(5.0):
            raise AssertionError("blocking VISA manager close was never released")
        self.closed = True


class SharedResourceManager(FakeResourceManager):
    """Model VISA manager closure invalidating every resource it opened."""

    def __init__(self, resources):
        super().__init__()
        self.resources = resources

    def open_resource(self, resource_name):
        self.opened_names.append(resource_name)
        return FakeVisaResource._result(self.resources[resource_name])

    def close(self):
        super().close()
        for name in self.opened_names:
            resource = self.resources[name]
            if isinstance(resource, FakeVisaResource) and not resource.closed:
                resource.close()


class PM400SharedManagerTests(unittest.TestCase):
    def test_closing_one_shared_manager_device_preserves_other_device(self):
        self.check_other_device_survives(fail_second=False)

    def test_failed_connection_preserves_existing_shared_manager_device(self):
        self.check_other_device_survives(fail_second="open")

    def test_failed_identity_preserves_existing_shared_manager_device(self):
        self.check_other_device_survives(fail_second="identity")

    def check_other_device_survives(self, *, fail_second):
        names = ("USB0::0x1313::0x8078::P1::0::INSTR",
                 "USB0::0x1313::0x8078::P2::0::INSTR")
        replies = {"*IDN?": "THORLABS,PM400,P001,2.0.0",
                   "SYSTem:SENSor:IDN?": '"S130C","S001","cal",1,0,49',
                   "SYSTem:VERSion?": "1999.0"}
        first_resource = FakeVisaResource(replies)
        second_resource = FakeVisaResource(replies)
        if fail_second == "identity":
            second_resource.failures[("query", "*IDN?")] = OSError("identity failed")
        manager = SharedResourceManager(dict(zip(names, (
            first_resource, OSError("open failed") if fail_second == "open" else second_resource
        ))))
        first, second = [PM400(name, resource_manager_factory=lambda: manager)
                         for name in names]
        try:
            first.connect()
            if fail_second:
                with self.assertRaises(InstrumentConnectionError):
                    second.connect()
                survivor, survivor_resource = first, first_resource
            else:
                second.connect()
                first.close()
                survivor, survivor_resource = second, second_resource
            self.assertFalse(survivor_resource.closed)
            self.assertEqual(survivor.system.get_scpi_version(), "1999.0")
            self.assertEqual(manager.close_calls, 0)
        finally:
            first.close()
            second.close()
        self.assertEqual(manager.close_calls, 1)

    def test_global_reservation_blocks_pm400_alias_before_open(self):
        canonical = "USB0::0x1313::0x8078::P1::0::INSTR"
        owner_manager = FakeResourceManager()
        owner = acquire_visa_manager(resource_manager=owner_manager)
        reservation = owner.reserve_resource(canonical)
        manager = FakeResourceManager(FakeVisaResource({
            "*IDN?": "THORLABS,PM400,P001,2.0.0",
            "SYSTem:SENSor:IDN?": '"S130C","S001","cal",1,0,49',
        }))
        manager.resource_info = lambda name: SimpleNamespace(resource_name=canonical)
        driver = PM400("PowerMeter", resource_manager=manager)
        try:
            with self.assertRaises(InstrumentConnectionError):
                driver.connect()
            self.assertEqual(manager.opened_names, [])
            reservation.release()
            driver.connect()
            self.assertEqual(manager.opened_names, [canonical])
            driver.close()
            reclaimed = owner.reserve_resource(canonical)
            reclaimed.release()
            self.assertEqual(manager.close_calls, 0)
        finally:
            driver.close()
            reservation.release()
            owner.close()

    def test_failed_manager_close_retains_lease_until_explicit_retry(self):
        failure = OSError("manager close failed")
        resource = FakeVisaResource({
            "*IDN?": "THORLABS,PM400,P001,2.0.0",
            "SYSTem:SENSor:IDN?": '"S130C","S001","cal",1,0,49',
        })
        manager = FakeResourceManager(resource, close_failure=failure)
        driver = PM400("USB0::0x1313::0x8078::P1::INSTR",
                       resource_manager_factory=lambda: manager)
        rejected = PM400("USB0::0x1313::0x8078::P2::INSTR",
                         resource_manager_factory=lambda: manager)
        try:
            driver.connect()
            with self.assertRaises(OSError) as caught:
                driver.close()
            self.assertIs(caught.exception, failure)
            self.assertIs(driver.cleanup_error, failure)
            self.assertEqual(driver.state, DriverState.FAULT)
            self.assertTrue(resource.closed)
            self.assertFalse(driver._visa_lease.released)
            with self.assertRaises(InstrumentConnectionError):
                rejected.connect()
            self.assertEqual(manager.close_calls, 1)
        finally:
            manager.close_failure = None
            driver.close()
            rejected.close()
        self.assertEqual(manager.close_calls, 2)
        self.assertIsNone(driver._visa_lease)
        self.assertIsNone(driver._manager)
        self.assertEqual(driver.state, DriverState.DISCONNECTED)


class StaleFallbackPM400(PM400):
    """Pauses one fallback after its resource snapshot but before helper launch."""

    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.stale_fallback_entered = threading.Event()
        self.release_stale_fallback = threading.Event()

    def _bounded_transport_call(self, kind, resource, target):
        if (
            kind == "close"
            and threading.current_thread().name == "PM400StaleFallback"
        ):
            self.stale_fallback_entered.set()
            if not self.release_stale_fallback.wait(2.0):
                raise AssertionError("stale fallback gate was never released")
        return super()._bounded_transport_call(kind, resource, target)


class PM400CoreFacadeTests(unittest.TestCase):
    _IDN = "THORLABS,PM400,P001,2.0.0"
    _SENSOR_ID = '"S130C","S001","cal",1,0,49'

    def setUp(self):
        self.drivers = []

    def tearDown(self):
        for driver in self.drivers:
            try:
                driver.close()
            except BaseException:
                pass

    def ready_pm400(self):
        resource = FakeVisaResource(
            {
                "*IDN?": self._IDN,
                "SYSTem:SENSor:IDN?": self._SENSOR_ID,
            }
        )
        driver = PM400(
            f"USB0::0x1313::0x8078::core-{len(self.drivers)}::INSTR",
            resource_manager=FakeResourceManager(resource),
            resource_manager_factory=lambda: (_ for _ in ()).throw(AssertionError("unused")),
        ).connect()
        self.drivers.append(driver)
        return driver, resource

    @staticmethod
    def _headers(transactions):
        return {transaction.split(" ", 1)[0] for transaction in transactions}

    def test_public_facades_cover_every_root_system_status_display_and_calibration_command(self):
        driver, resource = self.ready_pm400()
        resource.queue("*ESE?", "255", "0")
        resource.queue("*ESR?", "1")
        resource.queue("*OPC?", "1")
        resource.queue("*SRE?", "255", "0")
        resource.queue("*STB?", "255")
        resource.queue("*TST?", "0")
        for group in StatusGroup:
            prefix = f"STATus:{group.value}"
            resource.queue(f"{prefix}:EVENt?", "0")
            resource.queue(f"{prefix}:CONDition?", "0")
            resource.queue(f"{prefix}:PTRansition?", "65535", "0")
            resource.queue(f"{prefix}:NTRansition?", "65535", "0")
            resource.queue(f"{prefix}:ENABle?", "65535", "0")
        resource.queue("SYSTem:BEEPer:STATe?", "1", "0")
        resource.queue("SYSTem:ERRor?", '-222,"quoted, system error"', '0,"No error"')
        resource.queue("SYSTem:VERSion?", "1999.0")
        resource.queue("SYSTem:DATE?", "2025,2,3", "2025,2,3")
        resource.queue("SYSTem:TIME?", "4,5,6", "4,5,6")
        resource.queue("SYSTem:LFRequency?", "50", "60")
        resource.queue("DISPlay:BRIGhtness?", "1.25", "2.5")
        resource.queue("DISPlay:CONTrast?", "3.5", "4.5")
        resource.queue("CALibration:STRing?", "calibration text")

        self.assertEqual(driver.identify().model, "PM400")
        self.assertEqual(driver.system.next_error().message, "quoted, system error")
        driver.clear_status()
        self.assertEqual(driver.set_standard_event_enable(255), 255)
        self.assertEqual(driver.get_standard_event_enable(), 0)
        self.assertEqual(driver.read_standard_event_status(), 1)
        driver.mark_operation_complete()
        driver.wait_operation_complete()
        driver.reset(confirm=True)
        self.assertEqual(driver.set_service_request_enable(255), 255)
        self.assertEqual(driver.get_service_request_enable(), 0)
        self.assertEqual(driver.read_status_byte(), 255)
        self.assertEqual(driver.self_test(), 0)
        driver.wait_to_continue()
        for group in StatusGroup:
            self.assertEqual(driver.status.read_event(group), 0)
            self.assertEqual(driver.status.read_condition(group), 0)
            self.assertEqual(driver.status.set_positive_transition(group, 65535), 65535)
            self.assertEqual(driver.status.get_positive_transition(group), 0)
            self.assertEqual(driver.status.set_negative_transition(group, 65535), 65535)
            self.assertEqual(driver.status.get_negative_transition(group), 0)
            self.assertEqual(driver.status.set_enable(group, 65535), 65535)
            self.assertEqual(driver.status.get_enable(group), 0)
        driver.status.preset(confirm=True)
        driver.system.beep()
        self.assertTrue(driver.system.set_beeper_enabled(True))
        self.assertFalse(driver.system.get_beeper_enabled())
        self.assertEqual(driver.system.drain_errors(), ())
        self.assertEqual(driver.system.get_scpi_version(), "1999.0")
        self.assertEqual(driver.system.set_date(datetime.date(2025, 2, 3)), datetime.date(2025, 2, 3))
        self.assertEqual(driver.system.get_date(), datetime.date(2025, 2, 3))
        self.assertEqual(driver.system.set_time(datetime.time(4, 5, 6)), datetime.time(4, 5, 6))
        self.assertEqual(driver.system.get_time(), datetime.time(4, 5, 6))
        self.assertEqual(driver.system.set_line_frequency_hz(50), 50)
        self.assertEqual(driver.system.get_line_frequency_hz(), 60)
        self.assertEqual(driver.system.get_sensor_info().name, "S130C")
        self.assertEqual(driver.display.set_brightness(1.25), 1.25)
        self.assertEqual(driver.display.get_brightness(), 2.5)
        self.assertEqual(driver.display.set_contrast(3.5), 3.5)
        self.assertEqual(driver.display.get_contrast(), 4.5)
        self.assertEqual(driver.calibration.get_string(), "calibration text")

        required = (
            PM400_ROOT_COMMANDS
            | PM400_SYSTEM_COMMANDS
            | PM400_STATUS_COMMANDS
            | PM400_DISPLAY_COMMANDS
            | PM400_CALIBRATION_COMMANDS
        )
        self.assertTrue(required.issubset(self._headers(resource.transactions)))

    def test_status_table_uses_exact_group_paths(self):
        driver, resource = self.ready_pm400()
        for group in StatusGroup:
            prefix = f"STATus:{group.value}"
            resource.queue(f"{prefix}:EVENt?", "1")
            resource.queue(f"{prefix}:CONDition?", "2")
            resource.queue(f"{prefix}:PTRansition?", "3", "4")
            resource.queue(f"{prefix}:NTRansition?", "5", "6")
            resource.queue(f"{prefix}:ENABle?", "7", "8")
            start = len(resource.transactions)
            self.assertEqual(driver.status.read_event(group), 1)
            self.assertEqual(driver.status.read_condition(group), 2)
            self.assertEqual(driver.status.set_positive_transition(group, 3), 3)
            self.assertEqual(driver.status.get_positive_transition(group), 4)
            self.assertEqual(driver.status.set_negative_transition(group, 5), 5)
            self.assertEqual(driver.status.get_negative_transition(group), 6)
            self.assertEqual(driver.status.set_enable(group, 7), 7)
            self.assertEqual(driver.status.get_enable(group), 8)
            self.assertEqual(resource.transactions[start:], [
                f"{prefix}:EVENt?", f"{prefix}:CONDition?",
                f"{prefix}:PTRansition 3", f"{prefix}:PTRansition?",
                f"{prefix}:PTRansition?", f"{prefix}:NTRansition 5",
                f"{prefix}:NTRansition?", f"{prefix}:NTRansition?",
                f"{prefix}:ENABle 7", f"{prefix}:ENABle?", f"{prefix}:ENABle?",
            ])

    def test_register_domains_are_enforced_before_io_and_on_readback(self):
        driver, resource = self.ready_pm400()
        before = list(resource.transactions)
        for value in (-1, 65536, True, 1.5):
            with self.subTest(value=value):
                with self.assertRaises(InstrumentSafetyError):
                    driver.status.set_enable(StatusGroup.MEASUREMENT, value)
        self.assertEqual(resource.transactions, before)
        resource.queue("STATus:MEASurement:ENABle?", "65536")
        with self.assertRaises(InstrumentProtocolError):
            driver.status.get_enable(StatusGroup.MEASUREMENT)
        for value in (-1, 256, True, 1.5):
            with self.subTest(root_value=value):
                with self.assertRaises(InstrumentSafetyError):
                    driver.set_standard_event_enable(value)

    def test_reset_and_status_preset_require_literal_confirmation_before_io(self):
        driver, resource = self.ready_pm400()
        before = list(resource.transactions)
        for confirm in (False, 1, "True", None):
            with self.subTest(operation="reset", confirm=confirm):
                with self.assertRaises(InstrumentSafetyError):
                    driver.reset(confirm=confirm)
            with self.subTest(operation="preset", confirm=confirm):
                with self.assertRaises(InstrumentSafetyError):
                    driver.status.preset(confirm=confirm)
        self.assertEqual(resource.transactions, before)
        driver.reset(confirm=True)
        driver.status.preset(confirm=True)

    def test_read_only_queries_do_not_require_confirmation(self):
        driver, resource = self.ready_pm400()
        resource.queue("*ESR?", "0")
        resource.queue("STATus:MEASurement:EVENt?", "0")
        self.assertEqual(driver.read_standard_event_status(), 0)
        self.assertEqual(driver.status.read_event(StatusGroup.MEASUREMENT), 0)

    def test_system_date_time_frequency_and_display_validation(self):
        driver, resource = self.ready_pm400()
        before = list(resource.transactions)
        for value in (datetime.datetime(2025, 1, 1), "2025-01-01", None):
            with self.subTest(date=value):
                with self.assertRaises(InstrumentSafetyError):
                    driver.system.set_date(value)
        for value in ("01:02:03", datetime.datetime(2025, 1, 1), None):
            with self.subTest(time=value):
                with self.assertRaises(InstrumentSafetyError):
                    driver.system.set_time(value)
        for value in (49, 51, 61, True, 50.0):
            with self.subTest(frequency=value):
                with self.assertRaises(InstrumentSafetyError):
                    driver.system.set_line_frequency_hz(value)
        self.assertEqual(resource.transactions, before)
        resource.queue("SYSTem:DATE?", "2025,2,30")
        with self.assertRaises(InstrumentProtocolError):
            driver.system.get_date()
        resource.queue("SYSTem:TIME?", "12,60,0")
        with self.assertRaises(InstrumentProtocolError):
            driver.system.get_time()
        resource.queue("DISPlay:BRIGhtness?", "123.5")
        self.assertEqual(driver.display.set_brightness(123.5), 123.5)


class PM400SenseInputTests(unittest.TestCase):
    _IDN = "THORLABS,PM400,P001,2.0.0"
    _ALL_CAPABILITIES = 371

    # Each literal row is: (public method metadata, exact set command,
    # exact query command, required capability, requires_confirm).
    COMMAND_MATRIX = (
        (("sense", "set_average_count", "get_average_count", 4, "4"), "SENSe:AVERage:COUNt 4", "SENSe:AVERage:COUNt?", None, False),
        (("sense", "set_loss_db", "get_loss_db", 5.0, "5.0"), "SENSe:CORRection:LOSS:INPut:MAGNitude 5.0", "SENSe:CORRection:LOSS:INPut:MAGNitude?", None, False),
        (("sense", "start_zero_collection", None, None, None), "SENSe:CORRection:COLLect:ZERO:INITiate", None, None, True),
        (("sense", "abort_zero_collection", None, None, None), "SENSe:CORRection:COLLect:ZERO:ABORt", None, None, False),
        (("sense", None, "get_zero_state", None, "1"), None, "SENSe:CORRection:COLLect:ZERO:STATe?", None, False),
        (("sense", None, "get_zero_magnitude", None, "5.0"), None, "SENSe:CORRection:COLLect:ZERO:MAGNitude?", None, False),
        (("sense", "set_beam_diameter_mm", "get_beam_diameter_mm", 5.0, "5.0"), "SENSe:CORRection:BEAMdiameter 5.0", "SENSe:CORRection:BEAMdiameter?", None, False),
        (("sense", "set_wavelength_nm", "get_wavelength_nm", 5.0, "5.0"), "SENSe:CORRection:WAVelength 5.0", "SENSe:CORRection:WAVelength?", "wavelength_settable", False),
        (("sense", "set_photodiode_response_a_per_w", "get_photodiode_response_a_per_w", 5.0, "5.0"), "SENSe:CORRection:POWer:PDIOde:RESPonse 5.0", "SENSe:CORRection:POWer:PDIOde:RESPonse?", ("response_settable", "power"), True),
        (("sense", "set_thermopile_response_v_per_w", "get_thermopile_response_v_per_w", 5.0, "5.0"), "SENSe:CORRection:POWer:THERmopile:RESPonse 5.0", "SENSe:CORRection:POWer:THERmopile:RESPonse?", ("response_settable", "power"), True),
        (("sense", "set_pyro_response_v_per_j", "get_pyro_response_v_per_j", 5.0, "5.0"), "SENSe:CORRection:ENERgy:PYRO:RESPonse 5.0", "SENSe:CORRection:ENERgy:PYRO:RESPonse?", ("response_settable", "energy"), True),
        (("sense", "set_current_auto_range", "get_current_auto_range", True, "1"), "SENSe:CURRent:DC:RANGe:AUTO 1", "SENSe:CURRent:DC:RANGe:AUTO?", "power", False),
        (("sense", "set_current_range_a", "get_current_range_a", 5.0, "5.0"), "SENSe:CURRent:DC:RANGe:UPPer 5.0", "SENSe:CURRent:DC:RANGe:UPPer?", "power", False),
        (("sense", "set_current_reference_a", "get_current_reference_a", 5.0, "5.0"), "SENSe:CURRent:DC:REFerence 5.0", "SENSe:CURRent:DC:REFerence?", "power", False),
        (("sense", "set_current_delta_enabled", "get_current_delta_enabled", True, "1"), "SENSe:CURRent:DC:REFerence:STATe 1", "SENSe:CURRent:DC:REFerence:STATe?", "power", False),
        (("sense", "set_energy_range_j", "get_energy_range_j", 5.0, "5.0"), "SENSe:ENERgy:RANGe:UPPer 5.0", "SENSe:ENERgy:RANGe:UPPer?", "energy", False),
        (("sense", "set_energy_reference_j", "get_energy_reference_j", 5.0, "5.0"), "SENSe:ENERgy:REFerence 5.0", "SENSe:ENERgy:REFerence?", "energy", False),
        (("sense", "set_energy_delta_enabled", "get_energy_delta_enabled", True, "1"), "SENSe:ENERgy:REFerence:STATe 1", "SENSe:ENERgy:REFerence:STATe?", "energy", False),
        (("sense", None, "get_frequency_upper_hz", None, "5.0"), None, "SENSe:FREQuency:RANGe:UPPer?", None, False),
        (("sense", None, "get_frequency_lower_hz", None, "5.0"), None, "SENSe:FREQuency:RANGe:LOWer?", None, False),
        (("sense", "set_power_auto_range", "get_power_auto_range", True, "1"), "SENSe:POWer:DC:RANGe:AUTO 1", "SENSe:POWer:DC:RANGe:AUTO?", "power", False),
        (("sense", "set_power_range_w", "get_power_range_w", 5.0, "5.0"), "SENSe:POWer:DC:RANGe:UPPer 5.0", "SENSe:POWer:DC:RANGe:UPPer?", "power", False),
        (("sense", "set_power_reference_w", "get_power_reference_w", 5.0, "5.0"), "SENSe:POWer:DC:REFerence 5.0", "SENSe:POWer:DC:REFerence?", "power", False),
        (("sense", "set_power_delta_enabled", "get_power_delta_enabled", True, "1"), "SENSe:POWer:DC:REFerence:STATe 1", "SENSe:POWer:DC:REFerence:STATe?", "power", False),
        (("sense", "set_power_unit", "get_power_unit", PowerUnit.WATTS, "W"), "SENSe:POWer:DC:UNIT W", "SENSe:POWer:DC:UNIT?", "power", False),
        (("sense", "set_voltage_auto_range", "get_voltage_auto_range", True, "1"), "SENSe:VOLTage:DC:RANGe:AUTO 1", "SENSe:VOLTage:DC:RANGe:AUTO?", "power", False),
        (("sense", "set_voltage_range_v", "get_voltage_range_v", 5.0, "5.0"), "SENSe:VOLTage:DC:RANGe:UPPer 5.0", "SENSe:VOLTage:DC:RANGe:UPPer?", "power", False),
        (("sense", "set_voltage_reference_v", "get_voltage_reference_v", 5.0, "5.0"), "SENSe:VOLTage:DC:REFerence 5.0", "SENSe:VOLTage:DC:REFerence?", "power", False),
        (("sense", "set_voltage_delta_enabled", "get_voltage_delta_enabled", True, "1"), "SENSe:VOLTage:DC:REFerence:STATe 1", "SENSe:VOLTage:DC:REFerence:STATe?", "power", False),
        (("sense", "set_peak_threshold_percent", "get_peak_threshold_percent", 5.0, "5.0"), "SENSe:PEAKdetector:THReshold 5.0", "SENSe:PEAKdetector:THReshold?", "energy", False),
        (("input", "set_photodiode_lowpass_enabled", "get_photodiode_lowpass_enabled", True, "1"), "INPut:PDIOde:FILTer:LPASs:STATe 1", "INPut:PDIOde:FILTer:LPASs:STATe?", "power", False),
        (("input", "set_thermopile_accelerator_enabled", "get_thermopile_accelerator_enabled", True, "1"), "INPut:THERmopile:ACCelerator:STATe 1", "INPut:THERmopile:ACCelerator:STATe?", "power", False),
        (("input", "set_thermopile_accelerator_auto", "get_thermopile_accelerator_auto", True, "1"), "INPut:THERmopile:ACCelerator:AUTO 1", "INPut:THERmopile:ACCelerator:AUTO?", "power", False),
        (("input", "set_thermopile_tau_s", "get_thermopile_tau_s", 5.0, "5.0"), "INPut:THERmopile:ACCelerator:TAU 5.0", "INPut:THERmopile:ACCelerator:TAU?", ("tau_settable", "power"), False),
        (("input", "set_adapter_type", "get_adapter_type", AdapterType.PYRO, "PYR"), "INPut:ADAPter:TYPE PYRo", "INPut:ADAPter:TYPE?", None, True),
    )

    NUMERIC_PROPERTIES = (
        ("sense", "set_loss_db", "get_loss_db", "SENSe:CORRection:LOSS:INPut:MAGNitude", (LimitSelector.MINIMUM, LimitSelector.MAXIMUM, LimitSelector.DEFAULT), False),
        ("sense", "set_beam_diameter_mm", "get_beam_diameter_mm", "SENSe:CORRection:BEAMdiameter", (LimitSelector.MINIMUM, LimitSelector.MAXIMUM, LimitSelector.DEFAULT), False),
        ("sense", "set_wavelength_nm", "get_wavelength_nm", "SENSe:CORRection:WAVelength", (LimitSelector.MINIMUM, LimitSelector.MAXIMUM), False),
        ("sense", "set_photodiode_response_a_per_w", "get_photodiode_response_a_per_w", "SENSe:CORRection:POWer:PDIOde:RESPonse", (LimitSelector.MINIMUM, LimitSelector.MAXIMUM, LimitSelector.DEFAULT), True),
        ("sense", "set_thermopile_response_v_per_w", "get_thermopile_response_v_per_w", "SENSe:CORRection:POWer:THERmopile:RESPonse", (LimitSelector.MINIMUM, LimitSelector.MAXIMUM, LimitSelector.DEFAULT), True),
        ("sense", "set_pyro_response_v_per_j", "get_pyro_response_v_per_j", "SENSe:CORRection:ENERgy:PYRO:RESPonse", (LimitSelector.MINIMUM, LimitSelector.MAXIMUM, LimitSelector.DEFAULT), True),
        ("sense", "set_current_range_a", "get_current_range_a", "SENSe:CURRent:DC:RANGe:UPPer", (LimitSelector.MINIMUM, LimitSelector.MAXIMUM), False),
        ("sense", "set_current_reference_a", "get_current_reference_a", "SENSe:CURRent:DC:REFerence", (LimitSelector.MINIMUM, LimitSelector.MAXIMUM, LimitSelector.DEFAULT), False),
        ("sense", "set_energy_range_j", "get_energy_range_j", "SENSe:ENERgy:RANGe:UPPer", (LimitSelector.MINIMUM, LimitSelector.MAXIMUM), False),
        ("sense", "set_energy_reference_j", "get_energy_reference_j", "SENSe:ENERgy:REFerence", (LimitSelector.MINIMUM, LimitSelector.MAXIMUM, LimitSelector.DEFAULT), False),
        ("sense", "set_power_range_w", "get_power_range_w", "SENSe:POWer:DC:RANGe:UPPer", (LimitSelector.MINIMUM, LimitSelector.MAXIMUM), False),
        ("sense", "set_power_reference_w", "get_power_reference_w", "SENSe:POWer:DC:REFerence", (LimitSelector.MINIMUM, LimitSelector.MAXIMUM, LimitSelector.DEFAULT), False),
        ("sense", "set_voltage_range_v", "get_voltage_range_v", "SENSe:VOLTage:DC:RANGe:UPPer", (LimitSelector.MINIMUM, LimitSelector.MAXIMUM), False),
        ("sense", "set_voltage_reference_v", "get_voltage_reference_v", "SENSe:VOLTage:DC:REFerence", (LimitSelector.MINIMUM, LimitSelector.MAXIMUM, LimitSelector.DEFAULT), False),
        ("sense", "set_peak_threshold_percent", "get_peak_threshold_percent", "SENSe:PEAKdetector:THReshold", (LimitSelector.MINIMUM, LimitSelector.MAXIMUM, LimitSelector.DEFAULT), False),
        ("input", "set_thermopile_tau_s", "get_thermopile_tau_s", "INPut:THERmopile:ACCelerator:TAU", (LimitSelector.MINIMUM, LimitSelector.MAXIMUM, LimitSelector.DEFAULT), False),
    )

    def setUp(self):
        self.drivers = []

    def tearDown(self):
        for driver in self.drivers:
            try:
                driver.close()
            except BaseException:
                pass

    def ready_pm400(self, *, flags=_ALL_CAPABILITIES):
        resource = FakeVisaResource({
            "*IDN?": self._IDN,
            "SYSTem:SENSor:IDN?": f'"S","SN","cal",1,0,{flags}',
        })
        driver = PM400(
            f"USB0::0x1313::0x8078::sense-input-{len(self.drivers)}::INSTR",
            resource_manager=FakeResourceManager(resource),
            resource_manager_factory=lambda: (_ for _ in ()).throw(AssertionError("unused")),
        ).connect()
        self.drivers.append(driver)
        return driver, resource

    @staticmethod
    def _queue_numeric_bounds(resource, header):
        resource.queue(f"{header}? MINimum", "0.0")
        resource.queue(f"{header}? MAXimum", "10.0")

    def test_full_command_matrix_uses_exact_scpi_leaves(self):
        driver, resource = self.ready_pm400()
        matrix_start = len(resource.transactions)
        numeric_headers = {row[3] for row in self.NUMERIC_PROPERTIES}
        for metadata, set_command, query_command, _capability, requires_confirm in self.COMMAND_MATRIX:
            facade_name, setter_name, getter_name, argument, response = metadata
            facade = getattr(driver, facade_name)
            start = len(resource.transactions)
            header = set_command.split(" ", 1)[0] if set_command else None
            if setter_name is not None:
                if header in numeric_headers:
                    self._queue_numeric_bounds(resource, header)
                if query_command is not None:
                    resource.queue(query_command, response)
                kwargs = {"confirm": True} if requires_confirm else {}
                getattr(facade, setter_name)(argument, **kwargs) if argument is not None else getattr(facade, setter_name)(**kwargs)
            if getter_name is not None:
                resource.queue(query_command, response)
                getattr(facade, getter_name)()
            observed = resource.transactions[start:]
            if set_command is not None:
                self.assertIn(set_command, observed, metadata)
            if query_command is not None:
                self.assertIn(query_command, observed, metadata)

        expected_headers = {
            command.split(" ", 1)[0]
            for row in self.COMMAND_MATRIX
            for command in row[1:3]
            if isinstance(command, str)
        }
        observed_headers = {
            command.split(" ", 1)[0]
            for command in resource.transactions[matrix_start:]
            if not command.startswith("SYSTem:ERRor?")
        }
        self.assertEqual(observed_headers, expected_headers)

    def test_every_numeric_property_supports_only_its_documented_selectors(self):
        driver, resource = self.ready_pm400()
        for facade_name, setter_name, getter_name, header, selectors, requires_confirm in self.NUMERIC_PROPERTIES:
            facade = getattr(driver, facade_name)
            for selector in selectors:
                with self.subTest(method=setter_name, selector=selector):
                    resource.queue(f"{header}? {selector.value}", "2.5", "2.5")
                    resource.queue(f"{header}?", "2.5")
                    start = len(resource.transactions)
                    kwargs = {"confirm": True} if requires_confirm else {}
                    self.assertEqual(getattr(facade, setter_name)(selector, **kwargs), 2.5)
                    self.assertEqual(getattr(facade, getter_name)(selector), 2.5)
                    self.assertEqual(resource.transactions[start:], [
                        f"{header}? {selector.value}",
                        f"{header} {selector.value}",
                        f"{header}?",
                        f"{header}? {selector.value}",
                    ])

    def test_unsupported_default_selector_is_rejected_before_io(self):
        driver, resource = self.ready_pm400()
        unsupported = (
            (driver.sense.set_wavelength_nm, driver.sense.get_wavelength_nm),
            (driver.sense.set_current_range_a, driver.sense.get_current_range_a),
            (driver.sense.set_energy_range_j, driver.sense.get_energy_range_j),
            (driver.sense.set_power_range_w, driver.sense.get_power_range_w),
            (driver.sense.set_voltage_range_v, driver.sense.get_voltage_range_v),
        )
        before = list(resource.transactions)
        for setter, getter in unsupported:
            with self.subTest(method=setter.__name__):
                with self.assertRaises(InstrumentSafetyError):
                    setter(LimitSelector.DEFAULT)
                with self.assertRaises(InstrumentSafetyError):
                    getter(LimitSelector.DEFAULT)
        self.assertEqual(resource.transactions, before)

    def test_capability_rejection_precedes_all_io_including_public_wavelength_path(self):
        capability_bits = {
            "power": 1,
            "energy": 2,
            "response_settable": 16,
            "wavelength_settable": 32,
            "tau_settable": 64,
        }
        for metadata, _set_command, _query_command, required, requires_confirm in self.COMMAND_MATRIX:
            if required is None:
                continue
            facade_name, setter_name, getter_name, argument, _response = metadata
            capabilities = (required,) if isinstance(required, str) else required
            for capability in capabilities:
                driver, resource = self.ready_pm400(
                    flags=self._ALL_CAPABILITIES & ~capability_bits[capability]
                )
                facade = getattr(driver, facade_name)
                method = getattr(facade, setter_name or getter_name)
                kwargs = {"confirm": True} if requires_confirm else {}
                before = list(resource.transactions)
                with self.subTest(method=method.__name__, capability=capability):
                    with self.assertRaises(InstrumentCapabilityError):
                        method(argument, **kwargs) if setter_name is not None else method()
                    self.assertEqual(resource.transactions, before)

    def test_dangerous_actions_require_literal_confirmation_before_io(self):
        driver, resource = self.ready_pm400()
        operations = (
            lambda confirm: driver.sense.start_zero_collection(confirm=confirm),
            lambda confirm: driver.sense.set_photodiode_response_a_per_w(5.0, confirm=confirm),
            lambda confirm: driver.sense.set_thermopile_response_v_per_w(5.0, confirm=confirm),
            lambda confirm: driver.sense.set_pyro_response_v_per_j(5.0, confirm=confirm),
            lambda confirm: driver.input.set_adapter_type(AdapterType.PHOTODIODE, confirm=confirm),
        )
        before = list(resource.transactions)
        for operation in operations:
            for confirm in (False, 1, "True", None):
                with self.subTest(operation=operation, confirm=confirm):
                    with self.assertRaises(InstrumentSafetyError):
                        operation(confirm)
        self.assertEqual(resource.transactions, before)

    def test_numeric_domains_are_validated_before_io(self):
        driver, resource = self.ready_pm400()
        before = list(resource.transactions)
        with self.assertRaises(InstrumentSafetyError):
            driver.sense.set_average_count(True)
        with self.assertRaises(InstrumentSafetyError):
            driver.sense.set_average_count(0)
        for facade_name, setter_name, _getter_name, _header, _selectors, requires_confirm in self.NUMERIC_PROPERTIES:
            setter = getattr(getattr(driver, facade_name), setter_name)
            for value in (math.nan, math.inf, -math.inf, True, "5"):
                kwargs = {"confirm": True} if requires_confirm else {}
                with self.subTest(method=setter_name, value=value):
                    with self.assertRaises(InstrumentSafetyError):
                        setter(value, **kwargs)
        self.assertEqual(resource.transactions, before)

    def test_device_min_max_intersection_rejects_before_numeric_write(self):
        driver, resource = self.ready_pm400()
        header = "SENSe:CORRection:WAVelength"
        resource.queue(f"{header}? MINimum", "400.0")
        resource.queue(f"{header}? MAXimum", "1100.0")
        with self.assertRaises(InstrumentSafetyError):
            driver.sense.set_wavelength_nm(1550.0)
        self.assertEqual(resource.transactions[-2:], [
            f"{header}? MINimum", f"{header}? MAXimum",
        ])
        self.assertNotIn(f"{header} 1550.0", resource.mutating_commands)

    def test_numeric_readback_uses_tolerance_but_rejects_material_mismatch(self):
        driver, resource = self.ready_pm400()
        header = "SENSe:CORRection:LOSS:INPut:MAGNitude"
        self._queue_numeric_bounds(resource, header)
        resource.queue(f"{header}?", "5.0000000005")
        self.assertAlmostEqual(driver.sense.set_loss_db(5.0), 5.0000000005)
        self._queue_numeric_bounds(resource, header)
        resource.queue(f"{header}?", "5.1")
        with self.assertRaises(DeviceFault):
            driver.sense.set_loss_db(5.0)

    def test_adapter_query_parses_scpi_short_tokens(self):
        driver, resource = self.ready_pm400()
        resource.queue("INPut:ADAPter:TYPE?", "PHOT", "THER", "PYR")
        self.assertEqual(driver.input.get_adapter_type(), AdapterType.PHOTODIODE)
        self.assertEqual(driver.input.get_adapter_type(), AdapterType.THERMAL)
        self.assertEqual(driver.input.get_adapter_type(), AdapterType.PYRO)


class PM400MeasurementTests(unittest.TestCase):
    _IDN = "THORLABS,PM400,P001,2.0.0"
    _ALL_CAPABILITIES = 371

    def setUp(self):
        self.drivers = []

    def tearDown(self):
        for driver in self.drivers:
            try:
                driver.close()
            except BaseException:
                pass

    def ready_pm400(self, *, flags=_ALL_CAPABILITIES, resource=None, driver_type=PM400):
        resource = resource or FakeVisaResource({})
        resource.replies.update(
            {
                "*IDN?": self._IDN,
                "SYSTem:SENSor:IDN?": f'"S130C","S001","cal",1,0,{flags}',
            }
        )
        driver = driver_type(
            f"USB0::0x1313::0x8078::measurement-{len(self.drivers)}::INSTR",
            resource_manager=FakeResourceManager(resource),
            resource_manager_factory=lambda: (_ for _ in ()).throw(AssertionError("unused")),
        ).connect()
        self.drivers.append(driver)
        return driver, resource

    def blocked_read(self, *, resync_fails=False, failures=None):
        resource = GatedVisaResource(
            {"*OPC?": "0" if resync_fails else "1"}, failures=failures
        )
        driver, resource = self.ready_pm400(resource=resource)
        future = ThreadCall(
            lambda: driver.measurement.read(MeasurementKind.ENERGY, timeout=5.0),
            name="PM400MeasurementBlockedRead",
        )
        self.assertTrue(resource.read_blocked.wait(1.0))
        self.assertEqual(driver.state, DriverState.ACTIVE)
        return driver, resource, future

    def test_all_measurement_kinds_use_exact_configure_and_measure_commands(self):
        driver, resource = self.ready_pm400()
        cases = (
            (MeasurementKind.POWER, "CONFigure:SCALar:POWer", "MEASure:SCALar:POWer?"),
            (MeasurementKind.CURRENT, "CONFigure:SCALar:CURRent:DC", "MEASure:SCALar:CURRent:DC?"),
            (MeasurementKind.VOLTAGE, "CONFigure:SCALar:VOLTage:DC", "MEASure:SCALar:VOLTage:DC?"),
            (MeasurementKind.ENERGY, "CONFigure:SCALar:ENERgy", "MEASure:SCALar:ENERgy?"),
            (MeasurementKind.FREQUENCY, "CONFigure:SCALar:FREQuency", "MEASure:SCALar:FREQuency?"),
            (MeasurementKind.POWER_DENSITY, "CONFigure:SCALar:PDENsity", "MEASure:SCALar:PDENsity?"),
            (MeasurementKind.ENERGY_DENSITY, "CONFigure:SCALar:EDENsity", "MEASure:SCALar:EDENsity?"),
            (MeasurementKind.RESISTANCE, "CONFigure:SCALar:RESistance", "MEASure:SCALar:RESistance?"),
            (MeasurementKind.TEMPERATURE, "CONFigure:SCALar:TEMPerature", "MEASure:SCALar:TEMPerature?"),
        )
        for kind, configure, measure in cases:
            with self.subTest(kind=kind):
                resource.queue(measure, "1.25")
                if kind is MeasurementKind.POWER:
                    resource.queue("SENSe:POWer:DC:UNIT?", "W")
                self.assertIs(driver.measurement.configure(kind), kind)
                result = driver.measurement.measure(kind)
                self.assertIs(result.kind, kind)
                self.assertEqual(result.value, 1.25)
                self.assertIn(configure, resource.mutating_commands)
                self.assertIn(measure, resource.transactions)

    def test_measurement_facade_uses_exact_common_commands(self):
        driver, resource = self.ready_pm400()
        resource.queue("CONFigure?", "ENERgy", "ENERgy", "ENERgy")
        resource.queue("FETCh?", "2.5")
        resource.queue("READ?", "3.5")
        driver.measurement.initiate()
        driver.measurement.abort()
        self.assertIs(driver.measurement.get_configuration(), MeasurementKind.ENERGY)
        self.assertEqual(driver.measurement.fetch().value, 2.5)
        self.assertEqual(driver.measurement.read(timeout=0.5).value, 3.5)
        self.assertTrue(
            {"INITiate:IMMediate", "ABORt"}.issubset(resource.mutating_commands)
        )
        self.assertTrue(
            {"FETCh?", "READ?", "CONFigure?"}.issubset(resource.transactions)
        )

    def test_get_configuration_parses_every_configured_kind(self):
        driver, resource = self.ready_pm400()
        for kind in MeasurementKind:
            with self.subTest(kind=kind):
                resource.queue("CONFigure?", kind.scpi_suffix)
                self.assertIs(driver.measurement.get_configuration(), kind)

    def test_get_configuration_parses_literal_scpi_short_tokens(self):
        driver, resource = self.ready_pm400()
        cases = (
            ("POW", MeasurementKind.POWER),
            ("CURR:DC", MeasurementKind.CURRENT),
            ("VOLT:DC", MeasurementKind.VOLTAGE),
            ("ENER", MeasurementKind.ENERGY),
            ("FREQ", MeasurementKind.FREQUENCY),
            ("PDEN", MeasurementKind.POWER_DENSITY),
            ("EDEN", MeasurementKind.ENERGY_DENSITY),
            ("RES", MeasurementKind.RESISTANCE),
            ("TEMP", MeasurementKind.TEMPERATURE),
        )
        for response, expected in cases:
            with self.subTest(response=response):
                resource.queue("CONFigure?", response)
                self.assertIs(driver.measurement.get_configuration(), expected)

    def test_all_convenience_methods_return_the_requested_kind(self):
        driver, resource = self.ready_pm400()
        cases = (
            ("measure_power", MeasurementKind.POWER),
            ("measure_current", MeasurementKind.CURRENT),
            ("measure_voltage", MeasurementKind.VOLTAGE),
            ("measure_energy", MeasurementKind.ENERGY),
            ("measure_frequency", MeasurementKind.FREQUENCY),
            ("measure_power_density", MeasurementKind.POWER_DENSITY),
            ("measure_energy_density", MeasurementKind.ENERGY_DENSITY),
            ("measure_resistance", MeasurementKind.RESISTANCE),
            ("measure_temperature", MeasurementKind.TEMPERATURE),
        )
        for method_name, kind in cases:
            with self.subTest(method_name=method_name):
                command = f"MEASure:SCALar:{kind.scpi_suffix}?"
                resource.queue(command, "4.5")
                if kind is MeasurementKind.POWER:
                    resource.queue("SENSe:POWer:DC:UNIT?", "W")
                result = getattr(driver, method_name)(timeout=0.5)
                self.assertIs(result.kind, kind)

    def test_power_measurement_uses_current_dbm_unit(self):
        driver, resource = self.ready_pm400()
        resource.queue("MEASure:SCALar:POWer?", "-12.25")
        resource.queue("SENSe:POWer:DC:UNIT?", "DBM")
        result = driver.measure_power()
        self.assertEqual(result.unit, "DBM")

    def test_unsupported_measurement_capability_is_rejected_before_io(self):
        driver, resource = self.ready_pm400(flags=0)
        cases = (
            lambda: driver.measurement.configure(MeasurementKind.POWER),
            lambda: driver.measurement.measure(MeasurementKind.POWER_DENSITY),
            lambda: driver.measurement.read(MeasurementKind.ENERGY),
            lambda: driver.measurement.fetch(MeasurementKind.ENERGY_DENSITY),
            lambda: driver.measure_temperature(),
        )
        for operation in cases:
            with self.subTest(operation=operation):
                before = list(resource.transactions)
                with self.assertRaises(InstrumentCapabilityError):
                    operation()
                self.assertEqual(resource.transactions, before)

    def test_invalid_measurement_kind_is_rejected_before_io(self):
        driver, resource = self.ready_pm400()
        before = list(resource.transactions)
        with self.assertRaises(InstrumentSafetyError):
            driver.measurement.measure("POWER")
        self.assertEqual(resource.transactions, before)

    def test_nonfinite_measurement_response_is_rejected(self):
        driver, resource = self.ready_pm400()
        resource.queue("MEASure:SCALar:ENERgy?", "nan")
        with self.assertRaises(InstrumentProtocolError):
            driver.measure_energy()
        self.assertEqual(driver.state, DriverState.READY)

    def test_cancel_interrupts_io_aborts_after_release_and_returns_ready(self):
        driver, resource, future = self.blocked_read()
        driver.cancel_measurement()
        with self.assertRaises(InstrumentTimeoutError):
            future.result()
        self.assertIn(("clear",), resource.events)
        self.assertEqual(resource.transactions[-3:], ["READ?", "ABORt", "*OPC?"])
        self.assertEqual(resource.pending_reads, [])
        self.assertEqual(driver.state, DriverState.READY)

    def test_cancel_after_active_before_first_io_prevents_the_visa_call(self):
        driver, resource = self.ready_pm400(driver_type=GatedPM400)
        resource.queue("MEASure:SCALar:ENERgy?", "1.0")
        resource.queue("*OPC?", "1")
        driver.gate_before_first_io = True
        measurement = ThreadCall(driver.measure_energy, name="PM400PreIoCancel")
        self.assertTrue(driver.first_io_gate_entered.wait(1.0))
        self.assertEqual(driver.state, DriverState.ACTIVE)
        driver.cancel_measurement()
        driver.release_first_io_gate.set()
        with self.assertRaises(InstrumentTimeoutError):
            measurement.result()
        self.assertNotIn("MEASure:SCALar:ENERgy?", resource.transactions)
        self.assertEqual(driver.state, DriverState.READY)

    def test_close_after_active_before_first_io_prevents_the_visa_call(self):
        driver, resource = self.ready_pm400(driver_type=GatedPM400)
        resource.queue("MEASure:SCALar:ENERgy?", "1.0")
        resource.queue("*OPC?", "1")
        driver.gate_before_first_io = True
        measurement = ThreadCall(driver.measure_energy, name="PM400PreIoCloseMeasure")
        self.assertTrue(driver.first_io_gate_entered.wait(1.0))
        closing = ThreadCall(driver.close, name="PM400PreIoClose")
        with driver._lifecycle_condition:
            self.assertTrue(
                driver._lifecycle_condition.wait_for(
                    lambda: driver.state is DriverState.CLOSING, timeout=1.0
                )
            )
        driver.release_first_io_gate.set()
        with self.assertRaises(InstrumentTimeoutError):
            measurement.result()
        self.assertIsNone(closing.result())
        self.assertNotIn("MEASure:SCALar:ENERgy?", resource.transactions)
        self.assertEqual(driver.state, DriverState.DISCONNECTED)

    def test_cancel_between_power_unit_and_measure_prevents_final_visa_call(self):
        driver, resource = self.ready_pm400(driver_type=GatedPM400)
        resource.queue("SENSe:POWer:DC:UNIT?", "W")
        resource.queue("MEASure:SCALar:POWer?", "1.0")
        resource.queue("*OPC?", "1")
        driver.gate_after_command = "SENSe:POWer:DC:UNIT?"
        measurement = ThreadCall(driver.measure_power, name="PM400BetweenIoCancel")
        self.assertTrue(driver.between_io_gate_entered.wait(1.0))
        driver.cancel_measurement()
        driver.release_between_io_gate.set()
        with self.assertRaises(InstrumentTimeoutError):
            measurement.result()
        self.assertNotIn("MEASure:SCALar:POWer?", resource.transactions)
        self.assertEqual(driver.state, DriverState.READY)

    def test_close_between_power_unit_and_measure_prevents_final_visa_call(self):
        driver, resource = self.ready_pm400(driver_type=GatedPM400)
        resource.queue("SENSe:POWer:DC:UNIT?", "W")
        resource.queue("MEASure:SCALar:POWer?", "1.0")
        resource.queue("*OPC?", "1")
        driver.gate_after_command = "SENSe:POWer:DC:UNIT?"
        measurement = ThreadCall(driver.measure_power, name="PM400BetweenIoCloseMeasure")
        self.assertTrue(driver.between_io_gate_entered.wait(1.0))
        closing = ThreadCall(driver.close, name="PM400BetweenIoClose")
        with driver._lifecycle_condition:
            self.assertTrue(
                driver._lifecycle_condition.wait_for(
                    lambda: driver.state is DriverState.CLOSING, timeout=1.0
                )
            )
        driver.release_between_io_gate.set()
        with self.assertRaises(InstrumentTimeoutError):
            measurement.result()
        self.assertIsNone(closing.result())
        self.assertNotIn("MEASure:SCALar:POWer?", resource.transactions)
        self.assertEqual(driver.state, DriverState.DISCONNECTED)

    def test_repeated_interrupt_closes_the_arm_to_blocking_read_race(self):
        resource = LostInterruptVisaResource({"*OPC?": "1"})
        driver, resource = self.ready_pm400(resource=resource)
        measurement = ThreadCall(
            lambda: driver.measurement.read(MeasurementKind.ENERGY, timeout=5.0),
            name="PM400LostInterruptMeasure",
        )
        self.assertTrue(resource.query_invoked.wait(1.0))
        cancelling = ThreadCall(driver.cancel_measurement, name="PM400LostInterruptCancel")
        self.assertTrue(resource.first_early_clear.wait(1.0))
        resource.allow_blocking_read.set()
        self.assertIsNone(cancelling.result())
        with self.assertRaises(InstrumentTimeoutError):
            measurement.result()
        self.assertGreaterEqual(resource.clear_calls, 2)
        self.assertEqual(driver.state, DriverState.READY)

    def test_failed_repeated_interrupt_uses_terminal_close_without_deadlock(self):
        resource = GatedVisaResource(
            {"*OPC?": "1"}, failures={"clear": OSError("clear failed")}
        )
        driver, resource = self.ready_pm400(resource=resource)
        measurement = ThreadCall(
            lambda: driver.measurement.read(MeasurementKind.ENERGY, timeout=5.0),
            name="PM400FailedInterruptMeasure",
        )
        self.assertTrue(resource.read_blocked.wait(1.0))
        closing = ThreadCall(driver.close, name="PM400FailedInterruptClose")
        self.assertIsNone(closing.result(timeout=2.0))
        with self.assertRaises(InstrumentTimeoutError):
            measurement.result()
        self.assertTrue(resource.close_attempted)
        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertIsNotNone(driver.cleanup_error)

    def test_blocking_clear_is_bounded_and_reaped_before_disconnect(self):
        resource = BlockingClearVisaResource({"*OPC?": "1"})
        driver, resource = self.ready_pm400(resource=resource)
        measurement = ThreadCall(
            lambda: driver.measurement.read(MeasurementKind.ENERGY, timeout=5.0),
            name="PM400BlockingClearMeasure",
        )
        self.assertTrue(resource.read_blocked.wait(1.0))
        cancelling = ThreadCall(driver.cancel_measurement, name="PM400BlockingClearCancel")
        self.assertTrue(resource.clear_started.wait(1.0))
        try:
            self.assertIsNone(cancelling.result(timeout=0.25))
            self.assertEqual(driver.state, DriverState.FAULT)
            self.assertTrue(
                any(
                    thread.name.startswith("PM400Transport")
                    for thread in threading.enumerate()
                )
            )
        finally:
            resource.allow_clear.set()
            cancelling.thread.join(1.0)
            measurement.thread.join(1.0)
        driver.close()
        self.assertEqual(driver.state, DriverState.DISCONNECTED)
        self.assertEqual(
            [
                thread.name
                for thread in threading.enumerate()
                if thread.name.startswith("PM400Transport")
            ],
            [],
        )

    def test_blocking_terminal_close_faults_boundedly_and_reaps_later(self):
        resource = BlockingCloseVisaResource({"*OPC?": "0"})
        driver, resource = self.ready_pm400(resource=resource)
        measurement = ThreadCall(
            lambda: driver.measurement.read(MeasurementKind.ENERGY, timeout=5.0),
            name="PM400BlockingCloseMeasure",
        )
        self.assertTrue(resource.read_blocked.wait(1.0))
        driver.cancel_measurement()
        self.assertTrue(resource.close_started.wait(1.0))
        closing = None
        try:
            with self.assertRaises(DeviceFault):
                measurement.result(timeout=0.25)
            closing = ThreadCall(driver.close, name="PM400BlockingClosePublic")
            with self.assertRaises(DeviceFault):
                closing.result(timeout=0.25)
            self.assertEqual(driver.state, DriverState.FAULT)
            self.assertIsNotNone(driver.cleanup_error)
        finally:
            resource.allow_close.set()
            measurement.thread.join(1.0)
            if closing is not None:
                closing.thread.join(1.0)
        driver.close()
        self.assertEqual(driver.state, DriverState.DISCONNECTED)
        self.assertEqual(
            [
                thread.name
                for thread in threading.enumerate()
                if thread.name.startswith("PM400Transport")
            ],
            [],
        )

    def test_failed_resync_bounds_owned_manager_close_and_reaps_later(self):
        resource = GatedVisaResource({"*OPC?": "0"})
        resource.replies.update(
            {
                "*IDN?": self._IDN,
                "SYSTem:SENSor:IDN?": (
                    f'"S130C","S001","cal",1,0,{self._ALL_CAPABILITIES}'
                ),
            }
        )
        manager = BlockingResourceManager(resource)
        driver = PM400(
            "USB0::0x1313::0x8078::measurement-blocking-manager::INSTR",
            resource_manager_factory=lambda: manager,
        ).connect()
        self.drivers.append(driver)
        measurement = ThreadCall(
            lambda: driver.measurement.read(MeasurementKind.ENERGY, timeout=5.0),
            name="PM400BlockingManagerMeasure",
        )
        self.assertTrue(resource.read_blocked.wait(1.0))
        driver.cancel_measurement()
        self.assertTrue(manager.close_started.wait(1.0))
        try:
            with self.assertRaises(DeviceFault):
                measurement.result(timeout=0.25)
            self.assertEqual(driver.state, DriverState.FAULT)
            self.assertIsNotNone(driver.cleanup_error)
            self.assertIs(driver._manager, manager)
            self.assertTrue(driver._manager_owned)
        finally:
            manager.allow_close.set()
            measurement.thread.join(1.0)
        driver.close()
        self.assertEqual(driver.state, DriverState.DISCONNECTED)
        self.assertIsNone(driver._manager)
        self.assertEqual(manager.close_calls, 1)

    def test_blocked_clear_retains_resource_registry_and_owned_manager(self):
        resource_name = "USB0::0x1313::0x8078::measurement-blocked-clear-owner::INSTR"
        resource = BlockingClearVisaResource({"*OPC?": "1"})
        resource.replies.update(
            {
                "*IDN?": self._IDN,
                "SYSTem:SENSor:IDN?": (
                    f'"S130C","S001","cal",1,0,{self._ALL_CAPABILITIES}'
                ),
            }
        )
        manager = FakeResourceManager(resource)
        driver = PM400(
            resource_name,
            resource_manager_factory=lambda: manager,
        ).connect()
        self.drivers.append(driver)
        measurement = ThreadCall(
            lambda: driver.measurement.read(MeasurementKind.ENERGY, timeout=5.0),
            name="PM400SiblingOwnerMeasure",
        )
        self.assertTrue(resource.read_blocked.wait(1.0))
        cancelling = ThreadCall(
            driver.cancel_measurement, name="PM400SiblingOwnerCancel"
        )
        self.assertTrue(resource.clear_started.wait(1.0))
        self.assertIsNone(cancelling.result(timeout=0.25))

        replacement_resource = FakeVisaResource(
            {
                "*IDN?": self._IDN,
                "SYSTem:SENSor:IDN?": (
                    f'"S130C","S001","cal",1,0,{self._ALL_CAPABILITIES}'
                ),
            }
        )
        replacement = PM400(
            f" {resource_name} ",
            resource_manager=FakeResourceManager(replacement_resource),
        )
        self.drivers.append(replacement)
        try:
            self.assertIs(driver._resource, resource)
            self.assertIs(driver._manager, manager)
            self.assertTrue(driver._manager_owned)
            self.assertEqual(manager.close_calls, 0)
            with self.assertRaises(InstrumentConnectionError):
                replacement.connect()
        finally:
            resource.allow_clear.set()
            measurement.thread.join(1.0)

        driver.close()
        self.assertEqual(driver.state, DriverState.DISCONNECTED)
        self.assertEqual(manager.close_calls, 1)
        replacement.connect()
        replacement.close()

    def test_failed_resync_faults_and_closes_session(self):
        driver, resource, future = self.blocked_read(resync_fails=True)
        driver.cancel_measurement()
        with self.assertRaises(DeviceFault):
            future.result()
        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertTrue(resource.close_attempted)
        self.assertTrue(resource.closed)

    def test_concurrent_close_cannot_erase_a_resynchronization_fault(self):
        resource = GatedCloseVisaResource({"*OPC?": "0"})
        driver, resource = self.ready_pm400(resource=resource)
        measurement = ThreadCall(
            lambda: driver.measurement.read(MeasurementKind.ENERGY, timeout=5.0),
            name="PM400FaultCloseMeasure",
        )
        self.assertTrue(resource.read_blocked.wait(1.0))
        closing = ThreadCall(driver.close, name="PM400FaultCloseConcurrent")
        self.assertTrue(resource.close_started.wait(1.0))
        self.assertEqual(driver.state, DriverState.FAULT)
        fault_evidence = driver.cleanup_error
        self.assertIsNotNone(fault_evidence)
        resource.allow_close.set()
        with self.assertRaises(DeviceFault):
            measurement.result()
        self.assertIsNone(closing.result())
        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertIs(driver.cleanup_error, fault_evidence)

    def test_concurrent_close_failure_cannot_replace_resynchronization_evidence(self):
        resource = GatedCloseVisaResource(
            {"*OPC?": "0"}, failures={"close": OSError("close cleanup failed")}
        )
        driver, resource = self.ready_pm400(resource=resource)
        measurement = ThreadCall(
            lambda: driver.measurement.read(MeasurementKind.ENERGY, timeout=5.0),
            name="PM400FaultEvidenceMeasure",
        )
        self.assertTrue(resource.read_blocked.wait(1.0))
        closing = ThreadCall(driver.close, name="PM400FaultEvidenceClose")
        self.assertTrue(resource.close_started.wait(1.0))
        fault_evidence = driver.cleanup_error
        self.assertIsNotNone(fault_evidence)
        resource.allow_close.set()
        with self.assertRaises(DeviceFault):
            measurement.result()
        with self.assertRaisesRegex(OSError, "close cleanup failed"):
            closing.result()
        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertIs(driver.cleanup_error, fault_evidence)
        resource.failures.pop("close")

    def test_blocked_fault_close_preserves_first_evidence_through_concurrent_close(self):
        resource = BlockingCloseVisaResource({"*OPC?": "0"})
        driver, resource = self.ready_pm400(resource=resource)
        measurement = ThreadCall(
            lambda: driver.measurement.read(MeasurementKind.ENERGY, timeout=5.0),
            name="PM400FirstEvidenceMeasure",
        )
        self.assertTrue(resource.read_blocked.wait(1.0))
        closing = ThreadCall(driver.close, name="PM400FirstEvidenceClose")
        self.assertTrue(resource.close_started.wait(1.0))
        first_evidence = driver.cleanup_error
        self.assertIsInstance(first_evidence, InstrumentProtocolError)
        try:
            with self.assertRaises(DeviceFault):
                closing.result(timeout=0.25)
            with self.assertRaises(DeviceFault):
                measurement.result(timeout=0.25)
            self.assertEqual(driver.state, DriverState.FAULT)
            self.assertIs(driver.cleanup_error, first_evidence)
        finally:
            resource.allow_close.set()
            closing.thread.join(1.0)
            measurement.thread.join(1.0)
        driver.close()

    def test_resync_sentinel_surplus_faults_and_closes_session(self):
        driver, resource, future = self.blocked_read()
        resource.queue_pending_read("surplus after sentinel")
        driver.cancel_measurement()
        with self.assertRaises(DeviceFault):
            future.result()
        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertTrue(resource.closed)

    def test_caller_timeout_is_preserved_after_successful_resync(self):
        resource = FakeVisaResource(
            {"*OPC?": "1"},
            failures={("query", "MEASure:SCALar:ENERgy?"): VisaIOError(StatusCode.error_timeout)},
        )
        driver, resource = self.ready_pm400(resource=resource)
        with self.assertRaises(InstrumentTimeoutError):
            driver.measure_energy(timeout=0.25)
        self.assertEqual(resource.transactions[-2:], ["ABORt", "*OPC?"])
        self.assertEqual(driver.state, DriverState.READY)

    def test_keyboard_interrupt_is_preserved_after_successful_resync(self):
        resource = FakeVisaResource(
            {"*OPC?": "1"},
            failures={("query", "MEASure:SCALar:ENERgy?"): KeyboardInterrupt("stop")},
        )
        driver, resource = self.ready_pm400(resource=resource)
        with self.assertRaisesRegex(KeyboardInterrupt, "stop"):
            driver.measure_energy()
        self.assertEqual(resource.transactions[-2:], ["ABORt", "*OPC?"])
        self.assertEqual(driver.state, DriverState.READY)

    def test_close_during_active_measurement_interrupts_then_closes_after_request(self):
        driver, resource, measurement = self.blocked_read()
        closing = ThreadCall(driver.close, name="PM400MeasurementClose")
        with self.assertRaises(InstrumentTimeoutError):
            measurement.result()
        self.assertIsNone(closing.result())
        self.assertFalse(resource.closed_while_query_blocked)
        self.assertTrue(resource.closed)
        self.assertEqual(driver.state, DriverState.DISCONNECTED)

    def test_close_waits_for_an_inflight_nonmeasurement_query_transaction(self):
        resource = GatedVisaResource(
            {}, blocked_command="OTHER?", blocked_result="answer"
        )
        driver, resource = self.ready_pm400(resource=resource)
        query = ThreadCall(
            lambda: driver._query_text("OTHER?"), name="PM400TransactionBlockedQuery"
        )
        self.assertTrue(resource.read_blocked.wait(1.0))
        closing = ThreadCall(driver.close, name="PM400TransactionClose")
        with driver._lifecycle_condition:
            self.assertTrue(
                driver._lifecycle_condition.wait_for(
                    lambda: driver.state is DriverState.CLOSING, timeout=1.0
                )
            )
        self.assertFalse(resource.close_attempted)
        resource.query_released.set()
        self.assertEqual(query.result(), "answer")
        self.assertIsNone(closing.result())
        self.assertFalse(resource.closed_while_query_blocked)

    def test_cancel_generation_expires_queued_pre_cancel_measurement(self):
        driver, resource, active = self.blocked_read()
        queued = ThreadCall(driver.measure_energy, name="PM400MeasurementQueued")
        with driver._lifecycle_condition:
            self.assertTrue(
                driver._lifecycle_condition.wait_for(
                    lambda: driver._measurement_waiters >= 1, timeout=1.0
                )
            )
        driver.cancel_measurement()
        with self.assertRaises(InstrumentTimeoutError):
            active.result()
        with self.assertRaises(InstrumentTimeoutError):
            queued.result()
        self.assertEqual(resource.transactions.count("MEASure:SCALar:ENERgy?"), 0)

    def test_surplus_response_faults_instead_of_returning_ready(self):
        driver, resource = self.ready_pm400()
        resource.queue("MEASure:SCALar:ENERgy?", "1.0")
        resource.queue_pending_read("surplus")
        with self.assertRaises(DeviceFault):
            driver.measure_energy()
        self.assertEqual(driver.state, DriverState.FAULT)

    def test_primary_keyboard_interrupt_survives_abort_and_close_failures(self):
        resource = FakeVisaResource(
            {"*OPC?": "1"},
            failures={
                ("query", "MEASure:SCALar:ENERgy?"): KeyboardInterrupt("primary"),
                ("write", "ABORt"): OSError("abort cleanup failed"),
                "close": OSError("close cleanup failed"),
            },
        )
        driver, resource = self.ready_pm400(resource=resource)
        try:
            with self.assertRaisesRegex(KeyboardInterrupt, "primary"):
                driver.measure_energy()
            self.assertEqual(driver.state, DriverState.FAULT)
            self.assertTrue(resource.close_calls)
            self.assertIsNotNone(driver.cleanup_error)
        finally:
            resource.failures.pop("close")
            driver.close()

    def test_primary_keyboard_interrupt_survives_timeout_restoration_failure(self):
        resource = FakeVisaResource(
            {"*OPC?": "1"},
            failures={("query", "MEASure:SCALar:ENERgy?"): KeyboardInterrupt("primary")},
        )
        driver, resource = self.ready_pm400(resource=resource)
        resource.failures[("timeout", resource.timeout)] = OSError(
            "timeout restoration cleanup failed"
        )
        with self.assertRaisesRegex(KeyboardInterrupt, "primary"):
            driver.measure_energy(timeout=0.25)
        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertIsNotNone(driver.cleanup_error)
        self.assertEqual(resource.timeout, 250)

    def test_canonical_timeout_retry_succeeds_before_returning_ready(self):
        resource = FakeVisaResource(
            {"*OPC?": "1"},
            failures={("query", "MEASure:SCALar:ENERgy?"): KeyboardInterrupt("primary")},
        )
        driver, resource = self.ready_pm400(resource=resource)
        attempts = 0

        def fail_first_restoration_only():
            nonlocal attempts
            attempts += 1
            if attempts == 1:
                raise OSError("first timeout restoration failed")

        resource.failures[("timeout", resource.timeout)] = fail_first_restoration_only
        with self.assertRaisesRegex(KeyboardInterrupt, "primary"):
            driver.measure_energy(timeout=0.25)
        self.assertEqual(attempts, 2)
        self.assertEqual(resource.timeout, 5000)
        self.assertEqual(driver.state, DriverState.READY)
        self.assertIsNone(driver.cleanup_error)

    def test_tail_probe_timeout_restoration_failure_cannot_return_ready(self):
        driver, resource = self.ready_pm400()
        resource.queue("MEASure:SCALar:ENERgy?", "bad\nresponse")
        resource.queue("*OPC?", "1")
        resource.failures[("timeout", resource.timeout)] = OSError(
            "tail timeout restoration failed"
        )
        with self.assertRaises(InstrumentProtocolError):
            driver.measure_energy()
        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertIsNotNone(driver.cleanup_error)
        self.assertEqual(resource.timeout, 1)

    def test_fault_cleanup_clears_active_metadata_before_same_thread_reconnect(self):
        resource = FakeVisaResource(
            {"*OPC?": "0"},
            failures={
                ("query", "MEASure:SCALar:ENERgy?"): VisaIOError(
                    StatusCode.error_timeout
                )
            },
        )
        driver, resource = self.ready_pm400(resource=resource)
        with self.assertRaises(InstrumentTimeoutError):
            driver.measure_energy(timeout=0.25)
        self.assertEqual(driver.state, DriverState.FAULT)
        driver.close()
        self.assertEqual(driver.state, DriverState.DISCONNECTED)
        resource.failures.pop(("query", "MEASure:SCALar:ENERgy?"))
        driver.connect()
        self.assertEqual(driver.state, DriverState.READY)
        self.assertEqual(resource.transactions[-2:], ["*IDN?", "SYSTem:SENSor:IDN?"])


class PM400TransactionTests(unittest.TestCase):
    _IDN = "THORLABS,PM400,P001,2.0.0"

    def setUp(self):
        self.drivers = []

    def tearDown(self):
        for driver in self.drivers:
            try:
                driver.close()
            except BaseException:
                pass

    def ready_pm400(self, *, flags=49):
        resource = FakeVisaResource(
            {
                "*IDN?": self._IDN,
                "SYSTem:SENSor:IDN?": f'"S130C","S001","cal",1,0,{flags}',
            }
        )
        driver = PM400(
            f"USB0::0x1313::0x8078::transaction-{len(self.drivers)}::INSTR",
            resource_manager=FakeResourceManager(resource),
            resource_manager_factory=lambda: (_ for _ in ()).throw(AssertionError("unused")),
        ).connect()
        self.drivers.append(driver)
        return driver, resource

    def test_query_text_requires_one_nonempty_response_line(self):
        driver, resource = self.ready_pm400()
        resource.queue("BAD?", "one\ntwo")
        with self.assertRaises(InstrumentProtocolError):
            driver._query_text("BAD?")

    def test_query_integer_enforces_register_domain_before_and_after_io(self):
        driver, resource = self.ready_pm400()
        before = list(resource.transactions)
        with self.assertRaises(InstrumentSafetyError):
            driver._query_int("REG?", minimum=255, maximum=0)
        self.assertEqual(resource.transactions, before)
        resource.queue("REG?", "255", "256")
        self.assertEqual(driver._query_int("REG?", minimum=0, maximum=255), 255)
        with self.assertRaises(InstrumentProtocolError):
            driver._query_int("REG?", minimum=0, maximum=255)

    def test_query_boolean_accepts_only_zero_or_one(self):
        driver, resource = self.ready_pm400()
        resource.queue("BOOL?", "1", "0", "true")
        self.assertTrue(driver._query_bool("BOOL?"))
        self.assertFalse(driver._query_bool("BOOL?"))
        with self.assertRaises(InstrumentProtocolError):
            driver._query_bool("BOOL?")

    def test_query_boolean_rejects_surrounding_whitespace(self):
        driver, resource = self.ready_pm400()
        resource.queue("BOOL?", " 1", "1 ", "\t0")
        for _ in range(3):
            with self.assertRaises(InstrumentProtocolError):
                driver._query_bool("BOOL?")

    def test_query_float_rejects_nonfinite_response(self):
        driver, resource = self.ready_pm400()
        resource.queue("NUMBER?", "nan")
        with self.assertRaises(InstrumentProtocolError):
            driver._query_float("NUMBER?")

    def test_query_limit_rejects_wrong_enum_without_transport_io(self):
        driver, resource = self.ready_pm400()
        before = list(resource.transactions)
        with self.assertRaises(InstrumentSafetyError):
            driver._query_limit("SENSe:CORRection:WAVelength", "MINimum")
        self.assertEqual(resource.transactions, before)

    def test_query_timeout_is_translated(self):
        driver, resource = self.ready_pm400()
        resource.failures[("query", "SLOW?")] = VisaIOError(StatusCode.error_timeout)
        with self.assertRaises(InstrumentTimeoutError):
            driver._query_text("SLOW?", timeout=0.25)

    def test_query_rejects_and_faults_on_a_second_buffered_response(self):
        driver, resource = self.ready_pm400()
        resource.queue("FIRST?", "first")
        resource.queue_pending_read("second")
        with self.assertRaises(DeviceFault):
            driver._query_text("FIRST?")
        self.assertEqual(resource.pending_reads, [])

    def test_malformed_primary_response_is_preserved_while_trailing_data_faults(self):
        driver, resource = self.ready_pm400()
        resource.queue("BAD?", "first\nsecond")
        resource.queue_pending_read("third")
        with self.assertRaises(InstrumentProtocolError):
            driver._query_text("BAD?")
        self.assertEqual(resource.pending_reads, [])
        self.assertEqual(driver.state, DriverState.FAULT)

    def test_confirmed_setter_serializes_exact_commands_with_configured_newlines(self):
        driver, resource = self.ready_pm400()
        resource.queue("PROP?", "1")
        self.assertEqual(
            driver._set_and_confirm("PROP 1", "PROP?", 1, lambda text: int(text)),
            1,
        )
        self.assertEqual(resource.write_termination, "\n")
        self.assertEqual(resource.read_termination, "\n")
        self.assertEqual(resource.transactions[-2:], ["PROP 1", "PROP?"])

    def test_write_action_records_stale_errors_separately_from_new_command(self):
        driver, resource = self.ready_pm400()
        resource.queue(
            "SYSTem:ERRor?",
            '-222,"Old error"',
            '0,"No error"',
            '0,"No error"',
        )
        driver._write_action("ACTion")
        self.assertEqual(tuple(error.code for error in driver.last_preexisting_errors), (-222,))
        self.assertEqual(
            resource.transactions[-4:],
            ["SYSTem:ERRor?", "SYSTem:ERRor?", "ACTion", "SYSTem:ERRor?"],
        )

    def test_readback_mismatch_raises_device_fault_with_requested_and_observed_values(self):
        driver, resource = self.ready_pm400()
        resource.queue("PROP?", "2")
        with self.assertRaisesRegex(DeviceFault, "requested=1.*readback=2"):
            driver._set_and_confirm("PROP 1", "PROP?", 1, lambda text: int(text))

    def test_set_and_confirm_rejects_unrelated_readback_header_before_writing(self):
        driver, resource = self.ready_pm400()
        before = list(resource.transactions)
        with self.assertRaises(InstrumentSafetyError):
            driver._set_and_confirm("FIRST 1", "SECOND?", 1, lambda text: int(text))
        self.assertEqual(resource.transactions, before)

    def test_unsupported_capability_writes_nothing(self):
        driver, resource = self.ready_pm400(flags=0)
        before = list(resource.transactions)
        with self.assertRaises(InstrumentCapabilityError):
            driver._require_capability("wavelength_settable", "set wavelength")
        self.assertEqual(resource.transactions, before)

    def test_confirmation_requires_the_literal_true_value_before_io(self):
        driver, resource = self.ready_pm400()
        before = list(resource.transactions)
        with self.assertRaises(InstrumentSafetyError):
            driver._require_confirm(1, "reset")
        self.assertEqual(resource.transactions, before)

    def test_set_and_confirm_holds_the_request_lock_until_readback(self):
        driver, resource = self.ready_pm400()
        readback_started = threading.Event()
        release_readback = threading.Event()
        second_query_done = threading.Event()

        def delayed_readback():
            readback_started.set()
            self.assertTrue(release_readback.wait(1.0))
            return "1"

        resource.queue("PROP?", delayed_readback)
        resource.queue("OTHER?", "other")
        setter = threading.Thread(
            target=lambda: driver._set_and_confirm("PROP 1", "PROP?", 1, lambda text: int(text))
        )
        query = threading.Thread(
            target=lambda: (driver._query_text("OTHER?"), second_query_done.set())
        )
        setter.start()
        self.assertTrue(readback_started.wait(1.0))
        query.start()
        self.assertFalse(second_query_done.wait(0.05))
        release_readback.set()
        setter.join(1.0)
        query.join(1.0)
        self.assertFalse(setter.is_alive())
        self.assertFalse(query.is_alive())
        self.assertEqual(resource.transactions[-3:], ["PROP 1", "PROP?", "OTHER?"])


class PM400ConnectionTests(unittest.TestCase):
    def test_start_interruption_before_ident_retains_resource_ownership(self):
        manager, driver, resource = self.connected_pm400()
        driver._TRANSPORT_HELPER_SECONDS = 0.04
        interrupt = KeyboardInterrupt("launch status unknown")
        try:
            with patch("Code.Utils.pm400.threading.Thread.start", side_effect=interrupt):
                with self.assertRaises(KeyboardInterrupt) as caught:
                    driver.close()
            self.assertIs(caught.exception, interrupt)
            self.assertIs(driver._resource, resource)
            self.assertIsNotNone(driver._visa_lease)
            with self.assertRaises(DriverError):
                driver.close()
            self.assertFalse(resource.closed)
            self.assertEqual(len(driver._transport_helpers), 1)
        finally:
            for helper in tuple(driver._transport_helpers):
                if helper.thread.ident is None:
                    helper.thread.start()
                helper.thread.join(2)
            driver.close()

    def test_helper_start_failure_allows_explicit_cleanup_retry(self):
        manager, driver, resource = self.connected_pm400()
        driver._TRANSPORT_HELPER_SECONDS = 0.04
        try:
            with patch("Code.Utils.pm400.threading.Thread.start", side_effect=RuntimeError("cannot start thread")):
                with self.assertRaises(RuntimeError):
                    driver.close()
            self.assertIs(driver._resource, resource)
            self.assertFalse(resource.closed)
            driver.close()
            self.assertEqual(driver.state, DriverState.DISCONNECTED)
            self.assertTrue(resource.closed)
            self.assertIsNone(driver._visa_lease)
        finally:
            # Settle even the inherited implementation's unstarted helpers.
            for helper in tuple(driver._transport_helpers):
                if helper.thread.ident is None:
                    helper.thread.start()
                helper.thread.join(2)
            driver.close()

    def test_start_error_after_launch_retains_running_helper(self):
        manager, driver, resource = self.connected_pm400()
        driver._TRANSPORT_HELPER_SECONDS = 0.04
        entered, release = threading.Event(), threading.Event()
        actual_start, actual_close = threading.Thread.start, resource.close
        calls = []
        def close_resource():
            calls.append("close")
            entered.set()
            if not release.wait(5):
                raise AssertionError("resource close gate never released")
            actual_close()
        def start_then_fail(thread):
            actual_start(thread)
            self.assertTrue(entered.wait(2))
            raise RuntimeError("start interrupted after launch")
        resource.close = close_resource
        try:
            with patch("Code.Utils.pm400.threading.Thread.start", start_then_fail):
                with self.assertRaises(RuntimeError):
                    driver.close()
            with self.assertRaises(DriverError):
                driver.close()
            self.assertIs(driver._resource, resource)
            self.assertIsNotNone(driver._visa_lease)
            self.assertEqual(calls, ["close"])
        finally:
            release.set()
            for helper in tuple(driver._transport_helpers):
                helper.thread.join(2)
            driver.close()
        self.assertEqual(calls, ["close"])

    _IDN = "THORLABS,PM400,P001,2.0.0"
    _SENSOR_ID = '"S130C","S001","cal",1,0,49'

    def setUp(self):
        self.drivers = []

    def tearDown(self):
        for driver in self.drivers:
            try:
                driver.close()
            except BaseException:
                pass

    def make_pm400(self, resource, *, resource_name="USB0::0x1313::0x8078::1::INSTR", manager=None):
        from Code.Utils.pm400 import PM400

        manager = manager or FakeResourceManager(resource)
        driver = PM400(
            resource_name,
            timeout=1.25,
            resource_manager=manager,
            resource_manager_factory=lambda: (_ for _ in ()).throw(AssertionError("unused")),
        )
        self.drivers.append(driver)
        return driver

    def connected_pm400(self, *, injected_manager=False, resource_name="USB0::0x1313::0x8078::1::INSTR"):
        resource = FakeVisaResource({"*IDN?": self._IDN, "SYSTem:SENSor:IDN?": self._SENSOR_ID})
        manager = FakeResourceManager(resource)
        if injected_manager:
            driver = self.make_pm400(resource, resource_name=resource_name, manager=manager).connect()
        else:
            from Code.Utils.pm400 import PM400

            driver = PM400(
                resource_name,
                resource_manager_factory=lambda: manager,
            ).connect()
            self.drivers.append(driver)
        return manager, driver, resource

    def test_connect_only_reads_identity_and_sensor_identity(self):
        resource = FakeVisaResource({"*IDN?": self._IDN, "SYSTem:SENSor:IDN?": self._SENSOR_ID})
        driver = self.make_pm400(resource).connect()
        self.assertEqual(resource.transactions, ["*IDN?", "SYSTem:SENSor:IDN?"])
        self.assertEqual(resource.timeout, 1250)
        self.assertEqual(resource.read_termination, "\n")
        self.assertEqual(resource.write_termination, "\n")
        self.assertEqual(driver.state, DriverState.READY)
        driver.close()
        self.assertEqual(resource.mutating_commands, [])

    def test_negative_sensor_flags_fail_connection_before_capability_publication_or_write(self):
        resource = FakeVisaResource({
            "*IDN?": self._IDN,
            "SYSTem:SENSor:IDN?": '"S130C","S001","cal",1,0,-1',
        })
        driver = self.make_pm400(resource)

        with self.assertRaises(InstrumentProtocolError):
            driver.connect()

        self.assertIsNone(driver.sensor_info)
        self.assertEqual(driver.state, DriverState.DISCONNECTED)
        with self.assertRaises(InstrumentConnectionError):
            driver.sense.set_wavelength_nm(1550.0)
        self.assertEqual(resource.mutating_commands, [])

    def test_injected_manager_is_not_closed(self):
        manager, driver, resource = self.connected_pm400(injected_manager=True)
        driver.close()
        self.assertFalse(manager.closed)
        self.assertEqual(manager.close_calls, 0)
        self.assertTrue(resource.closed)

    def test_same_canonical_resource_cannot_open_twice_then_reopens_after_close(self):
        first_manager, first, _ = self.connected_pm400(resource_name="USB0::0x1313::0x8078::1::INSTR")
        with self.assertRaises(InstrumentConnectionError):
            self.make_pm400(
                FakeVisaResource({"*IDN?": self._IDN, "SYSTem:SENSor:IDN?": self._SENSOR_ID}),
                resource_name=" USB::0x1313::0x8078::1::0::INSTR ",
            ).connect()
        first.close()
        second_manager, second, _ = self.connected_pm400(resource_name=" USB::0x1313::0x8078::1::0::INSTR ")
        self.assertEqual(first_manager.opened_names, ["USB0::0x1313::0x8078::1::0::INSTR"])
        self.assertEqual(second_manager.opened_names, ["USB0::0x1313::0x8078::1::0::INSTR"])
        second.close()

    def test_different_resources_open_in_parallel_with_distinct_request_locks(self):
        resources = [
            FakeVisaResource({"*IDN?": self._IDN, "SYSTem:SENSor:IDN?": self._SENSOR_ID}),
            FakeVisaResource({"*IDN?": self._IDN, "SYSTem:SENSor:IDN?": self._SENSOR_ID}),
        ]
        drivers = [self.make_pm400(resource, resource_name=f"USB0::0x1313::0x8078::{index}::INSTR") for index, resource in enumerate(resources)]
        failures = []
        barrier = threading.Barrier(2)

        def connect(driver):
            try:
                barrier.wait()
                driver.connect()
            except BaseException as error:
                failures.append(error)

        threads = [threading.Thread(target=connect, args=(driver,)) for driver in drivers]
        for thread in threads:
            thread.start()
        for thread in threads:
            thread.join()
        self.assertEqual(failures, [])
        self.assertIsNot(drivers[0]._request_lock, drivers[1]._request_lock)
        self.assertEqual([driver.state for driver in drivers], [DriverState.READY, DriverState.READY])

    def test_owned_manager_closes_exactly_once(self):
        manager, driver, resource = self.connected_pm400()
        driver.close()
        driver.close()
        self.assertTrue(resource.closed)
        self.assertEqual(resource.close_calls, 1)
        self.assertTrue(manager.closed)
        self.assertEqual(manager.close_calls, 1)

    def test_blocking_owned_manager_close_is_bounded_and_reaped_later(self):
        resource = FakeVisaResource(
            {"*IDN?": self._IDN, "SYSTem:SENSor:IDN?": self._SENSOR_ID}
        )
        manager = BlockingResourceManager(resource)
        driver = PM400(
            "USB0::0x1313::0x8078::blocking-manager::INSTR",
            resource_manager_factory=lambda: manager,
        ).connect()
        self.drivers.append(driver)
        closing = ThreadCall(driver.close, name="PM400BlockingManagerClose")
        self.assertTrue(manager.close_started.wait(1.0))
        try:
            with self.assertRaises(DeviceFault):
                closing.result(timeout=0.25)
            self.assertEqual(driver.state, DriverState.FAULT)
            self.assertIsNotNone(driver.cleanup_error)
            self.assertIs(driver._manager, manager)
            self.assertTrue(driver._manager_owned)
        finally:
            manager.allow_close.set()
            closing.thread.join(1.0)
        driver.close()
        self.assertEqual(driver.state, DriverState.DISCONNECTED)
        self.assertIsNone(driver._manager)
        self.assertFalse(driver._manager_owned)
        self.assertIsNone(driver._visa_lease)
        self.assertIsNone(driver._resource_reservation)
        self.assertEqual(manager.close_calls, 1)

    def test_stale_terminal_fallback_does_not_close_resource_twice(self):
        resource = FakeVisaResource(
            {"*IDN?": self._IDN, "SYSTem:SENSor:IDN?": self._SENSOR_ID}
        )
        driver = StaleFallbackPM400(
            "USB0::0x1313::0x8078::stale-fallback::INSTR",
            resource_manager=FakeResourceManager(resource),
        ).connect()
        self.drivers.append(driver)
        evidence = DeviceFault("intentional terminal fallback")
        fallback = ThreadCall(
            lambda: driver._terminal_close_resource_without_request_lock(evidence),
            name="PM400StaleFallback",
        )
        self.assertTrue(driver.stale_fallback_entered.wait(1.0))
        closing = ThreadCall(driver.close, name="PM400ConcurrentNormalClose")
        self.assertIsNone(closing.result())
        driver.release_stale_fallback.set()
        self.assertTrue(fallback.result())
        self.assertEqual(resource.close_calls, 1)
        self.assertEqual(driver.state, DriverState.DISCONNECTED)
        self.assertIsNone(driver.cleanup_error)
        self.assertEqual(driver._transport_helpers, [])

    def test_open_failure_closes_owned_manager_and_releases_registry(self):
        from Code.Utils.pm400 import PM400

        manager = FakeResourceManager(open_failure=OSError("open failed"))
        driver = PM400("USB0::0x1313::0x8078::5::INSTR", resource_manager_factory=lambda: manager)
        self.drivers.append(driver)
        with self.assertRaises(InstrumentConnectionError):
            driver.connect()
        self.assertTrue(manager.closed)
        self.assertEqual(driver.state, DriverState.DISCONNECTED)
        replacement = self.make_pm400(
            FakeVisaResource({"*IDN?": self._IDN, "SYSTem:SENSor:IDN?": self._SENSOR_ID}),
            resource_name="USB0::0x1313::0x8078::5::INSTR",
        ).connect()
        replacement.close()

    def test_identity_query_failure_closes_resources_and_wraps_error(self):
        resource = FakeVisaResource(
            {"*IDN?": self._IDN, "SYSTem:SENSor:IDN?": self._SENSOR_ID},
            failures={("query", "*IDN?"): OSError("idn failed")},
        )
        manager = FakeResourceManager(resource)
        driver = self.make_pm400(resource, manager=manager)
        with self.assertRaises(InstrumentConnectionError):
            driver.connect()
        self.assertTrue(resource.closed)
        self.assertFalse(manager.closed)
        self.assertEqual(driver.state, DriverState.DISCONNECTED)

    def test_identity_query_timeout_preserves_timeout_taxonomy(self):
        resource = FakeVisaResource(
            {"*IDN?": self._IDN, "SYSTem:SENSor:IDN?": self._SENSOR_ID},
            failures={
                ("query", "*IDN?"): VisaIOError(StatusCode.error_timeout),
            },
        )
        driver = self.make_pm400(resource)
        with self.assertRaises(InstrumentTimeoutError):
            driver.connect()
        self.assertTrue(resource.closed)
        self.assertEqual(driver.state, DriverState.DISCONNECTED)

    def test_sensor_identity_failure_closes_resources_and_wraps_error(self):
        resource = FakeVisaResource(
            {"*IDN?": self._IDN, "SYSTem:SENSor:IDN?": self._SENSOR_ID},
            failures={("query", "SYSTem:SENSor:IDN?"): OSError("sensor id failed")},
        )
        driver = self.make_pm400(resource)
        with self.assertRaises(InstrumentConnectionError):
            driver.connect()
        self.assertTrue(resource.closed)
        self.assertEqual(driver.state, DriverState.DISCONNECTED)

    def test_unexpected_manufacturer_or_model_is_rejected(self):
        for identity in ("OTHER,PM400,P001,2.0", "THORLABS,PM300,P001,2.0"):
            with self.subTest(identity=identity):
                resource = FakeVisaResource({"*IDN?": identity, "SYSTem:SENSor:IDN?": self._SENSOR_ID})
                driver = self.make_pm400(resource, resource_name=f"USB0::0x1313::0x8078::{identity.split(',')[1]}::INSTR")
                with self.assertRaises(InstrumentConnectionError):
                    driver.connect()
                self.assertTrue(resource.closed)

    def test_close_failure_retains_resource_fault_and_retries(self):
        resource = FakeVisaResource(
            {"*IDN?": self._IDN, "SYSTem:SENSor:IDN?": self._SENSOR_ID},
            failures={"close": OSError("close failed")},
        )
        driver = self.make_pm400(resource).connect()
        with self.assertRaisesRegex(OSError, "close failed"):
            driver.close()
        self.assertIs(driver._resource, resource)
        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertIsInstance(driver.cleanup_error, OSError)
        resource.failures.pop("close")
        driver.close()
        self.assertIsNone(driver._resource)
        self.assertEqual(driver.state, DriverState.DISCONNECTED)
        self.assertIsNone(driver.cleanup_error)

    def test_keyboard_interrupt_during_connect_closes_partial_connection(self):
        resource = FakeVisaResource(
            {"*IDN?": self._IDN, "SYSTem:SENSor:IDN?": self._SENSOR_ID},
            failures={("query", "*IDN?"): KeyboardInterrupt("interrupted")},
        )
        driver = self.make_pm400(resource)
        with self.assertRaisesRegex(KeyboardInterrupt, "interrupted"):
            driver.connect()
        self.assertTrue(resource.closed)
        self.assertEqual(driver.state, DriverState.DISCONNECTED)

    def test_keyboard_interrupt_during_close_keeps_resource_for_retry(self):
        resource = FakeVisaResource(
            {"*IDN?": self._IDN, "SYSTem:SENSor:IDN?": self._SENSOR_ID},
            failures={"close": KeyboardInterrupt("interrupted")},
        )
        driver = self.make_pm400(resource).connect()
        with self.assertRaisesRegex(KeyboardInterrupt, "interrupted"):
            driver.close()
        self.assertIs(driver._resource, resource)
        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertIsInstance(driver.cleanup_error, KeyboardInterrupt)
        resource.failures.pop("close")
        driver.close()

    def test_resource_interrupt_retains_lease_and_preserves_other_device_until_retry(self):
        from Code.Utils.pm400 import PM400

        interrupted = KeyboardInterrupt("resource interrupted")
        resource = FakeVisaResource(
            {"*IDN?": self._IDN, "SYSTem:SENSor:IDN?": self._SENSOR_ID},
            failures={"close": interrupted},
        )
        other_resource = FakeVisaResource({
            "*IDN?": self._IDN, "SYSTem:SENSor:IDN?": self._SENSOR_ID,
            "SYSTem:VERSion?": "1999.0",
        })
        manager = SharedResourceManager({
            "USB0::0x1313::0x8078::6::0::INSTR": resource,
            "USB0::0x1313::0x8078::other::0::INSTR": other_resource,
        })
        driver = PM400("USB0::0x1313::0x8078::6::INSTR", resource_manager_factory=lambda: manager).connect()
        self.drivers.append(driver)
        other = PM400("USB0::0x1313::0x8078::other::INSTR", resource_manager_factory=lambda: manager).connect()
        self.drivers.append(other)
        try:
            with self.assertRaises(KeyboardInterrupt) as caught:
                driver.close()
            self.assertIs(caught.exception, interrupted)
            self.assertIs(driver.cleanup_error, interrupted)
            self.assertEqual(driver.state, DriverState.FAULT)
            self.assertEqual(manager.close_calls, 0)
            self.assertIs(driver._resource, resource)
            self.assertFalse(driver._visa_lease.released)
            self.assertFalse(driver._resource_reservation.released)
            self.assertEqual(other.system.get_scpi_version(), "1999.0")
        finally:
            resource.failures.pop("close", None)
            driver.close()
            other.close()
        self.assertEqual(manager.close_calls, 1)
        self.assertTrue(manager.closed)
        self.assertIsNone(driver._resource)
        self.assertIsNone(driver._visa_lease)
        self.assertIsNone(driver._resource_reservation)
        self.assertEqual(driver.state, DriverState.DISCONNECTED)

    def test_context_body_exception_has_priority_over_cleanup_error(self):
        resource = FakeVisaResource(
            {"*IDN?": self._IDN, "SYSTem:SENSor:IDN?": self._SENSOR_ID},
            failures={"close": OSError("cleanup failed")},
        )
        driver = self.make_pm400(resource)
        try:
            with self.assertRaisesRegex(RuntimeError, "body failed"):
                with driver:
                    raise RuntimeError("body failed")
            self.assertEqual(driver.state, DriverState.FAULT)
            self.assertIsInstance(driver.cleanup_error, OSError)
        finally:
            resource.failures.pop("close")
            driver.close()


class _DiagnosticVisaManager:
    def __init__(self, resources=("USB0::0x1313::0x8078::PM400::INSTR",), failure=None):
        self.resources = resources
        self.failure = failure
        self.close_calls = 0

    def list_resources(self):
        if self.failure is not None:
            raise self.failure
        return self.resources

    def close(self):
        self.close_calls += 1


class _DiagnosticPM400:
    def __init__(self):
        self.instrument_info = InstrumentInfo(
            "Thorlabs", "PM400", "P400", "2.0", "Thorlabs,PM400,P400,2.0"
        )
        capabilities = SensorInfo.parse('"S130C","S1","cal",1,2,371')
        self.sensor_info = capabilities
        self.connect_calls = 0
        self.close_calls = 0
        self.brightness_reads = 0
        self.brightness_sets = []
        self.configuration_reads = 0
        self.measurement_reads = []
        self.measurement_error = None
        self.display = SimpleNamespace(
            get_brightness=self.get_brightness,
            set_brightness=self.set_brightness,
        )
        self.measurement = SimpleNamespace(
            get_configuration=self.get_configuration,
            read=self.read_measurement,
        )

    def connect(self):
        self.connect_calls += 1
        return self

    def close(self):
        self.close_calls += 1

    def get_brightness(self):
        self.brightness_reads += 1
        return 0.625

    def set_brightness(self, value):
        self.brightness_sets.append(value)
        return value

    def get_configuration(self):
        self.configuration_reads += 1
        return MeasurementKind.POWER

    def read_measurement(self, kind=None, timeout=None):
        self.measurement_reads.append((kind, timeout))
        if self.measurement_error is not None:
            raise self.measurement_error
        return Measurement(MeasurementKind.POWER, 1.25, "W", 10.0, "1.25")


class PM400DiagnosticTests(unittest.TestCase):
    def run_main(self, argv, *, fake=None, prompt=None):
        fake = fake or _DiagnosticPM400()
        output = io.StringIO()
        patches = [
            patch.object(sys, "argv", argv),
            patch.object(check_pm400, "PM400", return_value=fake),
        ]
        if prompt is not None:
            patches.append(patch("builtins.input", side_effect=prompt))
        with patches[0], patches[1], redirect_stdout(output):
            if len(patches) == 3:
                with patches[2]:
                    result = check_pm400.main()
            else:
                result = check_pm400.main()
        return result, fake, output.getvalue()

    def test_package_exports_are_exact_unique_and_preserve_existing_names(self):
        import Code.Utils as utils

        expected = {
            "DeviceFault", "DriverError", "DriverState",
            "InstrumentCapabilityError", "InstrumentConnectionError",
            "InstrumentProtocolError", "InstrumentSafetyError",
            "InstrumentTimeoutError", "find_serial_port", "get_logger",
            "require_state", "AQ6370", "Spectrum", "VoltageSource",
            "VoltageStatus", "GainDriver", "GainStatus", "PM400",
            "ZeroState", "ZeroEvidence",
            "InstrumentInfo", "SensorInfo", "SensorCapabilities",
            "Measurement", "MeasurementKind", "PowerUnit", "AdapterType",
            "StatusGroup", "LimitSelector", "SystemError", "MDT693B",
            "MDTStatus", "AxisState", "Axis", "VoltageLimit", "RotaryMode",
            "TLB6700", "LaserStatus",
        }
        self.assertEqual(set(utils.__all__), expected)
        self.assertEqual(len(utils.__all__), len(set(utils.__all__)))
        self.assertTrue(all(not name.startswith("_") for name in utils.__all__))
        self.assertTrue(all(hasattr(utils, name) for name in expected))

    def test_no_resource_lists_only_and_closes_listing_manager(self):
        manager = _DiagnosticVisaManager(("USB0::0x1313::0x8078::A::INSTR", "USB0::0x1313::0x8078::B::INSTR"))
        output = io.StringIO()
        with patch.object(sys, "argv", ["check_pm400"]), \
             patch.object(check_pm400.pyvisa, "ResourceManager", return_value=manager), \
             patch.object(check_pm400, "PM400") as constructor, \
             redirect_stdout(output):
            self.assertEqual(check_pm400.main(), 0)
        constructor.assert_not_called()
        self.assertEqual(manager.close_calls, 1)
        self.assertIn("USB0::0x1313::0x8078::A::INSTR", output.getvalue())
        self.assertIn("USB0::0x1313::0x8078::B::INSTR", output.getvalue())

    def test_listing_manager_closes_for_exception_keyboard_interrupt_and_system_exit(self):
        for failure in (RuntimeError("list"), KeyboardInterrupt(), SystemExit(7)):
            with self.subTest(kind=type(failure).__name__):
                manager = _DiagnosticVisaManager(failure=failure)
                with patch.object(sys, "argv", ["check_pm400"]), \
                     patch.object(check_pm400.pyvisa, "ResourceManager", return_value=manager):
                    with self.assertRaises(type(failure)) as caught:
                        check_pm400.main()
                self.assertIs(caught.exception, failure)
                self.assertEqual(manager.close_calls, 1)

    def test_selected_resource_is_read_only_measures_current_kind_and_closes(self):
        result, fake, output = self.run_main(
            ["check_pm400", "--resource", "USB0::0x1313::0x8078::PM400::INSTR", "--timeout", "1.25"]
        )
        self.assertEqual(result, 0)
        self.assertEqual(fake.connect_calls, 1)
        self.assertEqual(fake.configuration_reads, 1)
        self.assertEqual(fake.measurement_reads, [(None, 1.25)])
        self.assertEqual(fake.brightness_sets, [])
        self.assertEqual(fake.close_calls, 1)
        for text in ("PM400", "S130C", "POWER", "1.25", "W"):
            self.assertIn(text, output)

    def test_selected_resource_and_timeout_are_routed_exactly_to_constructor(self):
        fake = _DiagnosticPM400()
        with patch.object(sys, "argv", [
                 "check_pm400", "--resource", "USB0::0x1313::0x8078::PM400::EXACT", "--timeout", "2.5"
             ]), patch.object(check_pm400, "PM400", return_value=fake) as constructor, \
             redirect_stdout(io.StringIO()):
            self.assertEqual(check_pm400.main(), 0)
        constructor.assert_called_once_with("USB0::0x1313::0x8078::PM400::EXACT", timeout=2.5)

    def test_selected_resource_failure_still_closes_without_mutation(self):
        fake = _DiagnosticPM400()
        failure = RuntimeError("measurement failed")
        fake.measurement_error = failure
        with self.assertRaises(RuntimeError) as caught:
            self.run_main(["check_pm400", "--resource", "USB0::0x1313::0x8078::PM400::INSTR"], fake=fake)
        self.assertIs(caught.exception, failure)
        self.assertEqual(fake.brightness_sets, [])
        self.assertEqual(fake.close_calls, 1)

    def test_timeout_domain_is_rejected_before_driver_construction(self):
        for value in ("0", "-1", "nan", "inf", "True"):
            with self.subTest(value=value), \
                 patch.object(sys, "argv", ["check_pm400", "--resource", "USB::X", "--timeout", value]), \
                 patch.object(check_pm400, "PM400") as constructor, \
                 redirect_stderr(io.StringIO()), \
                 self.assertRaises(SystemExit):
                check_pm400.main()
            constructor.assert_not_called()

    def test_no_op_requires_exact_phrase_and_writes_same_value_once(self):
        result, fake, output = self.run_main(
            ["check_pm400", "--resource", "USB::X", "--no-op-check"],
            prompt=["WRITE SAME VALUE"],
        )
        self.assertEqual(result, 0)
        self.assertEqual(fake.brightness_sets, [0.625])
        self.assertEqual(fake.brightness_reads, 1)
        self.assertIn("0.625", output)
        self.assertIn("set_brightness(0.625)", output)
        self.assertEqual(fake.close_calls, 1)

    def test_no_op_decline_blank_eof_interrupt_and_system_exit_never_write(self):
        prompt_results = ["no", "", EOFError(), KeyboardInterrupt(), SystemExit(3)]
        for prompt_result in prompt_results:
            with self.subTest(prompt=repr(prompt_result)):
                fake = _DiagnosticPM400()
                result, _, _ = self.run_main(
                    ["check_pm400", "--resource", "USB::X", "--no-op-check"],
                    fake=fake,
                    prompt=[prompt_result],
                )
                self.assertEqual(result, 0)
                self.assertEqual(fake.brightness_sets, [])
                self.assertEqual(fake.close_calls, 1)

    def test_help_performs_no_discovery_or_driver_construction(self):
        with patch.object(sys, "argv", ["check_pm400", "--help"]), \
             patch.object(check_pm400.pyvisa, "ResourceManager") as manager, \
             patch.object(check_pm400, "PM400") as constructor, \
             redirect_stdout(io.StringIO()), \
             self.assertRaises(SystemExit) as caught:
            check_pm400.main()
        self.assertEqual(caught.exception.code, 0)
        manager.assert_not_called()
        constructor.assert_not_called()


if __name__ == "__main__":
    unittest.main()
