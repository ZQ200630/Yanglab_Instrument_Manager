"""Offline runtime-input validation, never a selectable instrument backend."""

import base64
import copy
import hashlib
import importlib
import importlib.util
import io
import json
import os
import shutil
import stat
import subprocess
import sys
import tempfile
import unittest
import zipfile
from pathlib import Path
from unittest.mock import patch


def package(name, version, digest, *, requires=(), tags=('py3-none-any',)):
    return {'filename': f'{name}-{version}-{tags[0]}.whl', 'name': name, 'version': version,
            'sha256': digest, 'requires_python': '>=3.10', 'requires_dist': list(requires),
            'tags': list(tags), 'licenses': [f'{name}-{version}.dist-info/LICENSE']}


def wheel_bytes(*, changed_record=False, include_record=True):
    """Literal owned wheel boundary; RECORD has real independent content hashes."""
    entries = {'example/__init__.py': b'# Offline wheel parser fixture; not imported.\n',
               'example-1.0.dist-info/METADATA': b'Metadata-Version: 2.1\nName: example\nVersion: 1.0\nRequires-Python: >=3.10\nRequires-Dist: support>=2; python_version >= "3.13"\n',
               'example-1.0.dist-info/WHEEL': b'Wheel-Version: 1.0\nRoot-Is-Purelib: true\nTag: py3-none-any\n',
               'example-1.0.dist-info/LICENSE': b'Owned finite test fixture.\n'}
    rows = []
    for name, content in entries.items():
        digest = base64.urlsafe_b64encode(hashlib.sha256(content).digest()).decode().rstrip('=')
        rows.append(f'{name},sha256={digest},{len(content)}\n')
    rows.append('example-1.0.dist-info/RECORD,,\n')
    if include_record:
        entries['example-1.0.dist-info/RECORD'] = ''.join(rows).encode()
    if changed_record:
        entries['example/__init__.py'] = b'# Modified after RECORD was generated.\n'
    stream = io.BytesIO()
    with zipfile.ZipFile(stream, 'w') as archive:
        for name, content in entries.items():
            archive.writestr(name, content)
    return stream.getvalue()


