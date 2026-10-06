import hashlib
import io
import json
import struct
from types import SimpleNamespace
import unittest

from Code.Debugs.check_osa_remote import event, main


def chunks(raw, corrupt=False):
    frames = []
    for offset in (0, len(raw)//2):
        part = raw[offset:len(raw)//2] if offset == 0 else raw[offset:]
        value = dict(event_chunk=True,seq=7,offset=offset,size=len(raw),checksum='0'*64 if corrupt else hashlib.sha256(raw).hexdigest(),data_hex=part.hex())
        data = json.dumps(value).encode()
        frames.append(struct.pack('<I',len(data))+data)
    return SimpleNamespace(transport=io.BytesIO(b''.join(frames)))


class RemoteDiagnosticTests(unittest.TestCase):
    def test_event_is_assembled_and_hash_checked_before_publication(self):
        raw = b'{"type":"snapshot","host_id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","seq":7}'
        self.assertEqual(event(chunks(raw)),dict(type='snapshot',host_id='a'*32,seq=7))
        with self.assertRaises(ValueError): event(chunks(raw,True))

    def test_oversized_event_is_rejected_before_following_chunk(self):
        data=json.dumps(dict(event_chunk=True,seq=1,offset=0,size=1024*1024+1,checksum='0'*64,data_hex='00')).encode()
        client=SimpleNamespace(transport=io.BytesIO(struct.pack('<I',len(data))+data))
        with self.assertRaises(ValueError):event(client)

    def test_osa_stage_without_explicit_confirmation_fails_before_host_lookup(self):
        with self.assertRaisesRegex(ValueError,'authorization'):
            main(['--host','not-a-host.exe','--out','not-created','--read-osa'])


if __name__=='__main__':unittest.main()
