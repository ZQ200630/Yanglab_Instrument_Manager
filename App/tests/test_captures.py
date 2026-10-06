"""Real filesystem staging contracts with literal samples, never hardware data."""
import ctypes
import hashlib
import importlib
import json
import os
from dataclasses import replace
from datetime import datetime, timedelta, timezone
from pathlib import Path
from tempfile import TemporaryDirectory
import subprocess
import unittest
from unittest.mock import patch

import numpy as np

from Code.Utils.osa_trace import TraceCapture, TraceContext


NONCE = "a" * 32
ID = "b" * 32
DBM_BYTES = bytes.fromhex("000000000038984000000000000044c0"
                          "00000000003c98400000000000003ec0")
W_BYTES = bytes.fromhex("00000000003898400000000000000000"
                        "00000000003c9840000000000000f03f")


def capture(*, watts=False, count=2):
    context = TraceContext("ASCII", count, int(watts), int(watts), 0, 0,
                           "TRA", 1.55e-6, 1e-9, 2e-11, 1)
    start = datetime(2026, 10, 6, tzinfo=timezone.utc)
    x = [1550., 1551.] if count == 2 else np.linspace(1550., 1551., count)
    y = [0., 1.] if watts else ([-40., -30.] if count == 2 else np.full(count, -40.))
    return TraceCapture(x, y, "W" if watts else "dBm", "A", "YOKOGAWA,AQ6370E,SN,FW",
                        start, start + timedelta(seconds=1), 1., context, context)


