"""Build-only validation of a closed Windows CPython wheel input set.

No downloads, imports of inspected packages, pip installation, hardware or process launch.
Wheel bytes are inspected before an assembler may extract them.
"""

from __future__ import annotations

import base64
import copy
import csv
import hashlib
import io
import re
import stat
import zipfile
from email.parser import BytesParser
from pathlib import Path

from packaging.markers import default_environment
from packaging.requirements import Requirement
from packaging.specifiers import SpecifierSet
from packaging.tags import compatible_tags, cpython_tags, parse_tag
from packaging.utils import canonicalize_name, parse_wheel_filename
from packaging.version import Version


TARGET = {'python': '3.13.16', 'implementation': 'cp', 'abi': 'cp313', 'platform': 'win_amd64'}
MAX_FILES = 20000
MAX_EXPANDED = 2 * 1024 ** 3
MAX_METADATA = 65536
_FIELDS = {'filename', 'name', 'version', 'sha256', 'requires_python', 'requires_dist',
           'tags', 'licenses'}
_COMPATIBLE = set(cpython_tags((3, 13), abis=['cp313'], platforms=['win_amd64'])) | set(
    compatible_tags((3, 13), interpreter='cp313', platforms=['win_amd64']))


class RuntimeInputError(ValueError):
    """A build input is unqualified; never fall back to another input/runtime."""


def _require(condition, message):
    if not condition:
        raise RuntimeInputError(message)


def safe_relative_name(name):
    """Reject Windows aliases/escapes before any extraction, on any build OS."""
    _require(type(name) is str and 0 < len(name) <= 512 and name.isprintable()
             and '\\' not in name and ':' not in name and not name.startswith('/'),
             'Unsafe archive path')
    parts = name.rstrip('/').split('/')
    reserved = {'con', 'prn', 'aux', 'nul', *(f'com{i}' for i in range(1, 10)),
                *(f'lpt{i}' for i in range(1, 10))}
    _require(all(part not in {'', '.', '..'} and part == part.rstrip(' .')
                 and not any(c in part for c in '<>"|?*')
                 and part.split('.')[0].casefold() not in reserved for part in parts),
             'Unsafe Windows archive path')
    return '/'.join(parts)


def checked_inventory(archive):
    entries = archive.infolist()
    _require(len(entries) <= MAX_FILES, 'Archive file-count limit exceeded')
    seen, total, files = set(), 0, {}
    for entry in entries:
        name = safe_relative_name(entry.filename)
        _require(name.casefold() not in seen, 'Archive has duplicate case-insensitive paths')
        seen.add(name.casefold())
        mode = stat.S_IFMT(entry.external_attr >> 16)
        _require(mode in {0, stat.S_IFREG, stat.S_IFDIR} and not (entry.external_attr & 0x400)
                 and not (entry.flag_bits & 1), 'Archive contains link/special/encrypted entry')
        _require(entry.compress_type in {zipfile.ZIP_STORED, zipfile.ZIP_DEFLATED},
                 'Unsupported archive compression')
        total += entry.file_size
        _require(total <= MAX_EXPANDED, 'Archive expanded-byte limit exceeded')
        if not entry.is_dir():
            files[name] = entry
    return files


def sha256_file(path):
    digest = hashlib.sha256()
    with Path(path).open('rb') as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b''):
            digest.update(block)
    return digest.hexdigest()


def _small_bytes(archive, entry, limit=MAX_METADATA):
    _require(entry.file_size <= limit, 'Archive metadata byte limit exceeded')
    return archive.read(entry)


def _verify_records(archive, files, record_name):
    records = {}
    text = _small_bytes(archive, files[record_name], 4 * 1024 ** 2).decode('utf-8')
    for row in csv.reader(io.StringIO(text, newline='')):
        _require(len(row) == 3, 'Malformed wheel RECORD')
        name = safe_relative_name(row[0])
        _require(name not in records, 'Duplicate wheel RECORD entry')
        records[name] = row[1:]
    _require(set(records) == set(files), 'Wheel RECORD inventory mismatch')
    for name, entry in files.items():
        digest, size = records[name]
        if name == record_name:
            _require(digest == size == '', 'Wheel RECORD self-hash must be empty')
            continue
        _require(digest.startswith('sha256=') and size.isdecimal()
                 and int(size) == entry.file_size, 'Wheel RECORD size/hash missing')
        hashed = hashlib.sha256()
        with archive.open(entry) as stream:
            for block in iter(lambda: stream.read(1024 * 1024), b''):
                hashed.update(block)
        expected = base64.urlsafe_b64encode(hashed.digest()).decode('ascii').rstrip('=')
        _require(digest == 'sha256=' + expected, 'Wheel RECORD content hash mismatch')


