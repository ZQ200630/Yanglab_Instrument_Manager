"""Bounded, worker-owned delivery staging; not the durable capture archive."""
from __future__ import annotations

import copy
from contextlib import contextmanager, ExitStack
import ctypes
from ctypes import wintypes
from dataclasses import asdict
import hashlib
import json
import os
from pathlib import Path
import re
import stat
from threading import RLock

import numpy as np

from Code.Utils.osa_trace import MAX_TRACE_POINTS, TraceCapture


MAX_ENTRIES = 32
MAX_BYTES = 128 * 1024 * 1024
MAX_CHUNK = 16384
MAX_METADATA = 8192
MAX_NATIVE_BYTES = MAX_TRACE_POINTS * 16


def _identifier(value):
    if type(value) is not str or re.fullmatch(r"[0-9a-f]{32}", value) is None:
        raise ValueError("Capture identity must be 32 lowercase hexadecimal characters")
    return value


def _is_reparse(info):
    return stat.S_ISLNK(info.st_mode) or bool(
        getattr(info, "st_file_attributes", 0) & getattr(stat, "FILE_ATTRIBUTE_REPARSE_POINT", 1024))


class _FileInfo(ctypes.Structure):
    _fields_ = [("attributes", wintypes.DWORD), ("created", wintypes.FILETIME),
                ("accessed", wintypes.FILETIME), ("written", wintypes.FILETIME),
                ("volume", wintypes.DWORD), ("size_high", wintypes.DWORD),
                ("size_low", wintypes.DWORD), ("links", wintypes.DWORD),
                ("index_high", wintypes.DWORD), ("index_low", wintypes.DWORD)]


def _windows_files():
    # Match the native archive's transaction guards; other platforms fail closed
    # rather than silently substituting pathname checks for Windows sharing.
    if os.name != "nt":
        raise OSError("Guarded capture staging requires Windows")
    api = ctypes.WinDLL("kernel32", use_last_error=True)
    api.CreateFileW.argtypes = (wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD,
                               ctypes.c_void_p, wintypes.DWORD, wintypes.DWORD, wintypes.HANDLE)
    api.CreateFileW.restype = wintypes.HANDLE
    api.GetFileInformationByHandle.argtypes = (wintypes.HANDLE, ctypes.POINTER(_FileInfo))
    api.GetFileInformationByHandle.restype = wintypes.BOOL
    api.CloseHandle.argtypes = (wintypes.HANDLE,)
    api.CloseHandle.restype = wintypes.BOOL
    api.SetFileInformationByHandle.argtypes = (wintypes.HANDLE, ctypes.c_int,
                                             ctypes.c_void_p, wintypes.DWORD)
    api.SetFileInformationByHandle.restype = wintypes.BOOL
    return api


def _file_info(api, handle, directory):
    info = _FileInfo()
    if not api.GetFileInformationByHandle(handle, ctypes.byref(info)):
        raise ctypes.WinError(ctypes.get_last_error())
    if info.attributes & 0x400 or bool(info.attributes & 0x10) != directory:
        raise ValueError("Staging requires ordinary files and directories, not reparse points")
    if not directory and info.links != 1:
        raise ValueError("Staged file must have exactly one link")
    return info


@contextmanager
def _opened(path, *, directory=False, delete=False, create=False):
    api = _windows_files()
    access = (0x81 if directory else 0x80 | (0x10000 if delete else 0x80000000))
    if create:
        access = 0x40000080
    handle = api.CreateFileW(str(path), access, 1, None, 1 if create else 3,
                             0x200000 | (0x2000000 if directory else 0), None)
    if handle == ctypes.c_void_p(-1).value:
        raise ctypes.WinError(ctypes.get_last_error())
    primary = None
    try:
        info = _file_info(api, handle, directory)
        yield api, handle, info
    except BaseException as error:
        primary = error
        raise
    finally:
        if not api.CloseHandle(handle):
            error = ctypes.WinError(ctypes.get_last_error())
            if primary is None:
                raise error
            setattr(primary, "staging_close_error", error)


@contextmanager
def _pinned_directories(root):
    with ExitStack() as guards:
        for path in reversed((root, *root.parents)):
            guards.enter_context(_opened(path, directory=True))
        yield


