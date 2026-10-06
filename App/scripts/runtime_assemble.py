"""Offline, commit-bound private-runtime assembly. Never launch/open hardware here.

The extra keyword-only paths are build inputs, not installed-GUI runtime selectors.
Uncommitted source is deliberately excluded: use the named Git tree's blob bytes.
"""
from __future__ import annotations

import argparse
import base64
import csv
import hashlib
import io
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import tempfile
import zipfile
from contextlib import ExitStack

from packaging.utils import canonicalize_name

from .runtime_lock import (RuntimeInputError, TARGET, checked_inventory, inspect_wheel,
                           safe_relative_name, validate_lock)

MAX_FILES = 20000
MAX_EXPANDED = 2 * 1024 ** 3
MAX_MANIFEST = 4 * 1024 ** 2
MAX_SOURCE = 64 * 1024 ** 2
MAX_INPUTS = 2 * 1024 ** 3
SEARCH_PATH = b'python313.zip\n.\nLib/site-packages\n..\n'
_SOURCE_ROOT = Path(__file__).resolve().parents[2]


def _require(condition, message):
    if not condition:
        raise RuntimeInputError(message)


def _json_bytes(value):
    return (json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(',', ':')) + '\n').encode('utf-8')


def _guard_path(path):
    """Reject existing link/reparse ancestors before writing an owned build path."""
    path = Path(os.path.abspath(path))
    for part in (path, *path.parents):
        if os.path.lexists(part):
            info = part.lstat()
            _require(not stat.S_ISLNK(info.st_mode) and not (
                getattr(info, 'st_file_attributes', 0) & 0x400), 'Build path contains a link/reparse point')
    return path


def _read_small(path, maximum):
    path = _guard_path(path)
    info = path.lstat()
    _require(stat.S_ISREG(info.st_mode) and info.st_nlink == 1 and info.st_size <= maximum,
             'Invalid bounded build input')
    with path.open('rb') as stream:
        opened = os.fstat(stream.fileno())
        _require((opened.st_dev, opened.st_ino) == (info.st_dev, info.st_ino), 'Build input changed while opening')
        data = stream.read(maximum + 1)
    _require(len(data) <= maximum, 'Build input grew past its bound')
    return data


def _git_bytes(root, git, *args, input_bytes=None):
    environment = {**os.environ, 'GIT_NO_REPLACE_OBJECTS': '1', 'GIT_TERMINAL_PROMPT': '0'}
    result = subprocess.run([str(git), '-C', str(root), *args], input=input_bytes,
                            capture_output=True, timeout=60, env=environment)
    _require(result.returncode == 0, 'Named source commit cannot be read by Git')
    return result.stdout


def _source_selected(name):
    if name in {'Code/__init__.py', 'App/__init__.py'}:
        return True
    parent, _, filename = name.rpartition('/')
    return ((parent in {'Code/Utils', 'Code/Setups', 'App/worker'} and filename.endswith('.py')) or
            (parent in {'Config', 'App/catalog'} and filename.endswith('.json')) or
            (parent == 'App/runtime' and filename in {'interpreter.json', 'license-files.json',
             'wheel-sources.json', 'requirements.in', 'requirements-win-x64.lock',
             'python313._pth', 'THIRD_PARTY.md'}))