class RuntimeLockTests(unittest.TestCase):
    def setUp(self):
        self.assertIsNotNone(importlib.util.find_spec('App.scripts.runtime_lock'),
                             'Missing production runtime lock validation')
        self.module = importlib.import_module('App.scripts.runtime_lock')
        self.error = self.module.RuntimeInputError
        self.validate = self.module.validate_lock
        self.lock = (f'example==1.0 --hash=sha256:{"a" * 64}\n'
                     f'support==2.0 --hash=sha256:{"b" * 64}\n').encode()
        self.wheels = [package('example', '1.0', 'a' * 64, requires=('support>=2',)),
                       package('support', '2.0', 'b' * 64)]

    def test_locked_versions_hashes_and_target_are_detached(self):
        result = self.validate(self.lock, self.wheels)
        self.assertEqual(result['target'], {'python': '3.13.16', 'implementation': 'cp',
                                          'abi': 'cp313', 'platform': 'win_amd64'})
        self.assertEqual([(p['name'], p['version']) for p in result['packages']],
                         [('example', '1.0'), ('support', '2.0')])
        result['packages'][0]['requires_dist'].append('unreviewed')
        self.assertEqual(self.wheels[0]['requires_dist'], ['support>=2'])

    def test_missing_transitive_wheel_does_not_satisfy_complete_lock(self):
        for lock, metadata in [(self.lock, self.wheels[:1]),
                               (self.lock.splitlines(keepends=True)[0], self.wheels[:1])]:
            with self.subTest(lock=lock), self.assertRaises(self.error):
                self.validate(lock, metadata)

    def test_dependency_markers_use_target_python_and_windows_not_build_machine(self):
        metadata = [package('example', '1.0', 'a' * 64, requires=(
            'support>=2; python_version >= "3.13"',
            'unneeded; sys_platform == "linux"', 'pyvisa-sim; extra == "tests"'))]
        with self.assertRaises(self.error):
            self.validate(self.lock.splitlines(keepends=True)[0], metadata)
        self.assertEqual(len(self.validate(self.lock, metadata + self.wheels[1:])['packages']), 2)

    def test_marker_mismatch_requires_python_and_unsatisfied_versions_reject(self):
        for change in ({'requires_python': '<3.13'}, {'requires_dist': ['support>=3']},
                       {'requires_dist': ['support @ https://example.test/a.whl']},
                       {'requires_dist': ['support[extra]>=2']}):
            metadata = copy.deepcopy(self.wheels)
            metadata[0].update(change)
            with self.subTest(change=change), self.assertRaises(self.error):
                self.validate(self.lock, metadata)

    def test_wrong_hash_duplicate_version_and_extra_wheel_reject(self):
        metadata = copy.deepcopy(self.wheels)
        metadata[0]['sha256'] = 'c' * 64
        for wheels in (metadata, self.wheels + self.wheels[:1],
                       self.wheels + [package('extra', '1.0', 'd' * 64)]):
            with self.subTest(wheels=wheels), self.assertRaises(self.error):
                self.validate(self.lock, wheels)

    def test_unpinned_duplicate_url_sdist_and_hashless_lock_reject(self):
        for text in ('example>=1', 'example==1.0', '-r other.txt',
                     f'example==1.0 --hash=md5:{"a" * 32}',
                     'example @ https://example.test/a.whl',
                     self.lock.decode() + self.lock.decode().splitlines()[0],
                     f'pyvisa-sim==1.0 --hash=sha256:{"a" * 64}'):
            with self.subTest(text=text), self.assertRaises(self.error):
                self.validate(text.encode(), self.wheels)

    def test_unsupported_abi_architecture_and_sdist_reject(self):
        for tag in ('cp313-cp313t-win_amd64', 'cp310-cp310-win_amd64',
                    'cp313-cp313-win32', 'cp313-cp313-linux_x86_64'):
            metadata = copy.deepcopy(self.wheels)
            metadata[0].update(package('example', '1.0', 'a' * 64, tags=(tag,)))
            with self.subTest(tag=tag), self.assertRaises(self.error):
                self.validate(self.lock, metadata)
        metadata = copy.deepcopy(self.wheels)
        metadata[0]['filename'] = 'example-1.0.tar.gz'
        with self.assertRaises(self.error):
            self.validate(self.lock, metadata)

    def test_cp313_and_backward_stable_abi_windows_wheels_are_accepted(self):
        for tag in ('cp313-cp313-win_amd64', 'cp313-abi3-win_amd64',
                    'cp312-abi3-win_amd64'):
            metadata = copy.deepcopy(self.wheels)
            metadata[1] = package('support', '2.0', 'b' * 64, tags=(tag,))
            with self.subTest(tag=tag):
                self.assertEqual(len(self.validate(self.lock, metadata)['packages']), 2)

    def test_malformed_metadata_and_input_bounds_reject_with_domain_error(self):
        for metadata in ([], None, [{}], ['not metadata'], self.wheels * 65):
            with self.subTest(metadata=type(metadata)), self.assertRaises(self.error):
                self.validate(self.lock, metadata)
        for lock in (None, b'\xff', b'x' * 65537):
            with self.subTest(lock=type(lock)), self.assertRaises(self.error):
                self.validate(lock, self.wheels)

    def test_wheel_name_version_tag_and_declared_metadata_must_agree(self):
        for change in ({'name': 'other'}, {'version': '1.1'}, {'tags': ['cp313-cp313-win_amd64']},
                       {'filename': '../example-1.0-py3-none-any.whl'}, {'licenses': None}):
            metadata = copy.deepcopy(self.wheels)
            metadata[0].update(change)
            with self.subTest(change=change), self.assertRaises(self.error):
                self.validate(self.lock, metadata)

    def test_missing_in_wheel_license_is_explicit_not_fabricated_as_qualified(self):
        metadata = copy.deepcopy(self.wheels)
        metadata[0]['licenses'] = []
        try:
            result = self.validate(self.lock, metadata)
        except self.error as error:
            self.fail('Missing in-wheel license must remain explicit for assembly: ' + str(error))
        self.assertEqual(result['missing_license_packages'], ['example'])
        self.assertEqual(result['packages'][0]['licenses'], [])
        self.assertEqual(self.validate(self.lock, self.wheels)['missing_license_packages'], [])