def _read_file(path, maximum):
    with _opened(path) as (_, handle, info):
        length = (info.size_high << 32) | info.size_low
        if length > maximum:
            raise ValueError("Staged capture length changed")
        # Duplicate the OS handle before CRT ownership transfer. The guard's
        # original handle remains open through validation/read and is closed once.
        fd = _duplicate_fd(handle, os.O_RDONLY | os.O_BINARY)
        with os.fdopen(fd, "rb") as stream:
            payload = stream.read(maximum + 1)
        if len(payload) != length:
            raise ValueError("Staged capture length changed")
        return payload


def _duplicate_fd(handle, flags):
    import msvcrt
    api = _windows_files()
    api.GetCurrentProcess.restype = wintypes.HANDLE
    api.DuplicateHandle.argtypes = (wintypes.HANDLE, wintypes.HANDLE, wintypes.HANDLE,
                                   ctypes.POINTER(wintypes.HANDLE), wintypes.DWORD,
                                   wintypes.BOOL, wintypes.DWORD)
    api.DuplicateHandle.restype = wintypes.BOOL
    duplicate = wintypes.HANDLE()
    process = api.GetCurrentProcess()
    if not api.DuplicateHandle(process, handle, process, ctypes.byref(duplicate), 0, False, 2):
        raise ctypes.WinError(ctypes.get_last_error())
    try:
        return msvcrt.open_osfhandle(duplicate.value, flags)
    except BaseException:
        api.CloseHandle(duplicate)
        raise


def _delete_file(path):
    try:
        with _opened(path, delete=True) as (api, handle, _):
            disposition = wintypes.BOOLEAN(1)
            if not api.SetFileInformationByHandle(handle, 4, ctypes.byref(disposition),
                                                 ctypes.sizeof(disposition)):
                raise ctypes.WinError(ctypes.get_last_error())
    except FileNotFoundError:
        # An earlier partial acknowledgement may already have deleted this file.
        pass


def _write_file(path, content):
    with _opened(path, create=True) as (_, guarded, _):
        _write_fd(_duplicate_fd(guarded, os.O_WRONLY | os.O_BINARY), content)


def _write_fd(handle, content):
    primary = None
    try:
        offset = 0
        while offset < len(content):
            part = memoryview(content)[offset:offset + 65536]
            written = os.write(handle, part)
            if type(written) is not int or not 1 <= written <= len(part):
                raise OSError("Capture write made invalid progress")
            offset += written
        os.fsync(handle)
    except BaseException as error:
        primary = error
        raise
    finally:
        try:
            os.close(handle)
        except BaseException as error:
            if primary is None:
                raise
            setattr(primary, "staging_close_error", error)


