"""Metadata-only serial and VISA inventory. No instrument is opened here."""

from __future__ import annotations

from collections.abc import Callable, Iterable
from typing import Any

import pyvisa
from serial.tools import list_ports

from Code.Setups.fiber_coupling import FiberCouplingSetup, StageSide


def _port_record(port: object) -> dict[str, Any]:
    vid = getattr(port, "vid", None)
    pid = getattr(port, "pid", None)
    return {
        "resource": str(getattr(port, "device", "")),
        "vid": vid if type(vid) is int else None,
        "pid": pid if type(pid) is int else None,
        "serial": str(getattr(port, "serial_number", "") or ""),
        "description": str(getattr(port, "description", "") or ""),
    }


def discover(
    *,
    port_enumerator: Callable[[], Iterable[object]] = list_ports.comports,
    resource_manager_factory: Callable[[], object] = pyvisa.ResourceManager,
    newport_inventory: Callable | None = None,
) -> dict[str, Any]:
    """Return candidates and partial enumeration errors without querying devices."""
    result: dict[str, Any] = {
        "serial": [], "visa": [], "fiber": {"left": None, "right": None, "unknown": []},
        "suggestions": {"voltage": [], "gain": []}, "errors": {},
    }
    try:
        if newport_inventory is None:
            from Code.Utils.newport_usb import inventory as newport_inventory
        result['newport'] = newport_inventory()
    except Exception as error:
        result['errors']['newport'] = f'{type(error).__name__}: {error}'
    try:
        ports = tuple(port_enumerator())
        result["serial"] = [_port_record(port) for port in ports]
        try:
            stages = FiberCouplingSetup.enumerate(port_enumerator=lambda: ports)
            for side in StageSide:
                info = stages.registered.get(side)
                if info is not None:
                    result["fiber"][side.value] = {
                        "resource": info.resource,
                        "serial": info.serial_number,
                        "description": info.description,
                    }
            result["fiber"]["unknown"] = [
                {"resource": info.resource, "serial": info.serial_number,
                 "description": info.description}
                for info in stages.unknown_devices
            ]
        except Exception as error:
            result["errors"]["fiber"] = f"{type(error).__name__}: {error}"
        else:
            stage_ports = {info["resource"].casefold() for info in result["fiber"].values()
                           if isinstance(info, dict)}
            stage_ports.update(info["resource"].casefold()
                               for info in result["fiber"]["unknown"])
            for record in result["serial"]:
                if record["resource"].casefold() in stage_ports:
                    continue
                identity = (record["vid"], record["pid"])
                if identity == (0x1A86, 0x7523):
                    result["suggestions"]["voltage"].append(record["resource"])
                elif identity == (0x10C4, 0xEA60):
                    result["suggestions"]["gain"].append(record["resource"])
    except Exception as error:
        result["errors"]["serial"] = f"{type(error).__name__}: {error}"

    manager = None
    try:
        manager = resource_manager_factory()
        result["visa"] = list(manager.list_resources())
    except Exception as error:
        result["errors"]["visa"] = f"{type(error).__name__}: {error}"
    finally:
        if manager is not None:
            try:
                manager.close()
            except Exception as error:
                result["errors"]["visa_close"] = f"{type(error).__name__}: {error}"
    return result
