"""Trusted model profiles. Loading this module never imports a hardware driver."""
from __future__ import annotations

import json
import math
import re
from dataclasses import dataclass
from pathlib import Path
from types import MappingProxyType
from typing import Any, Mapping

DEFAULT_PATH = Path(__file__).resolve().parents[1] / "catalog" / "devices.json"
DRIVERS = {"aq6370": "osa", "voltage": "voltage", "gain": "gain",
           "pm400": "pm400", "mdt693b": "mdt", "tlb6700": "laser"}
CATEGORIES = ("OSA", "ESA", "Oscilloscope", "Function Generator",
              "Power Meter", "Piezo Controller", "Laser", "Custom")
REVIEWED = {
    "aq6370": ("OSA", "gpib-visa", "visa", ("GPIB",)),
    "pm400": ("Power Meter", "usb-visa", "visa", ("USB",)),
    "voltage": ("Custom", "ch340-serial", "serial", ("Serial",)),
    "gain": ("Custom", "cp210x-serial", "serial", ("Serial",)),
    "mdt693b": ("Piezo Controller", "serial", "serial", ("Serial",)),
    "tlb6700": ("Laser", "newport-usb", "newport", ("USB",)),
}


class CatalogError(ValueError):
    """The model, profile, or connection is outside the reviewed directory."""


def _object(value, required, optional=()):
    if type(value) is not dict or not set(required) <= set(value) or (
        set(value) - set(required) - set(optional)
    ):
        raise CatalogError("Unexpected or missing catalog fields")
    return value


def _strings(value):
    if type(value) is not list or any(type(item) is not str or not item for item in value):
        raise CatalogError("Expected nonempty strings")
    if len(value) != len(set(value)):
        raise CatalogError("Duplicate catalog entries")
    return tuple(value)


def _freeze(value):
    if type(value) is dict:
        return MappingProxyType({key: _freeze(item) for key, item in value.items()})
    if type(value) is list:
        return tuple(_freeze(item) for item in value)
    return value