class CaptureSpool:
    """A nonce-named private directory, with no crash-file trust or auto-delete.

    Only this process's completed put creates a deliverable entry. Existing files
    remain orphans, count toward capacity, and cannot become live proof or data.
    All paths are internal; a client supplies only a typed capture identifier.
    """
    def __init__(self, root, ownership_nonce):
        nonce = _identifier(ownership_nonce)
        self.root = Path(root).absolute()
        if self.root.name != nonce or ".." in self.root.parts:
            raise ValueError("Spool directory must be bound to its ownership nonce")
        self._lock = RLock()
        self._entries = {}
        self._validate_root()

    def _validate_root(self):
        for path in (self.root, *self.root.parents):
            info = path.lstat()
            if _is_reparse(info) or not stat.S_ISDIR(info.st_mode):
                raise ValueError("Spool paths must be ordinary directories, not links or reparse points")

    def _budget(self):
        self._validate_root()
        groups, size = set(), 0
        with os.scandir(self.root) as entries:
            for entry in entries:
                info = entry.stat(follow_symlinks=False)
                if _is_reparse(info) or not stat.S_ISREG(info.st_mode):
                    raise ValueError("Spool contains an unsafe file or directory")
                matched = re.fullmatch(r"([0-9a-f]{32})\.(bin|json)", entry.name)
                groups.add(matched.group(1) if matched else entry.name)
                size += info.st_size
                if len(groups) > MAX_ENTRIES or size > MAX_BYTES:
                    raise ValueError("Spool physical capacity is exceeded")
        return groups, size

    def _path(self, identifier, suffix):
        self._validate_root()
        path = self.root / (identifier + suffix)
        if path.exists() or path.is_symlink():
            info = path.lstat()
            if _is_reparse(info) or not stat.S_ISREG(info.st_mode):
                raise ValueError("Staged data path is unsafe")
        return path

    def _verified_payload(self, identifier, descriptor):
        path = self._path(identifier, ".bin")
        payload = _read_file(path, MAX_NATIVE_BYTES)
        if (len(payload) != descriptor["byte_count"]
                or hashlib.sha256(payload).hexdigest() != descriptor["sha256"]):
            raise ValueError("Staged capture length or hash changed")
        return payload

    def put(self, capture_id, capture, *, cancelled=lambda: False):
        identifier = _identifier(capture_id)
        if not isinstance(capture, TraceCapture):
            raise ValueError("Expected validated native trace capture")
        metadata = {
            "native_unit": capture.native_unit, "trace": capture.trace, "identity": capture.identity,
            "read_started_at": capture.read_started_at.isoformat(),
            "read_finished_at": capture.read_finished_at.isoformat(), "elapsed_s": capture.elapsed_s,
            "context_before": asdict(capture.context_before), "context_after": asdict(capture.context_after),
            "consistency": capture.consistency,
        }
        encoded_metadata = json.dumps(metadata, separators=(",", ":"), ensure_ascii=False,
                                      allow_nan=False).encode("utf-8")
        if len(encoded_metadata) > MAX_METADATA:
            raise ValueError("Capture metadata exceeds its bounded envelope")
        pairs = np.empty((len(capture.native_values), 2), dtype="<f8")
        pairs[:, 0], pairs[:, 1] = capture.wavelength_nm, capture.native_values
        payload = pairs.tobytes()
        descriptor = {
            "schema": 1, "kind": "osa_trace", "capture_id": identifier,
            "point_count": len(capture.native_values), "byte_count": len(payload),
            "sha256": hashlib.sha256(payload).hexdigest(), "metadata": metadata,
        }
        encoded = json.dumps(descriptor, separators=(",", ":"), ensure_ascii=False,
                             allow_nan=False).encode("utf-8")
        with self._lock, _pinned_directories(self.root):
            groups, size = self._budget()
            if cancelled():
                raise ValueError("Capture staging cancelled before publication")
            if identifier in self._entries:
                if descriptor != self._entries[identifier]:
                    raise ValueError("Capture identity conflicts with staged content or metadata")
                self._verified_payload(identifier, descriptor)
                return copy.deepcopy(descriptor)
            if identifier in groups or len(groups) >= MAX_ENTRIES or size + len(payload) + len(encoded) > MAX_BYTES:
                raise ValueError("Capture spool capacity or incomplete identity conflict")
            # Exclusive files are intentionally retained on any interrupted write.
            _write_file(self._path(identifier, ".bin"), payload)
            if cancelled():
                raise ValueError("Capture staging cancelled before publication")
            _write_file(self._path(identifier, ".json"), encoded)
            if cancelled():
                raise ValueError("Capture staging cancelled before publication")
            self._entries[identifier] = copy.deepcopy(descriptor)
            return copy.deepcopy(descriptor)

    def read(self, capture_id, offset, length):
        identifier = _identifier(capture_id)
        if type(offset) is not int or type(length) is not int or offset < 0 or not 1 <= length <= MAX_CHUNK:
            raise ValueError("Invalid bounded capture chunk range")
        with self._lock, _pinned_directories(self.root):
            descriptor = self._entries.get(identifier)
            if descriptor is None or offset + length > descriptor["byte_count"]:
                raise ValueError("Unknown capture or out-of-bounds chunk")
            payload = self._verified_payload(identifier, descriptor)
            return {"capture_id": identifier, "offset": offset, "byte_count": length,
                    "sha256": descriptor["sha256"], "data_hex": payload[offset:offset + length].hex()}

    def acknowledge(self, capture_id, sha256):
        identifier = _identifier(capture_id)
        with self._lock, _pinned_directories(self.root):
            descriptor = self._entries.get(identifier)
            if descriptor is None or type(sha256) is not str or sha256 != descriptor["sha256"]:
                raise ValueError("Acknowledgement does not match the complete capture")
            _delete_file(self._path(identifier, ".bin"))
            _delete_file(self._path(identifier, ".json"))
            # A failed deletion retains both the in-memory obligation and whatever
            # files actually remain; budget never subtracts a promised deletion.
            del self._entries[identifier]