class CaptureSpoolTests(unittest.TestCase):
    def setUp(self):
        self.temporary = TemporaryDirectory(prefix="yang-capture-test-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name) / NONCE
        self.root.mkdir()

    def api(self):
        try:
            return importlib.import_module("App.worker.captures")
        except ModuleNotFoundError as error:
            if error.name != "App.worker.captures":
                raise
            self.fail("Bounded capture staging API is missing")

    def spool(self):
        return self.api().CaptureSpool(self.root, NONCE)

    def test_literal_pairs_keep_native_dbm_watts_and_complete_metadata(self):
        spool = self.spool()
        for watts, identifier, payload in ((False, ID, DBM_BYTES), (True, "c" * 32, W_BYTES)):
            with self.subTest(watts=watts):
                descriptor = spool.put(identifier, capture(watts=watts))
                chunk = spool.read(identifier, 0, 32)
                self.assertEqual(bytes.fromhex(chunk["data_hex"]), payload)
                self.assertEqual(chunk["capture_id"], identifier)
                self.assertEqual(chunk["offset"], 0)
                self.assertEqual(chunk["byte_count"], 32)
                self.assertEqual(descriptor["sha256"], hashlib.sha256(payload).hexdigest())
                self.assertEqual(chunk["sha256"], descriptor["sha256"])
                self.assertEqual(descriptor["schema"], 1)
                self.assertEqual(descriptor["kind"], "osa_trace")
                self.assertEqual(descriptor["point_count"], 2)
                self.assertEqual(descriptor["byte_count"], 32)
                metadata = descriptor["metadata"]
                self.assertEqual(metadata["native_unit"], "W" if watts else "dBm")
                self.assertEqual(metadata["identity"], "YOKOGAWA,AQ6370E,SN,FW")
                self.assertEqual(metadata["trace"], "A")
                self.assertEqual(metadata["read_started_at"], "2026-10-06T00:00:00+00:00")
                self.assertEqual(metadata["read_finished_at"], "2026-10-06T00:00:01+00:00")
                self.assertEqual(metadata["elapsed_s"], 1.)
                self.assertEqual(metadata["consistency"], "unproven")
                self.assertEqual(metadata["context_before"], metadata["context_after"])
                self.assertEqual(metadata["context_before"]["center_m"], 1.55e-6)

    def test_maximum_native_trace_uses_small_descriptor_and_bounded_chunks(self):
        spool = self.spool()
        descriptor = spool.put(ID, capture(count=200001))
        self.assertEqual(descriptor["byte_count"], 3200016)
        self.assertLess(len(json.dumps({"result": descriptor}).encode()), 65536)
        chunk = spool.read(ID, 0, 16384)
        self.assertEqual(len(chunk["data_hex"]), 32768)
        self.assertLess(len(json.dumps({"result": chunk}).encode()), 65536)
        with self.assertRaises(ValueError):
            spool.read(ID, 0, 16385)

    def test_invalid_ids_do_not_create_files_or_read_paths(self):
        spool = self.spool()
        for invalid in ("../escape", "B" * 32, "x" * 32, "", True, "b" * 31):
            with self.subTest(invalid=invalid):
                with self.assertRaises(ValueError):
                    spool.put(invalid, capture())
                with self.assertRaises(ValueError):
                    spool.read(invalid, 0, 1)
        self.assertEqual(list(self.root.iterdir()), [])

    def test_invalid_ranges_and_unknown_ids_fail_without_partial_chunks(self):
        spool = self.spool()
        spool.put(ID, capture())
        for offset, length in ((-1, 1), (0, 0), (True, 1), (0, False), (0., 1), (31, 2), (32, 1)):
            with self.subTest(offset=offset, length=length), self.assertRaises(ValueError):
                spool.read(ID, offset, length)
        with self.assertRaises(ValueError):
            spool.read("c" * 32, 0, 1)

    def test_partial_range_returns_exact_original_bytes(self):
        spool = self.spool()
        spool.put(ID, capture())
        self.assertEqual(spool.read(ID, 8, 16)["data_hex"], "00000000000044c000000000003c9840")

    def test_duplicate_id_requires_identical_content_and_metadata(self):
        spool = self.spool()
        descriptor = spool.put(ID, capture())
        self.assertEqual(spool.put(ID, capture()), descriptor)
        for changed in (replace(capture(), native_values=[-50., -30.]),
                        replace(capture(), identity="YOKOGAWA,AQ6370E,OTHER,FW")):
            with self.subTest(identity=changed.identity), self.assertRaises(ValueError):
                spool.put(ID, changed)
        self.assertEqual(bytes.fromhex(spool.read(ID, 0, 32)["data_hex"]), DBM_BYTES)

    def test_caller_descriptor_mutation_never_rewrites_staged_identity_or_hash(self):
        spool = self.spool()
        descriptor = spool.put(ID, capture())
        descriptor["metadata"]["identity"] = "OTHER"
        descriptor["sha256"] = "0" * 64
        original = spool.put(ID, capture())
        self.assertEqual(original["metadata"]["identity"], "YOKOGAWA,AQ6370E,SN,FW")
        self.assertEqual(original["sha256"], hashlib.sha256(DBM_BYTES).hexdigest())

    def test_entry_capacity_counts_orphans_and_never_removes_them(self):
        spool = self.spool()
        for number in range(32):
            (self.root / f"orphan-{number}").touch()
        with self.assertRaises(ValueError):
            spool.put(ID, capture())
        self.assertEqual(len(list(self.root.iterdir())), 32)

    def test_byte_capacity_counts_unindexed_file_lengths(self):
        spool = self.spool()
        with (self.root / "orphan").open("wb") as file:
            file.truncate(128 * 1024 * 1024)
        with self.assertRaises(ValueError):
            spool.put(ID, capture())
        self.assertEqual((self.root / "orphan").stat().st_size, 128 * 1024 * 1024)

    def test_wrong_ack_hash_retains_data_and_matching_ack_removes_only_its_entry(self):
        spool = self.spool()
        descriptor = spool.put(ID, capture())
        other = spool.put("c" * 32, capture())
        with self.assertRaises(ValueError):
            spool.acknowledge(ID, "0" * 64)
        self.assertEqual(bytes.fromhex(spool.read(ID, 0, 32)["data_hex"]), DBM_BYTES)
        spool.acknowledge(ID, descriptor["sha256"])
        with self.assertRaises(ValueError):
            spool.read(ID, 0, 1)
        self.assertEqual(spool.put("c" * 32, capture()), other)

    def test_failed_ack_deletion_retains_entry_capacity_until_explicit_retry(self):
        spool = self.spool()
        descriptor = spool.put(ID, capture())
        for number in range(31):
            (self.root / f"orphan-{number}").touch()
        with patch("App.worker.captures._delete_file", side_effect=PermissionError("retained staged file")):
            with self.assertRaises(PermissionError):
                spool.acknowledge(ID, descriptor["sha256"])
        with self.assertRaises(ValueError):
            spool.put("c" * 32, capture())
        spool.acknowledge(ID, descriptor["sha256"])
        self.assertEqual(spool.put("c" * 32, capture())["point_count"], 2)

    def test_cancelled_write_is_not_published_or_reused_as_a_complete_capture(self):
        spool = self.spool()
        checks = []
        def cancelled():
            checks.append(True)
            return len(checks) > 1
        with self.assertRaises(ValueError):
            spool.put(ID, capture(), cancelled=cancelled)
        with self.assertRaises(ValueError):
            spool.read(ID, 0, 1)
        self.assertGreater(len(list(self.root.iterdir())), 0)
        with self.assertRaises(ValueError):
            spool.put(ID, capture())

    def test_interrupted_disk_write_closes_handle_but_keeps_unpublished_orphan(self):
        spool = self.spool()
        import os
        real_write = os.write
        def interrupted(handle, data):
            real_write(handle, data[:8])
            raise KeyboardInterrupt("interrupted staging")
        with patch("os.write", side_effect=interrupted), self.assertRaises(KeyboardInterrupt):
            spool.put(ID, capture())
        with self.assertRaises(ValueError):
            spool.read(ID, 0, 1)
        # No locked leftover handle: the temporary-directory cleanup can unlink.
        files = list(self.root.iterdir())
        self.assertEqual(len(files), 1)
        self.assertEqual(files[0].stat().st_size, 8)

    def test_changed_or_truncated_staged_file_never_returns_a_valid_chunk(self):
        spool = self.spool()
        for identifier, changed in ((ID, b"\x01" + DBM_BYTES[1:]), ("c" * 32, DBM_BYTES[:8])):
            with self.subTest(byte_count=len(changed)):
                spool.put(identifier, capture())
                (self.root / f"{identifier}.bin").write_bytes(changed)
                with self.assertRaises(ValueError):
                    spool.read(identifier, 0, 1)

    def test_oversized_metadata_is_rejected_before_creating_payload_files(self):
        spool = self.spool()
        with self.assertRaises(ValueError):
            spool.put(ID, replace(capture(), identity="YOKOGAWA," + "X" * 8192))
        self.assertEqual(list(self.root.iterdir()), [])

    def test_nonce_mismatch_cannot_borrow_another_workers_spool(self):
        with self.assertRaises(ValueError):
            self.api().CaptureSpool(self.root, "c" * 32)

    def test_existing_incomplete_files_are_accounted_but_never_trusted_after_reopen(self):
        (self.root / f"{ID}.bin").write_bytes(DBM_BYTES)
        spool = self.spool()
        with self.assertRaises(ValueError):
            spool.read(ID, 0, 1)
        with self.assertRaises(ValueError):
            spool.put(ID, capture())

    def test_reparse_parent_cannot_redirect_worker_spool_to_another_directory(self):
        alias = Path(self.temporary.name) / "alias"
        if os.name == "nt":
            subprocess.run([os.environ["COMSPEC"], "/c", "mklink", "/J",
                            str(alias), self.temporary.name], check=True,
                           stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        else:
            alias.symlink_to(self.temporary.name, target_is_directory=True)
        try:
            with self.assertRaisesRegex(ValueError, "reparse"):
                self.api().CaptureSpool(alias / NONCE, NONCE)
        finally:
            # Remove only this fixture's link, never recurse into its target.
            if os.name == "nt":
                os.rmdir(alias)
            else:
                alias.unlink()

    def _assert_root_pinned_at_path_access(self, spool, action):
        original = spool._path
        attempted = []
        def replace_checked_directory(identifier, suffix):
            path = original(identifier, suffix)
            attempted.append(suffix)
            # A pathname check is not a transaction guard. This actual rename
            # would allow a junction to replace the checked private directory.
            with self.assertRaises(OSError):
                self.root.rename(self.root.with_name("displaced"))
            return path
        with patch.object(spool, "_path", side_effect=replace_checked_directory):
            action()
        self.assertTrue(attempted)

    def test_put_pins_checked_directory_until_both_files_are_published(self):
        spool = self.spool()
        self._assert_root_pinned_at_path_access(spool, lambda: spool.put(ID, capture()))
        self.assertEqual(bytes.fromhex(spool.read(ID, 0, 32)["data_hex"]), DBM_BYTES)

    def test_read_pins_checked_directory_until_verified_bytes_are_returned(self):
        spool = self.spool()
        spool.put(ID, capture())
        self._assert_root_pinned_at_path_access(spool, lambda: spool.read(ID, 0, 32))

    def test_ack_pins_checked_directory_until_exact_owned_files_are_deleted(self):
        spool = self.spool()
        descriptor = spool.put(ID, capture())
        self._assert_root_pinned_at_path_access(
            spool, lambda: spool.acknowledge(ID, descriptor["sha256"]))
        self.assertEqual(list(self.root.iterdir()), [])

    def test_opened_file_cannot_be_replaced_during_write_read_or_ack(self):
        api = self.api()
        spool = self.spool()
        checked = []
        real_info = api._file_info
        def check_handle_then_replace(kernel, handle, directory):
            info = real_info(kernel, handle, directory)
            if not directory:
                paths = list(self.root.iterdir())
                # DELETE-open tests file sharing directly; pathname rename alone
                # could also be blocked by the separate parent-directory guard.
                denied = []
                for path in paths:
                    candidate = kernel.CreateFileW(str(path), 0x10080, 7, None, 3, 0x200000, None)
                    if candidate == ctypes.c_void_p(-1).value:
                        self.assertEqual(ctypes.get_last_error(), 32)
                        denied.append(path)
                    else:
                        self.assertTrue(kernel.CloseHandle(candidate))
                self.assertTrue(denied)
                checked.extend(denied)
            return info
        with patch.object(api, "_file_info", side_effect=check_handle_then_replace):
            descriptor = spool.put(ID, capture())
            self.assertEqual(bytes.fromhex(spool.read(ID, 0, 32)["data_hex"]), DBM_BYTES)
            spool.acknowledge(ID, descriptor["sha256"])
        self.assertGreaterEqual(len(checked), 3)
        self.assertEqual(list(self.root.iterdir()), [])

    def test_ack_rejects_final_path_junction_inserted_after_path_check(self):
        spool = self.spool()
        descriptor = spool.put(ID, capture())
        target = Path(self.temporary.name) / "operator-files"
        target.mkdir()
        sentinel = target / "keep.bin"
        sentinel.write_bytes(b"operator-owned fixture")
        path = self.root / (ID + ".bin")
        original = spool._path
        def replace_after_check(identifier, suffix):
            checked = original(identifier, suffix)
            if suffix == ".bin":
                checked.rename(checked.with_suffix(".orphan"))
                subprocess.run([os.environ["COMSPEC"], "/c", "mklink", "/J",
                                str(checked), str(target)], check=True,
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            return checked
        try:
            with patch.object(spool, "_path", side_effect=replace_after_check):
                with self.assertRaises((OSError, ValueError)):
                    spool.acknowledge(ID, descriptor["sha256"])
            self.assertEqual(sentinel.read_bytes(), b"operator-owned fixture")
            self.assertIn(ID, spool._entries)
        finally:
            if path.is_dir():
                os.rmdir(path)  # Remove only the fixture junction, not its target.


if __name__ == "__main__":
    unittest.main()