class RuntimeWheelInspectionTests(unittest.TestCase):
    def setUp(self):
        self.assertIsNotNone(importlib.util.find_spec('App.scripts.runtime_lock'),
                             'Missing production runtime wheel inspection')
        self.module = importlib.import_module('App.scripts.runtime_lock')

    def test_inspects_real_zip_metadata_record_hashes_and_licenses(self):
        with tempfile.TemporaryDirectory(prefix='yang-wheel-') as temporary:
            wheel = Path(temporary) / 'example-1.0-py3-none-any.whl'
            payload = wheel_bytes()
            wheel.write_bytes(payload)
            metadata = self.module.inspect_wheel(wheel)
            self.assertEqual(metadata['sha256'], hashlib.sha256(payload).hexdigest())
            self.assertEqual(metadata['requires_dist'], ['support>=2; python_version >= "3.13"'])
            self.assertEqual(metadata['licenses'], ['example-1.0.dist-info/LICENSE'])
            self.assertEqual((metadata['name'], metadata['version'], metadata['tags']),
                             ('example', '1.0', ['py3-none-any']))

    def test_missing_record_or_modified_record_content_rejects(self):
        with tempfile.TemporaryDirectory(prefix='yang-wheel-') as temporary:
            wheel = Path(temporary) / 'example-1.0-py3-none-any.whl'
            for options in ({'include_record': False}, {'changed_record': True}):
                wheel.write_bytes(wheel_bytes(**options))
                with self.subTest(options=options), self.assertRaises(self.module.RuntimeInputError):
                    self.module.inspect_wheel(wheel)

    def test_traversal_case_collisions_and_symlinks_reject_without_extraction(self):
        with tempfile.TemporaryDirectory(prefix='yang-wheel-') as temporary:
            wheel = Path(temporary) / 'example-1.0-py3-none-any.whl'
            for names in (('../escape',), ('Package/a.py', 'package/A.py'), ('link',)):
                with zipfile.ZipFile(wheel, 'w') as archive:
                    for name in names:
                        entry = zipfile.ZipInfo(name)
                        if name == 'link':
                            entry.external_attr = 0o120777 << 16
                        archive.writestr(entry, b'fixture')
                with self.subTest(names=names), self.assertRaises(self.module.RuntimeInputError):
                    self.module.inspect_wheel(wheel)
            self.assertEqual([p.name for p in Path(temporary).iterdir()], [wheel.name])


def assembly_wheel(name='example', *, license=True, extra=None):
    """Owned, non-imported packaging inputs, not an instrument emulator."""
    entries = {f'{name}/__init__.py': b'# owned wheel fixture\n',
               f'{name}-1.0.dist-info/METADATA':
                   f'Metadata-Version: 2.1\nName: {name}\nVersion: 1.0\nRequires-Python: >=3.10\n'.encode(),
               f'{name}-1.0.dist-info/WHEEL':
                   b'Wheel-Version: 1.0\nRoot-Is-Purelib: true\nTag: py3-none-any\n'}
    if license:
        entries[f'{name}-1.0.dist-info/LICENSE'] = b'Owned fixture notice.\n'
    entries.update(extra or {})
    rows = []
    for path, content in entries.items():
        digest = base64.urlsafe_b64encode(hashlib.sha256(content).digest()).decode().rstrip('=')
        rows.append(f'{path},sha256={digest},{len(content)}\n')
    rows.append(f'{name}-1.0.dist-info/RECORD,,\n')
    entries[f'{name}-1.0.dist-info/RECORD'] = ''.join(rows).encode()
    stream = io.BytesIO()
    with zipfile.ZipFile(stream, 'w') as archive:
        for path, content in entries.items():
            archive.writestr(path, content)
    return stream.getvalue()