def _committed_sources(root, git, commit):
    _require(type(commit) is str and re.fullmatch('[0-9a-f]{40}', commit), 'Exact source commit is required')
    _require(git is not None and Path(git).is_file(), 'An explicit available Git executable is required')
    identity = _git_bytes(root, git, 'rev-parse', '--verify', commit + '^{commit}').strip().decode('ascii')
    _require(identity == commit, 'Source commit identity mismatch')
    tree = _git_bytes(root, git, 'ls-tree', '-r', '-l', '-z', commit, '--', 'Code', 'App', 'Config')
    _require(len(tree) <= MAX_MANIFEST, 'Source tree inventory exceeds bound')
    selected, total = [], 0
    for raw in tree.split(b'\0'):
        if not raw:
            continue
        fields, name_bytes = raw.split(b'\t', 1)
        name = name_bytes.decode('utf-8')
        if not _source_selected(name):
            continue
        mode, kind, oid, size_text = fields.split()
        _require(mode in {b'100644', b'100755'} and kind == b'blob' and size_text.isdigit(),
                 'Selected source contains a link or non-file')
        safe_relative_name(name)
        size = int(size_text)
        total += size
        _require(total <= MAX_SOURCE and len(selected) < MAX_FILES, 'Source payload exceeds bound')
        selected.append((name, oid, size))
    _require({'Code/__init__.py', 'Code/Utils/__init__.py', 'App/__init__.py', 'App/worker/main.py'}
             <= {item[0] for item in selected}, 'Named commit lacks required application resources')
    blob_bytes = _git_bytes(root, git, 'cat-file', '--batch',
                           input_bytes=b''.join(oid + b'\n' for _, oid, _ in selected))
    _require(len(blob_bytes) <= MAX_SOURCE + MAX_MANIFEST, 'Source blob response exceeds bound')
    cursor, files = 0, {}
    for name, oid, size in selected:
        end = blob_bytes.index(b'\n', cursor)
        _require(blob_bytes[cursor:end] == oid + b' blob ' + str(size).encode(), 'Source blob identity mismatch')
        start, cursor = end + 1, end + 1 + size
        _require(blob_bytes[cursor:cursor + 1] == b'\n', 'Source blob framing mismatch')
        files[name] = blob_bytes[start:cursor]
        cursor += 1
    _require(cursor == len(blob_bytes), 'Unexpected source blob response')
    return files


def _snapshot_input(path, destination, remaining):
    """Copy before inspection: extracted bytes come from the hash-checked snapshot."""
    path = _guard_path(path)
    info = path.lstat()
    _require(stat.S_ISREG(info.st_mode) and info.st_nlink == 1 and info.st_size <= remaining,
             'Invalid runtime archive input')
    digest, size = hashlib.sha256(), 0
    with path.open('rb') as source, destination.open('xb') as output:
        opened = os.fstat(source.fileno())
        _require((opened.st_dev, opened.st_ino) == (info.st_dev, info.st_ino), 'Archive changed while opening')
        for block in iter(lambda: source.read(1024 * 1024), b''):
            size += len(block)
            _require(size <= remaining, 'Runtime input byte limit exceeded')
            digest.update(block)
            output.write(block)
    return digest.hexdigest(), size


def _write_payload_file(stage, relative, source):
    target = _guard_path(stage / safe_relative_name(relative))
    _require(target.is_relative_to(stage), 'Payload path escaped owned stage')
    target.parent.mkdir(parents=True, exist_ok=True)
    digest, size = hashlib.sha256(), 0
    with target.open('xb') as output:
        for block in iter(lambda: source.read(1024 * 1024), b''):
            size += len(block)
            _require(size <= MAX_EXPANDED, 'Payload file exceeds expanded-byte bound')
            digest.update(block)
            output.write(block)
    return size, digest.hexdigest()


def _wheel_path(name):
    parts = name.split('/')
    if parts[0].endswith('.data'):
        _require(len(parts) >= 3 and parts[1] in {'purelib', 'platlib'}, 'Unreviewed wheel data layout')
        return '/'.join(parts[2:])
    return name


def _omit_upstream_asset(name, licenses):
    if name in licenses:
        return False
    return any(part in {'tests', 'test', 'examples', 'benchmarks', '__pycache__'}
               for part in name.split('/')) or name.endswith(('.pyc', '.pyo'))


