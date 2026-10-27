"""Deterministic, bounded BI_BITFIELDS BMP fixtures for MP-IMG2-W3."""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import struct

MAX_CASES = 12
MAX_TOTAL_BYTES = 1024 * 1024
MASKS = (0xF800, 0x07E0, 0x001F)
CASE_IDS = ("primaries", "midtones", "narrow_bottom", "odd_bottom", "odd_top",
            "truncated", "bad_masks", "overlap_masks", "bad_header", "inconsistent",
            "unsupported", "not_bgr565")


def _pixels(cid: str):
    if cid in ("truncated", "bad_masks", "overlap_masks", "bad_header", "inconsistent", "unsupported", "not_bgr565"):
        return 3, 3, [(255,0,0),(0,255,0),(0,0,255),(255,255,0),(0,255,255),(255,0,255),(16,32,48),(80,112,144),(208,224,240)]
    if cid == "primaries":
        return 5, 1, [(0, 0, 0), (255, 255, 255), (255, 0, 0), (0, 255, 0), (0, 0, 255)]
    if cid == "midtones":
        vals = [(8, 16, 24), (40, 80, 120), (88, 144, 200),
                (128, 128, 128), (200, 160, 96), (248, 232, 216), (31, 63, 95),
                (17, 73, 181), (99, 155, 211), (239, 191, 143), (55, 111, 167),
                (201, 77, 133), (3, 129, 251), (227, 35, 101)]
        return 7, 2, vals
    if cid == "narrow_bottom":
        return 1, 3, [(255, 0, 0), (0, 255, 0), (0, 0, 255)]
    if cid in ("odd_bottom", "odd_top"):
        return 3, 3, [(255,0,0),(0,255,0),(0,0,255),(255,255,0),(0,255,255),(255,0,255),(16,32,48),(80,112,144),(208,224,240)]
    raise ValueError("unknown_fixture_id")


def _expand(v, bits):
    q = v >> (8 - bits)
    return (q << (8 - bits)) | (q >> (2 * bits - 8))


def _bmp(cid):
    w, h, px = _pixels(cid)
    if cid not in CASE_IDS:
        raise ValueError("unknown_fixture_id")
    if cid == "bad_header":
        data = bytearray(_build(w, h, px))
        struct.pack_into("<I", data, 14, 0xFFFFFFFF)
        return bytes(data), w, h, "bottom-up", MASKS, "invalid_header", None
    if cid == "unsupported":
        return _build(w, h, px, compression=0xFFFFFFFF), w, h, "bottom-up", MASKS, "unsupported_compression", None
    if cid == "inconsistent":
        return _build(w, h, px, planes=0), w, h, "bottom-up", MASKS, "inconsistent_planes", None
    if cid == "bad_masks":
        return _build(w,h,px,masks=(0,0,0)),w,h,"bottom-up",(0,0,0),"zero_masks",None
    if cid == "overlap_masks":
        return _build(w,h,px,masks=(0xF800,0xF800,0x001F)),w,h,"bottom-up",(0xF800,0xF800,0x001F),"overlapping_masks",None
    if cid == "not_bgr565":
        m=(0x7C00,0x03E0,0x001F)
        return _build(w,h,px,masks=m),w,h,"bottom-up",m,"valid_bgr555_control",None
    if cid == "truncated":
        data = _build(w, h, px)[:-8]
        return data, w, h, "bottom-up", MASKS, "missing_last_row", None
    top = cid == "odd_top"
    data = _build(w,h,px,top_down=top)
    rgba = bytes(c for r,g,b in px for c in (_expand(r,5),_expand(g,6),_expand(b,5),255))
    return data,w,h,"top-down" if top else "bottom-up",MASKS,"bgr565",rgba


def _build(w,h,pixels,top_down=False,masks=MASKS,compression=3,planes=1):
    stride=((w*2+3)//4)*4
    rows=[]
    for y in range(h):
        logical=y if top_down else h-1-y
        row=bytearray()
        for r,g,b in pixels[logical*w:(logical+1)*w]:
            word=((r>>3)<<11)|((g>>2)<<5)|(b>>3)
            row += struct.pack('<H',word)
        while len(row)<stride: row.append(0xA5)
        rows.append(bytes(row))
    payload=b''.join(rows)
    offset=14+40+12
    header=struct.pack('<2sIHHI',b'BM',offset+len(payload),0,0,offset)
    dib=struct.pack('<IiiHHIIiiII',40,w,-h if top_down else h,planes,16,compression,len(payload),2835,2835,0,0)
    return header+dib+struct.pack('<III',*masks)+payload


def fixture_bytes():
    return {cid:_bmp(cid)[0] for cid in CASE_IDS}


def generate(root: Path) -> list[dict]:
    root=Path(root)
    if root.exists(): raise ValueError("output_root_must_be_fresh")
    root.mkdir(parents=True)
    records=[]
    total=0
    for cid in CASE_IDS:
        data,w,h,order,masks,kind,rgba=_bmp(cid)
        total+=len(data)
        if len(records)>=MAX_CASES or total>MAX_TOTAL_BYTES: raise ValueError("fixture_bounds_exceeded")
        name=cid+".bmp"
        with (root/name).open('xb') as f: f.write(data)
        stride=((w*2+3)//4)*4 if w else 0
        rec={"id":cid,"path":name,"width":w,"height":h,"row_order":order,"masks":list(masks),
             "bits_per_pixel":16,"encoded_row_stride":stride,"logical_row_bytes":w*2,
             "logical_source_bytes":w*h*2,"bytes":len(data),"sha256":hashlib.sha256(data).hexdigest(),
             "expected_rgba_sha256":hashlib.sha256(rgba).hexdigest() if rgba is not None else None,
             "expected_rgba_hex":rgba.hex() if rgba is not None else None,"kind":kind,
             "method":"stdlib deterministic BMP BI_BITFIELDS; pack RGB565, preserve odd-row padding as 0xA5",
             "algorithm":"quantize R5/G6/B5 by high-bit truncation; expected channels independently expanded by bit replication; alpha=255"}
        records.append(rec)
    (root/"manifest.json").write_text(json.dumps({"algorithm":"RGB565 high-bit quantization; bit-replication expansion to RGBA8", "fixtures":records},indent=2)+"\n",encoding="utf-8")
    return records


def main():
    parser=argparse.ArgumentParser()
    parser.add_argument('--out',required=True,type=Path)
    generate(parser.parse_args().out)

if __name__=='__main__': main()
