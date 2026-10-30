#!/usr/bin/env python3
"""Reproduce the colour-run measurements recorded in session.json.

Standard library only: a bounded PNG reader (IHDR/IDAT, all five scanline
filters) plus run-length listings along fixed columns. It measures nothing
about the window identity; the capture it reads is the identity-proven one
published beside this file.
"""
import pathlib
import struct
import sys
import zlib

CHANNELS = {0: 1, 2: 3, 4: 2, 6: 4}


def decode(path: pathlib.Path) -> tuple[int, int, list[bytes]]:
    data = path.read_bytes()
    offset = 8
    idat = b""
    width = height = channels = 0
    while offset < len(data):
        length = int.from_bytes(data[offset:offset + 4], "big")
        kind = data[offset + 4:offset + 8]
        payload = data[offset + 8:offset + 8 + length]
        if kind == b"IHDR":
            width, height, depth, colour, _, _, interlace = struct.unpack(">IIBBBBB", payload)
            if depth != 8 or interlace != 0:
                raise SystemExit("only 8-bit non-interlaced PNGs are handled")
            channels = CHANNELS[colour]
        elif kind == b"IDAT":
            idat += payload
        elif kind == b"IEND":
            break
        offset += 12 + length
    raw = zlib.decompress(idat)
    stride = width * channels
    previous = bytearray(stride)
    rows: list[bytes] = []
    cursor = 0
    for _ in range(height):
        filter_kind = raw[cursor]
        cursor += 1
        line = bytearray(raw[cursor:cursor + stride])
        cursor += stride
        if filter_kind == 1:
            for i in range(channels, stride):
                line[i] = (line[i] + line[i - channels]) & 0xFF
        elif filter_kind == 2:
            for i in range(stride):
                line[i] = (line[i] + previous[i]) & 0xFF
        elif filter_kind == 3:
            for i in range(stride):
                left = line[i - channels] if i >= channels else 0
                line[i] = (line[i] + ((left + previous[i]) >> 1)) & 0xFF
        elif filter_kind == 4:
            for i in range(stride):
                left = line[i - channels] if i >= channels else 0
                up = previous[i]
                up_left = previous[i - channels] if i >= channels else 0
                estimate = left + up - up_left
                da, db, dc = abs(estimate - left), abs(estimate - up), abs(estimate - up_left)
                predictor = left if (da <= db and da <= dc) else (up if db <= dc else up_left)
                line[i] = (line[i] + predictor) & 0xFF
        rows.append(bytes(line))
        previous = line
    return width, height, rows


def runs(pixels: list[tuple[int, int, int]]) -> list[tuple[tuple[int, int, int], int]]:
    out: list[tuple[tuple[int, int, int], int]] = []
    last = None
    count = 0
    for value in pixels:
        if value == last:
            count += 1
        else:
            if last is not None:
                out.append((last, count))
            last = value
            count = 1
    out.append((last, count))
    return out


def main() -> int:
    path = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else "screenshot.png")
    width, height, rows = decode(path)
    channels = len(rows[0]) // width
    pixel = lambda x, y: tuple(rows[y][x * channels:x * channels + 3])
    print(f"image {width}x{height}, {channels} channels")
    for column in (200, 300, 700, 1200):
        print(f"x={column} vertical runs (bottom 200 rows):")
        for colour, length in runs([pixel(column, y) for y in range(height - 200, height - 20)])[:10]:
            print(f"   {colour} x{length} px")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