def _verify_stage(stage, files, manifest_bytes):
    """The final filesystem must match, not merely the earlier write results."""
    expected = {entry['path']: entry for entry in files}
    expected['runtime-manifest.json'] = {'size': len(manifest_bytes),
        'sha256': hashlib.sha256(manifest_bytes).hexdigest()}
    allowed_dirs = {'/'.join(name.split('/')[:i]) for name in expected
                    for i in range(1, len(name.split('/')))}
    observed = set()
    for root, directories, names in os.walk(stage, followlinks=False):
        for name in directories:
            path = _guard_path(Path(root) / name)
            _require(path.relative_to(stage).as_posix() in allowed_dirs,
                     'Unlisted payload directory appeared during assembly')
        for name in names:
            path = _guard_path(Path(root) / name)
            relative = path.relative_to(stage).as_posix()
            item = expected.get(relative)
            info = path.lstat()
            _require(item is not None and stat.S_ISREG(info.st_mode) and info.st_nlink == 1
                     and info.st_size == item['size'], 'Payload inventory changed during assembly')
            digest = hashlib.sha256()
            with path.open('rb') as stream:
                for block in iter(lambda: stream.read(1024 * 1024), b''):
                    digest.update(block)
            _require(digest.hexdigest() == item['sha256'], 'Payload hash changed during assembly')
            observed.add(relative)
    _require(observed == set(expected), 'Payload file disappeared during assembly')