def inspect_wheel(path: Path) -> dict:
    """Inspect actual ZIP bytes, default dependency metadata, tags and licenses."""
    path = Path(path)
    try:
        parse_wheel_filename(path.name)
        with zipfile.ZipFile(path) as archive:
            files = checked_inventory(archive)
            metadata_names = [name for name in files if name.endswith('.dist-info/METADATA')]
            _require(len(metadata_names) == 1, 'Wheel needs one METADATA identity')
            prefix = metadata_names[0].rsplit('/', 1)[0]
            record, wheel = prefix + '/RECORD', prefix + '/WHEEL'
            _require(record in files and wheel in files, 'Wheel lacks RECORD or WHEEL')
            _verify_records(archive, files, record)
            metadata = BytesParser().parsebytes(_small_bytes(archive, files[metadata_names[0]]))
            wheel_metadata = BytesParser().parsebytes(_small_bytes(archive, files[wheel]))
            _require(len(metadata.get_all('Name', [])) == len(metadata.get_all('Version', [])) == 1
                     and len(metadata.get_all('Requires-Python', [])) <= 1,
                     'Wheel metadata identity is ambiguous')
            licenses = sorted(name for name in files if any(word in name.rsplit('/', 1)[-1].lower()
                              for word in ('license', 'copying', 'notice')))
            return {'filename': path.name, 'name': metadata['Name'], 'version': metadata['Version'],
                'sha256': sha256_file(path), 'requires_python': metadata.get('Requires-Python', ''),
                'requires_dist': metadata.get_all('Requires-Dist', []),
                'tags': wheel_metadata.get_all('Tag', []), 'licenses': licenses}
    except RuntimeInputError:
        raise
    except (OSError, ValueError, UnicodeError, zipfile.BadZipFile, RuntimeError, KeyError) as error:
        raise RuntimeInputError('Invalid wheel archive') from error


def validate_lock(lock_bytes: bytes, wheel_metadata: list[dict]) -> dict:
    """Validate the DEFAULT target closure; report missing license files explicitly.

    A closed wheel graph is not redistribution/package approval. The assembler
    must supply and verify each missing external notice before admitting files.
    """
    _require(type(lock_bytes) is bytes and len(lock_bytes) <= 65536, 'Invalid runtime lock size/type')
    _require(type(wheel_metadata) is list and 0 < len(wheel_metadata) <= 128,
             'Invalid runtime wheel metadata count/type')
    try:
        pins = {}
        for raw in lock_bytes.decode('utf-8').splitlines():
            line = raw.strip()
            if not line or line.startswith('#'):
                continue
            entry = re.fullmatch(r'([A-Za-z0-9][A-Za-z0-9_.-]*)==([^\s]+) --hash=sha256:([0-9a-f]{64})', line)
            _require(entry is not None, 'Runtime lock requires exact versions and SHA256')
            name = canonicalize_name(entry[1])
            _require(name not in pins and name not in {'pyvisa-sim', 'pyvisa-py'},
                     'Duplicate or unapproved runtime dependency')
            pins[name] = (Version(entry[2]), entry[3])
        _require(0 < len(pins) <= 128, 'Invalid runtime lock package count')
        checked = {}
        for item in wheel_metadata:
            _require(type(item) is dict and set(item) == _FIELDS, 'Malformed wheel metadata shape')
            _require(all(type(item[key]) is str and len(item[key]) <= 512 for key in
                         ('filename', 'name', 'version', 'sha256', 'requires_python')),
                     'Malformed wheel metadata text')
            safe_relative_name(item['filename'])
            _require('/' not in item['filename'], 'Wheel filename must be a basename')
            name, version, _build, filename_tags = parse_wheel_filename(item['filename'])
            name = canonicalize_name(name)
            _require(name == canonicalize_name(item['name']) and version == Version(item['version']),
                     'Wheel filename/metadata identity mismatch')
            _require(name not in checked and name in pins and pins[name] == (version, item['sha256']),
                     'Wheel not uniquely admitted by lock/hash')
            for key in ('requires_dist', 'tags', 'licenses'):
                _require(type(item[key]) is list and len(item[key]) <= 128
                         and all(type(value) is str and len(value) <= 2048 for value in item[key]),
                         'Malformed wheel metadata list')
            declared_tags = {tag for text in item['tags'] for tag in parse_tag(text)}
            _require(declared_tags == filename_tags and bool(filename_tags & _COMPATIBLE),
                     'Wheel has incompatible target tags')
            _require(SpecifierSet(item['requires_python']).contains(TARGET['python']),
                     'Wheel requires a different Python version')
            for license_path in item['licenses']:
                safe_relative_name(license_path)
            checked[name] = copy.deepcopy(item)
        _require(set(checked) == set(pins), 'Locked wheel inventory is incomplete')
        environment = default_environment()
        environment.update(python_version='3.13', python_full_version=TARGET['python'],
            implementation_version=TARGET['python'], implementation_name='cpython',
            platform_python_implementation='CPython', os_name='nt', sys_platform='win32',
            platform_system='Windows', platform_machine='AMD64', platform_release='',
            platform_version='', extra='')
        for item in checked.values():
            for text in item['requires_dist']:
                requirement = Requirement(text)
                if requirement.marker is not None and not requirement.marker.evaluate(environment):
                    continue
                _require(requirement.url is None and not requirement.extras,
                         'Unreviewed dependency URL/extra')
                dependency = canonicalize_name(requirement.name)
                _require(dependency in pins and requirement.specifier.contains(pins[dependency][0]),
                         'Target dependency closure is incomplete or version incompatible')
        return {'target': copy.deepcopy(TARGET),
                'packages': [checked[name] for name in sorted(checked)],
                'missing_license_packages': [name for name in sorted(checked)
                    if not checked[name]['licenses']]}
    except RuntimeInputError:
        raise
    except (ValueError, UnicodeError, TypeError, KeyError) as error:
        raise RuntimeInputError('Malformed runtime lock or metadata') from error
