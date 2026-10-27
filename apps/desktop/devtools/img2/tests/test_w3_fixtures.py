from __future__ import annotations

import hashlib
import json
from pathlib import Path
import struct
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from w3_fixtures import CASE_IDS, fixture_bytes, generate


class FixtureTests(unittest.TestCase):
    def test_inventory_and_deterministic_generation(self):
        raw=fixture_bytes()
        self.assertEqual(tuple(raw),CASE_IDS)
        with tempfile.TemporaryDirectory() as td:
            a=Path(td)/"a"; b=Path(td)/"b"
            ra=generate(a); rb=generate(b)
            self.assertEqual(ra,rb)
            self.assertEqual((a/"manifest.json").read_bytes(),(b/"manifest.json").read_bytes())
            self.assertLessEqual(sum(len(x) for x in raw.values()),1024*1024)
            for rec in ra:
                data=(a/rec['path']).read_bytes()
                self.assertEqual(data,raw[rec['id']])
                self.assertEqual(hashlib.sha256(data).hexdigest(),rec['sha256'])
                self.assertEqual(rec['bytes'],len(data))
                self.assertIn(rec['kind'],('bgr565','missing_last_row','zero_masks','overlapping_masks','invalid_header','inconsistent_planes','unsupported_compression','valid_bgr555_control'))
            manifest=json.loads((a/'manifest.json').read_text())
            self.assertEqual([x['id'] for x in manifest['fixtures']],list(CASE_IDS))

    def test_bmp_dimensions_padding_and_row_order(self):
        raw=fixture_bytes()
        self.assertEqual(raw['primaries'][:2],b'BM')
        prim=raw['primaries']
        self.assertEqual(struct.unpack_from('<ii',prim,18),(5,1))
        self.assertEqual(struct.unpack_from('<I',prim,34)[0],12)
        for cid in ('odd_bottom','odd_top'):
            data=raw[cid]
            self.assertEqual(struct.unpack_from('<ii',data,18),(3,3 if cid=='odd_bottom' else -3))
            self.assertEqual(struct.unpack_from('<I',data,34)[0],24)
            self.assertEqual(data[-2:],b'\xa5\xa5')
        bottom_rows = [raw['odd_bottom'][66 + y * 8:74 + y * 8] for y in range(3)]
        top_rows = [raw['odd_top'][66 + y * 8:74 + y * 8] for y in range(3)]
        self.assertEqual(bottom_rows, list(reversed(top_rows)))

    def test_closed_invalid_matrix_and_fresh_root(self):
        raw=fixture_bytes()
        self.assertEqual(len(raw),len(CASE_IDS))
        self.assertEqual(struct.unpack_from('<ii',raw['bad_masks'],18),(3,3))
        self.assertEqual(struct.unpack_from('<I',raw['bad_masks'],34)[0],24)
        self.assertEqual(struct.unpack_from('<ii',raw['not_bgr565'],18),(3,3))
        self.assertEqual(struct.unpack_from('<I',raw['not_bgr565'],34)[0],24)
        self.assertEqual(struct.unpack_from('<I', raw['bad_header'], 14)[0], 0xFFFFFFFF)
        self.assertEqual(struct.unpack_from('<III',raw['bad_masks'],54),(0,0,0))
        self.assertEqual(struct.unpack_from('<III',raw['overlap_masks'],54),(0xF800,0xF800,0x001F))
        self.assertEqual(len(raw['truncated']), 14 + 40 + 12 + 24 - 8)
        with tempfile.TemporaryDirectory() as td:
            occupied=Path(td)/'occupied'; occupied.mkdir()
            with self.assertRaises(ValueError): generate(occupied)
            self.assertTrue(generate(Path(td) / 'occupied' / 'child'))

if __name__=='__main__': unittest.main()