class RuntimeAssemblyTests(unittest.TestCase):
    """Catch admission without validation, source drift and destructive failures."""
    def setUp(self):
        self.assertIsNotNone(importlib.util.find_spec('App.scripts.runtime_assemble'),
                             'Missing production private runtime assembly')
        self.module = importlib.import_module('App.scripts.runtime_assemble')
        self.error = self.module.RuntimeInputError
        self.temporary = tempfile.TemporaryDirectory(prefix='runtime-组装 ')
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.repo = self.root / 'source'
        self.repo.mkdir()
        self.git = shutil.which('git')
        self.assertIsNotNone(self.git, 'Runtime builds require Git for exact source provenance')
        self.git_call('init', '-q')
        self.inputs = self.root / 'inputs'
        self.wheels = self.inputs / 'wheels'
        self.wheels.mkdir(parents=True)
        self.catalog = self.repo / 'App/runtime'
        self.catalog.mkdir(parents=True)
        for name, content in {'Code/__init__.py': b'# source fixture\n',
                              'Code/Utils/__init__.py': b'# utils fixture\n',
                              'App/__init__.py': b'# app fixture\n',
                              'App/worker/main.py': b'# source worker fixture; never executed\n',
                              'Config/example.json': b'{"safe":true}\n'}.items():
            path = self.repo / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(content)
        self.archive = self.inputs / 'python-3.13.16-embed-amd64.zip'
        self.archive_entries = {'python.exe': b'MZ owned non-executable fixture',
                                'python313.zip': b'owned standard library placeholder',
                                'python313.dll': b'owned DLL placeholder',
                                'LICENSE.txt': b'Owned interpreter fixture notice.\n',
                                'python313._pth': b'python313.zip\n.\n#import site\n'}
        self.replace_archive()
        (self.catalog / 'python313._pth').write_bytes(b'python313.zip\n.\nLib/site-packages\n..\n')
        (self.catalog / 'license-files.json').write_text('{"v":1,"files":[]}', encoding='utf-8')
        self.lock = self.catalog / 'requirements-win-x64.lock'
        self.set_wheels({'example': assembly_wheel(extra={
            'example/tests/test_unused.py': b'# excluded upstream fixture\n',
            'example/testing/__init__.py': b'# retained public support API\n',
            'example-1.0.data/purelib/example/extra.py': b'# purelib mapping\n'})})
        self.destination = self.root / 'installed 设备'

    def git_call(self, *args):
        result = subprocess.run([self.git, '-c', 'user.name=Fixture', '-c',
            'user.email=fixture@example.invalid', '-c', 'commit.gpgsign=false',
            '-C', str(self.repo), *args], capture_output=True, check=True)
        return result.stdout.decode('utf-8').strip()

    def commit(self):
        self.git_call('add', '--all')
        self.git_call('commit', '-qm', 'Owned packaging inputs', '--allow-empty')
        self.commit_id = self.git_call('rev-parse', 'HEAD')

    def replace_archive(self, *, link=False):
        with zipfile.ZipFile(self.archive, 'w') as archive:
            for name, content in self.archive_entries.items():
                entry = zipfile.ZipInfo(name)
                if link and name == 'python.exe':
                    entry.external_attr = (stat.S_IFLNK | 0o777) << 16
                archive.writestr(entry, content)
        metadata = {'v': 1, 'version': '3.13.16', 'implementation': 'cp', 'abi': 'cp313',
                    'platform': 'win_amd64', 'archive': self.archive.name,
                    'url': 'https://example.invalid/owned-fixture.zip',
                    'sha256': hashlib.sha256(self.archive.read_bytes()).hexdigest(),
                    'provenance': 'https://example.invalid/owned-fixture'}
        (self.catalog / 'interpreter.json').write_text(json.dumps(metadata), encoding='utf-8')

    def set_wheels(self, items):
        for path in self.wheels.iterdir():
            path.unlink()
        pins = []
        for name, content in items.items():
            (self.wheels / f'{name}-1.0-py3-none-any.whl').write_bytes(content)
            pins.append(f'{name}==1.0 --hash=sha256:{hashlib.sha256(content).hexdigest()}\n')
        self.lock.write_text(''.join(pins), encoding='utf-8')
        self.commit()

    def assemble(self, **kwargs):
        return self.module.assemble_runtime(self.archive, self.wheels, self.lock,
            self.destination, self.commit_id, source_root=self.repo, git_executable=self.git, **kwargs)

    def assert_rejected_cleanly(self):
        originals = {str(path.relative_to(self.inputs)): path.read_bytes()
                     for path in self.inputs.rglob('*') if path.is_file()}
        with self.assertRaises(self.error):
            self.assemble()
        self.assertFalse(self.destination.exists())
        self.assertEqual(originals, {str(path.relative_to(self.inputs)): path.read_bytes()
                                    for path in self.inputs.rglob('*') if path.is_file()})
        self.assertFalse(list(self.root.glob('.runtime-stage-*')))

    def test_new_unicode_destination_contains_exact_files_hashes_and_safe_search_path(self):
        result = self.assemble()
        manifest_bytes = (self.destination / 'runtime-manifest.json').read_bytes()
        self.assertEqual(json.loads(manifest_bytes), result)
        self.assertEqual(result['v'], 1)
        self.assertEqual(result['source_commit'], self.commit_id)
        self.assertEqual((self.destination / 'runtime/python313._pth').read_bytes(),
                         b'python313.zip\n.\nLib/site-packages\n..\n')
        self.assertEqual((self.destination / 'App/worker/main.py').read_bytes(),
                         b'# source worker fixture; never executed\n')
        self.assertEqual((self.destination / 'runtime/Lib/site-packages/example/extra.py').read_bytes(),
                         b'# purelib mapping\n')
        self.assertTrue((self.destination / 'runtime/Lib/site-packages/example/testing/__init__.py').is_file())
        self.assertFalse((self.destination / 'runtime/Lib/site-packages/example/tests').exists())
        actual = {path.relative_to(self.destination).as_posix(): path for path in
                  self.destination.rglob('*') if path.is_file() and path.name != 'runtime-manifest.json'}
        self.assertEqual(set(actual), {entry['path'] for entry in result['files']})
        for entry in result['files']:
            data = actual[entry['path']].read_bytes()
            self.assertEqual(entry['size'], len(data))
            self.assertEqual(entry['sha256'], hashlib.sha256(data).hexdigest())
        # Regenerated RECORD must describe the installed mapping, not removed upstream tests.
        record = (self.destination / 'runtime/Lib/site-packages/example-1.0.dist-info/RECORD').read_text()
        self.assertIn('example/extra.py,sha256=', record)
        self.assertNotIn('example/tests/', record)
        self.assertEqual(result['excluded'], [{'wheel': 'example-1.0-py3-none-any.whl',
                                              'path': 'example/tests/test_unused.py',
                                              'reason': 'upstream-test-or-example'}])

    def test_source_uses_named_git_commit_not_modified_or_untracked_worktree_files(self):
        (self.repo / 'App/worker/main.py').write_bytes(b'# unrelated working change\n')
        (self.repo / 'App/worker/untracked.py').write_bytes(b'# not in source commit\n')
        result = self.assemble()
        self.assertEqual(result['source_kind'], 'git-commit')
        self.assertEqual((self.destination / 'App/worker/main.py').read_bytes(),
                         b'# source worker fixture; never executed\n')
        self.assertFalse((self.destination / 'App/worker/untracked.py').exists())
        self.assertEqual((self.repo / 'App/worker/main.py').read_bytes(), b'# unrelated working change\n')

    def test_windows_catalog_checkout_line_endings_keep_committed_payload_bytes(self):
        for path in self.catalog.iterdir():
            if path.name in {'interpreter.json', 'license-files.json', 'python313._pth',
                             'requirements-win-x64.lock'}:
                path.write_bytes(path.read_bytes().replace(b'\r\n', b'\n').replace(b'\n', b'\r\n'))
        result = self.assemble()
        self.assertEqual(result['source_commit'], self.commit_id)
        self.assertEqual((self.destination / 'runtime/python313._pth').read_bytes(),
                         b'python313.zip\n.\nLib/site-packages\n..\n')

    def test_existing_destination_file_or_directory_is_never_overwritten(self):
        self.destination.write_bytes(b'preserve existing file')
        with self.assertRaises(self.error):
            self.assemble()
        self.assertEqual(self.destination.read_bytes(), b'preserve existing file')
        self.destination.unlink()
        self.destination.mkdir()
        (self.destination / 'sentinel').write_bytes(b'preserve existing folder')
        with self.assertRaises(self.error):
            self.assemble()
        self.assertEqual((self.destination / 'sentinel').read_bytes(), b'preserve existing folder')

    def test_archive_hash_and_committed_lock_metadata_must_match(self):
        self.archive.write_bytes(self.archive.read_bytes() + b'modified input')
        self.assert_rejected_cleanly()
        self.replace_archive()
        self.commit()
        self.lock.write_bytes(b'changed uncommitted lock')
        self.assert_rejected_cleanly()

    def test_invalid_source_commit_rejects_without_creating_a_destination(self):
        self.commit_id = '0' * 40
        self.assert_rejected_cleanly()

    def test_archive_traversal_case_collision_and_link_entries_do_not_escape(self):
        for name, link in [('../escaped.txt', False), ('PYTHON.exe', False), ('python.exe', True)]:
            with self.subTest(name=name, link=link):
                original = self.archive_entries.copy()
                self.archive_entries[name] = b'unsafe entry'
                self.replace_archive(link=link)
                self.commit()
                self.assert_rejected_cleanly()
                self.assertFalse((self.root / 'escaped.txt').exists())
                self.archive_entries = original

    def test_aggregate_file_and_byte_limits_apply_before_destination_admission(self):
        for limit, value in [('MAX_FILES', 3), ('MAX_EXPANDED', 200)]:
            with self.subTest(limit=limit), patch.object(self.module, limit, value):
                self.assert_rejected_cleanly()

    def test_colliding_wheel_destinations_and_executable_pth_reject(self):
        self.set_wheels({'example': assembly_wheel(), 'support': assembly_wheel('support',
                         extra={'example/__init__.py': b'# collision\n'})})
        self.assert_rejected_cleanly()
        self.set_wheels({'example': assembly_wheel(extra={'startup.pth': b'import os\n'})})
        self.assert_rejected_cleanly()

    def test_external_notice_is_bound_to_package_wheel_and_exact_notice_bytes(self):
        self.set_wheels({'example': assembly_wheel(license=False)})
        self.assert_rejected_cleanly()
        notice = self.inputs / 'licenses/example-1.0.txt'
        notice.parent.mkdir()
        notice.write_bytes(b'Owned external license.\n')
        catalog = {'v': 1, 'files': [{'package': 'example', 'version': '1.0',
            'wheel_sha256': hashlib.sha256(next(self.wheels.iterdir()).read_bytes()).hexdigest(),
            'filename': 'licenses/example-1.0.txt', 'url': 'https://example.invalid/license',
            'sha256': hashlib.sha256(notice.read_bytes()).hexdigest()}]}
        (self.catalog / 'license-files.json').write_text(json.dumps(catalog), encoding='utf-8')
        self.commit()
        notice.write_bytes(b'Wrong notice')
        self.assert_rejected_cleanly()
        notice.write_bytes(b'Owned external license.\n')
        result = self.assemble()
        self.assertEqual((self.destination / 'licenses/example-1.0.txt').read_bytes(),
                         b'Owned external license.\n')
        self.assertEqual(result['external_notices'], catalog['files'])

    def test_partial_extraction_failure_removes_only_owned_stage_and_preserves_inputs(self):
        writer = self.module._write_payload_file
        calls = []
        def failing_writer(*args, **kwargs):
            calls.append(args)
            if len(calls) == 2:
                raise OSError('Owned finite disk failure')
            return writer(*args, **kwargs)
        with patch.object(self.module, '_write_payload_file', failing_writer):
            self.assert_rejected_cleanly()
        self.assertEqual(len(calls), 2)

    def test_repeated_assembly_has_identical_build_identity_and_inventory(self):
        first = self.assemble()
        self.destination = self.root / 'second independent destination'
        second = self.assemble()
        self.assertEqual(first, second)

    def test_filesystem_hardlinked_archive_is_not_an_admissible_input(self):
        linked = self.inputs / 'shared-archive.zip'
        os.link(self.archive, linked)
        self.assert_rejected_cleanly()

    def test_unlisted_or_modified_staged_file_is_rejected_before_admission(self):
        writer = self.module._write_payload_file
        for extra in (True, False):
            self.destination = self.root / ('extra-file' if extra else 'modified-file')
            touched = []
            def corrupt_writer(stage, relative, stream):
                result = writer(stage, relative, stream)
                if not touched:
                    touched.append(True)
                    path = stage / 'unexpected.py' if extra else stage / relative
                    path.write_bytes(b'# injected finite corruption\n')
                return result
            with self.subTest(extra=extra), patch.object(self.module, '_write_payload_file', corrupt_writer):
                self.assert_rejected_cleanly()

    def test_destination_created_during_final_rename_is_preserved(self):
        rename = Path.rename
        def raced_rename(path, target):
            target.mkdir()
            (target / 'sentinel').write_bytes(b'Concurrent owner destination')
            return rename(path, target)
        with patch.object(Path, 'rename', raced_rename), self.assertRaises(self.error):
            self.assemble()
        self.assertEqual((self.destination / 'sentinel').read_bytes(), b'Concurrent owner destination')
        self.assertFalse(list(self.root.glob('.runtime-stage-*')))

    def test_powershell_build_entry_uses_explicit_visa_and_admits_new_payload(self):
        script = Path(__file__).resolve().parents[1] / 'scripts/build-runtime.ps1'
        self.assertTrue(script.is_file(), 'Missing private-runtime PowerShell build entry')
        result = subprocess.run([shutil.which('powershell'), '-NoProfile', '-ExecutionPolicy', 'Bypass',
            '-File', str(script), '-Python', sys.executable, '-Git', self.git,
            '-SourceRoot', str(self.repo), '-SourceCommit', self.commit_id,
            '-InputRoot', str(self.inputs), '-Destination', str(self.destination)],
            capture_output=True, timeout=30)
        self.assertEqual(result.returncode, 0, result.stderr.decode(errors='replace'))
        summary = json.loads(result.stdout)
        self.assertEqual(summary['source_commit'], self.commit_id)
        self.assertTrue((self.destination / 'runtime/python.exe').is_file())

    def test_powershell_build_entry_rejects_bare_python_without_creating_payload(self):
        script = Path(__file__).resolve().parents[1] / 'scripts/build-runtime.ps1'
        self.assertTrue(script.is_file(), 'Missing private-runtime PowerShell build entry')
        result = subprocess.run([shutil.which('powershell'), '-NoProfile', '-ExecutionPolicy', 'Bypass',
            '-File', str(script), '-Python', 'python', '-Git', self.git,
            '-SourceRoot', str(self.repo), '-SourceCommit', self.commit_id,
            '-InputRoot', str(self.inputs), '-Destination', str(self.destination)],
            capture_output=True, timeout=30)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(self.destination.exists())


if __name__ == '__main__':
    unittest.main()