def assemble_runtime(archive: Path, wheels: Path, lock: Path, destination: Path,
                     source_commit: str, *, source_root=None, git_executable=None) -> dict:
    """Create a new complete package root, or leave inputs/existing outputs alone.

    Only this call's unique sibling stage is removed on failure. A manifest is an
    inventory, not proof of clean-machine, physical-device or native-launch acceptance.
    """
    stage = None
    try:
        destination = _guard_path(destination)
        parent = destination.parent
        _require(parent.is_dir() and not os.path.lexists(destination), 'Destination must be NEW with an existing parent')
        archive, wheels, lock = _guard_path(archive), _guard_path(wheels), _guard_path(lock)
        _require(wheels.is_dir(), 'Wheel input directory is missing')
        source = _committed_sources(Path(source_root or _SOURCE_ROOT),
                                    git_executable or shutil.which('git'), source_commit)
        inputs = {name.rsplit('/', 1)[-1]: data for name, data in source.items() if name.startswith('App/runtime/')}
        _require({'interpreter.json', 'license-files.json', 'python313._pth', 'requirements-win-x64.lock'}
                 <= set(inputs), 'Committed runtime build catalogs are missing')
        for name in ('interpreter.json', 'license-files.json', 'python313._pth', 'requirements-win-x64.lock'):
            # Git text checkout may change LF to CRLF. Package bytes still come
            # only from the named blob; no other uncommitted change is accepted.
            _require(_read_small(lock.parent / name, MAX_MANIFEST).replace(b'\r\n', b'\n') ==
                     inputs[name].replace(b'\r\n', b'\n'),
                     'Runtime catalog differs from the named source commit')
        _require(lock.name == 'requirements-win-x64.lock' and inputs['python313._pth'] == SEARCH_PATH,
                 'Unreviewed runtime lock or executable search path')
        interpreter = json.loads(inputs['interpreter.json'])
        _require(type(interpreter) is dict and interpreter.get('v') == 1 and all(
            interpreter.get(key) == value for key, value in
            {'version': TARGET['python'], 'implementation': 'cp', 'abi': 'cp313', 'platform': 'win_amd64'}.items()),
            'Unqualified interpreter target')
        _require(interpreter.get('archive') == archive.name and
                 re.fullmatch('[0-9a-f]{64}', interpreter.get('sha256', '')), 'Invalid interpreter archive identity')
        wheel_paths = sorted(wheels.iterdir(), key=lambda path: path.name.casefold())
        _require(0 < len(wheel_paths) <= 128 and all(path.name.endswith('.whl') for path in wheel_paths),
                 'Only the exact locked wheel input set is allowed')
        stage = Path(tempfile.mkdtemp(prefix='.runtime-stage-', dir=parent))
        raw = stage / '.inputs'
        raw.mkdir()
        digest, consumed = _snapshot_input(archive, raw / archive.name, MAX_INPUTS)
        _require(digest == interpreter['sha256'], 'Interpreter archive SHA256 mismatch')
        metadata = []
        for path in wheel_paths:
            _digest, size = _snapshot_input(path, raw / path.name, MAX_INPUTS - consumed)
            consumed += size
            metadata.append(inspect_wheel(raw / path.name))
        qualified = validate_lock(inputs['requirements-win-x64.lock'], metadata)
        notices = json.loads(inputs['license-files.json'])
        _require(type(notices) is dict and set(notices) == {'v', 'files'} and notices['v'] == 1
                 and type(notices['files']) is list and len(notices['files']) <= 128, 'Malformed external notice catalog')
        package_map = {canonicalize_name(item['name']): item for item in qualified['packages']}
        covered, external = set(), []
        for notice in notices['files']:
            _require(type(notice) is dict and set(notice) ==
                     {'package', 'version', 'wheel_sha256', 'filename', 'url', 'sha256'} and
                     all(type(value) is str for value in notice.values()), 'Malformed external notice binding')
            name = canonicalize_name(notice['package'])
            package = package_map.get(name)
            filename = safe_relative_name(notice['filename'])
            _require(package is not None and name not in covered and filename.startswith('licenses/') and
                     notice['version'] == package['version'] and notice['wheel_sha256'] == package['sha256'] and
                     re.fullmatch('[0-9a-f]{64}', notice['sha256']) and notice['url'].startswith('https://'),
                     'External notice is not bound to the locked wheel')
            content = _read_small(wheels.parent / filename, MAX_MANIFEST)
            _require(hashlib.sha256(content).hexdigest() == notice['sha256'], 'External notice SHA256 mismatch')
            external.append((filename, content))
            covered.add(name)
        _require(set(qualified['missing_license_packages']) <= covered, 'Required external license notice is missing')

        files, excluded, destinations, directories, expanded = [], [], set(), set(), 0
        def write(relative, size, origin, stream):
            nonlocal expanded
            relative = safe_relative_name(relative)
            folded = relative.casefold()
            parents = {'/'.join(folded.split('/')[:i]) for i in range(1, len(folded.split('/')))}
            _require(folded not in destinations and folded not in directories and not parents & destinations,
                     'Payload has a file/directory or case-insensitive collision')
            _require(len(files) < MAX_FILES and expanded + size <= MAX_EXPANDED, 'Aggregate payload limit exceeded')
            destinations.add(folded)
            directories.update(parents)
            expanded += size
            observed, sha = _write_payload_file(stage, relative, stream)
            _require(observed == size, 'Payload changed during extraction')
            entry = {'path': relative, 'size': size, 'sha256': sha, 'origin': origin}
            files.append(entry)
            return entry
        with ExitStack() as opened:
            embedded = opened.enter_context(zipfile.ZipFile(raw / archive.name))
            embedded_files = checked_inventory(embedded)
            _require({'python.exe', 'python313.dll', 'python313.zip', 'LICENSE.txt', 'python313._pth'}
                     <= set(embedded_files), 'Embedded interpreter is incomplete')
            for name in sorted(embedded_files):
                _require(not name.endswith(('.pth', '._pth')) or name == 'python313._pth',
                         'Unreviewed interpreter path configuration')
                content = SEARCH_PATH if name == 'python313._pth' else None
                with io.BytesIO(content) if content is not None else embedded.open(embedded_files[name]) as stream:
                    write('runtime/' + name, len(content) if content is not None else embedded_files[name].file_size,
                          'interpreter', stream)
            for package in qualified['packages']:
                wheel = opened.enter_context(zipfile.ZipFile(raw / package['filename']))
                inventory = checked_inventory(wheel)
                record = next(name for name in inventory if name.endswith('.dist-info/RECORD'))
                installed = []
                for name in sorted(inventory):
                    if name == record:
                        continue
                    _require(not name.endswith(('.pth', '._pth')), 'Wheel path-execution configuration is forbidden')
                    relative = _wheel_path(name)
                    if _omit_upstream_asset(name, package['licenses']):
                        excluded.append({'wheel': package['filename'], 'path': name, 'reason': 'upstream-test-or-example'})
                        continue
                    with wheel.open(inventory[name]) as stream:
                        entry = write('runtime/Lib/site-packages/' + relative, inventory[name].file_size,
                                      'wheel:' + package['filename'], stream)
                    installed.append((relative, entry))
                text = io.StringIO(newline='')
                rows = csv.writer(text, lineterminator='\n')
                for relative, entry in installed:
                    digest = base64.urlsafe_b64encode(bytes.fromhex(entry['sha256'])).decode().rstrip('=')
                    rows.writerow([relative, 'sha256=' + digest, entry['size']])
                rows.writerow([_wheel_path(record), '', ''])
                content = text.getvalue().encode('utf-8')
                write('runtime/Lib/site-packages/' + _wheel_path(record), len(content),
                      'wheel-record:' + package['filename'], io.BytesIO(content))
        for name, content in sorted(source.items()):
            write(name, len(content), 'source:' + source_commit, io.BytesIO(content))
        for name, content in external:
            write(name, len(content), 'external-notice', io.BytesIO(content))
        manifest = {'v': 1, 'target': qualified['target'], 'interpreter': interpreter,
                    'source_kind': 'git-commit', 'source_commit': source_commit,
                    'packages': qualified['packages'], 'external_notices': notices['files'],
                    'excluded': excluded, 'files': sorted(files, key=lambda entry: entry['path'])}
        manifest['build_id'] = hashlib.sha256(_json_bytes(manifest)).hexdigest()
        encoded = _json_bytes(manifest)
        _require(len(encoded) <= MAX_MANIFEST and expanded + len(encoded) <= MAX_EXPANDED
                 and len(files) + 1 <= MAX_FILES, 'Final manifest/payload exceeds bound')
        _write_payload_file(stage, 'runtime-manifest.json', io.BytesIO(encoded))
        _require(_guard_path(raw).parent == stage and raw.name == '.inputs', 'Unsafe input-stage cleanup target')
        shutil.rmtree(raw)
        _verify_stage(stage, files, encoded)
        _require(_guard_path(parent) == parent and _guard_path(stage).parent == parent
                 and not os.path.lexists(destination), 'Destination changed before atomic admission')
        # Windows rename never overwrites an existing file or directory.
        _require(os.name == 'nt', 'Runtime assembly admission currently requires Windows')
        stage.rename(destination)
        stage = None
        return manifest
    except RuntimeInputError:
        raise
    except (OSError, ValueError, TypeError, KeyError, UnicodeError, zipfile.BadZipFile,
            subprocess.SubprocessError) as error:
        raise RuntimeInputError('Runtime assembly failed; destination was not admitted') from error
    finally:
        if stage is not None and os.path.lexists(stage):
            _require(_guard_path(stage).parent == parent and stage.name.startswith('.runtime-stage-'),
                     'Unsafe failed-stage cleanup target')
            shutil.rmtree(stage)


def main(argv=None):
    parser = argparse.ArgumentParser(description='Build a commit-bound private runtime; no hardware or installation')
    for name in ('archive', 'wheels', 'lock', 'destination', 'source-root', 'git'):
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--source-commit', required=True)
    args = parser.parse_args(argv)
    result = assemble_runtime(args.archive, args.wheels, args.lock, args.destination, args.source_commit,
                              source_root=args.source_root, git_executable=args.git)
    print(json.dumps({'destination': str(args.destination), 'source_kind': result['source_kind'],
                      'source_commit': result['source_commit'], 'build_id': result['build_id'],
                      'files': len(result['files']), 'excluded_assets': len(result['excluded'])}, ensure_ascii=True))
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