def _pairs(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise CatalogError("Duplicate JSON field: " + key)
        result[key] = value
    return result


def _constant(value):
    raise CatalogError("Non-finite JSON value: " + value)


@dataclass(frozen=True)
class Category:
    name: str
    can_register_without_driver: bool = False


@dataclass(frozen=True)
class Profile:
    id: str
    interfaces: tuple[str, ...]
    access: str
    probe_mode: str
    probe_version: int
    automatic_probe: bool
    communication_ttl_s: int
    dependencies: tuple[str, ...]
    transport_hint: Mapping | None
    open_effects: tuple[str, ...]
    fields: Mapping[str, Mapping[str, Any]]

    @property
    def baudrate(self):
        return self.fields.get("baudrate", {}).get("default")

    def validate(self, params: dict) -> dict:
        if type(params) is not dict or set(params) - set(self.fields):
            raise CatalogError("Unknown connection parameter")
        result = {}
        for name, rule in self.fields.items():
            if name in params:
                value = params[name]
            elif "default" in rule:
                value = rule["default"]
            elif rule.get("required", False):
                raise CatalogError("Missing connection parameter: " + name)
            else:
                continue
            kind = rule["kind"]
            if kind in {"text", "serial", "resource"}:
                if type(value) is not str or not value or len(value) > 256 or any(
                    ord(char) < 32 or ord(char) == 127 for char in value
                ):
                    raise CatalogError("Invalid text parameter: " + name)
                if kind == "serial":
                    value = value.upper()
                    if not re.fullmatch(r"COM[1-9][0-9]{0,3}", value):
                        raise CatalogError("Expected a Windows COM port")
                if kind == "resource":
                    gpib = re.fullmatch(r"GPIB[0-9]*::[0-9]{1,2}(?:::[0-9]{1,2})?::INSTR", value)
                    usb = re.fullmatch(r"USB[0-9]*::[A-Za-z0-9]+::[A-Za-z0-9]+::[A-Za-z0-9_.-]+(?:::[0-9]+)?::INSTR", value)
                    if not ((gpib and "GPIB" in rule["interfaces"]) or
                            (usb and "USB" in rule["interfaces"])):
                        raise CatalogError("VISA interface is not reviewed for this profile")
                if name == 'device_key' and not re.fullmatch(r'6700 SN[0-9]{1,16}', value):
                    raise CatalogError('Expected the exact Newport controller key: 6700 SN<serial>')
            elif kind == "integer":
                if type(value) is not int:
                    raise CatalogError("Expected an integer: " + name)
            elif kind == "number":
                if type(value) not in {int, float} or not math.isfinite(value):
                    raise CatalogError("Expected a finite number: " + name)
            else:
                raise CatalogError("Unknown connection field kind")
            if "choices" in rule and value not in rule["choices"]:
                raise CatalogError("Value outside reviewed choices: " + name)
            if "minimum" in rule and value < rule["minimum"]:
                raise CatalogError("Value below allowed minimum: " + name)
            if "maximum" in rule and value > rule["maximum"]:
                raise CatalogError("Value above allowed maximum: " + name)
            result[name] = value
        return result


@dataclass(frozen=True)
class Model:
    id: str
    manufacturer: str
    name: str
    category: str
    driver_id: str
    driver_kind: str
    driver_version: int
    dependencies: tuple[str, ...]
    connect_effects: tuple[str, ...]
    close_effects: tuple[str, ...]
    operations: tuple[str, ...]
    profiles: Mapping[str, Profile]


@dataclass(frozen=True)
class Catalog:
    categories: tuple[Category, ...]
    models: Mapping[str, Model]
    setups: tuple[Mapping, ...]

    def category(self, name: str) -> Category:
        for category in self.categories:
            if category.name == name:
                return category
        raise CatalogError("Unknown category")

    def model(self, model_id: str) -> Model:
        try:
            return self.models[model_id]
        except (KeyError, TypeError) as error:
            raise CatalogError("Driver required for this model") from error

    def models_for(self, category: str) -> tuple[Model, ...]:
        self.category(category)
        return tuple(item for item in self.models.values() if item.category == category)

    def profile(self, model_id: str, profile_id: str) -> Profile:
        try:
            return self.model(model_id).profiles[profile_id]
        except (KeyError, TypeError) as error:
            raise CatalogError("Unreviewed connection profile") from error


def _profile(value, model_id):
    _object(value, ("id", "interfaces", "access", "probe_mode", "probe_version",
                    "automatic_probe", "communication_ttl_s", "dependencies",
                    "transport_hint", "open_effects", "fields"))
    _, profile_id, access, interfaces = REVIEWED[model_id]
    if value["id"] != profile_id or value["access"] != access or (
        _strings(value["interfaces"]) != interfaces
    ):
        raise CatalogError("Unreviewed model/profile/interface binding")
    if type(value["probe_mode"]) is not str or value["probe_mode"] not in {
        "readonly_pending", "readonly", "supervised"
    }:
        raise CatalogError("Unknown access or probe mode")
    if type(value["probe_version"]) is not int or value["probe_version"] < 1 or (
        type(value["communication_ttl_s"]) is not int or value["communication_ttl_s"] < 1
    ) or type(value["automatic_probe"]) is not bool:
        raise CatalogError("Invalid profile version or TTL")
    if value["automatic_probe"] and value["probe_mode"] != "readonly":
        raise CatalogError("Automatic probe is not eligible")
    if type(value["id"]) is not str or not value["id"] or type(value["fields"]) is not dict:
        raise CatalogError("Invalid profile identity or fields")
    expected = ({"device_key": "text"} if access == "newport" else
                {"resource": "resource", "backend": "text", "timeout_s": "number"}
                if access == "visa" else
                {"port": "serial", "baudrate": "integer", "io_timeout_s": "number"})
    if set(value["fields"]) != set(expected):
        raise CatalogError("Unreviewed connection fields")
    for name, rule in value["fields"].items():
        _object(rule, ("kind",), ("required", "default", "choices", "minimum", "maximum", "interfaces"))
        if rule["kind"] != expected[name]:
            raise CatalogError("Unknown field kind")
        if "required" in rule and type(rule["required"]) is not bool:
            raise CatalogError("Invalid required flag")
        if "choices" in rule and (type(rule["choices"]) is not list or not rule["choices"]):
            raise CatalogError("Invalid choices")
        for bound in ("minimum", "maximum"):
            if bound in rule and (type(rule[bound]) not in {int, float} or
                                  not math.isfinite(rule[bound])):
                raise CatalogError("Invalid field bound")
    if access == "serial" and (value["fields"]["baudrate"].get("default") != 115200 or
                               value["fields"]["baudrate"].get("choices") != [115200]):
        raise CatalogError("Serial baudrate is fixed")
    if access == "visa" and (value["fields"]["backend"].get("default") != "system" or
        value["fields"]["backend"].get("choices") != ["system"] or
        _strings(value["fields"]["resource"].get("interfaces")) != interfaces):
        raise CatalogError("VISA backend/interface is fixed")
    profile = Profile(value["id"], _strings(value["interfaces"]), value["access"],
                   value["probe_mode"], value["probe_version"], value["automatic_probe"],
                   value["communication_ttl_s"], _strings(value["dependencies"]),
                   _freeze(value["transport_hint"]), _strings(value["open_effects"]),
                   _freeze(value["fields"]))
    sample = {"device_key": "6700 SN1012"} if access == "newport" else {"port": "COM1"} if access == "serial" else {
        "resource": "GPIB0::4::INSTR" if interfaces == ("GPIB",) else
                    "USB0::0x1313::0x807B::TEST::INSTR"}
    profile.validate(sample)
    return profile


def load_catalog(path: Path = DEFAULT_PATH) -> Catalog:
    try:
        value = json.loads(path.read_text(encoding="utf-8"), object_pairs_hook=_pairs,
                           parse_constant=_constant)
    except (OSError, ValueError, TypeError) as error:
        raise CatalogError("Cannot load trusted catalog: " + str(error)) from error
    _object(value, ("version", "categories", "models", "setups"))
    if type(value["version"]) is not int or value["version"] != 1 or (
        _strings(value["categories"]) != CATEGORIES
    ):
        raise CatalogError("Unsupported catalog version or categories")
    if type(value["models"]) is not list or type(value["setups"]) is not list:
        raise CatalogError("Invalid model or setup list")
    models = {}
    for raw in value["models"]:
        _object(raw, ("id", "manufacturer", "name", "category", "driver_id", "driver_kind",
                      "driver_version", "dependencies", "connect_effects", "close_effects",
                      "operations", "profiles"))
        if any(type(raw[field]) is not str or not raw[field] for field in
               ("id", "manufacturer", "name", "category", "driver_id", "driver_kind")):
            raise CatalogError("Invalid model metadata")
        if raw["id"] not in DRIVERS or raw["driver_id"] != raw["id"] or (
            DRIVERS[raw["id"]] != raw["driver_kind"]
        ) or raw["category"] != REVIEWED[raw["id"]][0]:
            raise CatalogError("Unknown trusted driver binding")
        if raw["id"] in models or type(raw["profiles"]) is not list or (
            type(raw["driver_version"]) is not int or raw["driver_version"] < 1
        ):
            raise CatalogError("Duplicate model or invalid version")
        profiles = {}
        for item in raw["profiles"]:
            profile = _profile(item, raw["id"])
            if profile.id in profiles:
                raise CatalogError("Duplicate connection profile")
            profiles[profile.id] = profile
        models[raw["id"]] = Model(raw["id"], raw["manufacturer"], raw["name"],
            raw["category"], raw["driver_id"], raw["driver_kind"], raw["driver_version"],
            _strings(raw["dependencies"]), _strings(raw["connect_effects"]),
            _strings(raw["close_effects"]), _strings(raw["operations"]),
            MappingProxyType(profiles))
    for setup in value["setups"]:
        _object(setup, ("kind", "model_id", "driver_id", "driver_kind", "name", "members",
                        "operations", "close_effects"))
        if setup["kind"] != "fiber" or setup["driver_id"] != "fiber" or (
            setup["driver_kind"] != "fiber" or setup["model_id"] != "fiber-coupling"
        ) or _strings(setup["members"]) != ("2110148249-10", "160721175410"):
            raise CatalogError("Unknown setup binding")
    return Catalog(tuple(Category(name) for name in CATEGORIES),
                   MappingProxyType(models), tuple(_freeze(item) for item in value["setups"]))

