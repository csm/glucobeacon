#!/usr/bin/env python3
"""Crop a PNG to a given height, in pure Python.

Headless Chromium clips the bottom of the page when the viewport height is
exactly the content height — about a hundred pixels vanish, silently. So the
render is done into a taller viewport and trimmed back here.

Only what that needs: 8-bit RGB/RGBA, no interlacing, which is what Chromium
emits. Pillow is not installed and is not worth a dependency for this.
"""

from __future__ import annotations

import struct
import sys
import zlib


def _unfilter(raw: bytes, width: int, height: int, channels: int) -> list[bytes]:
    stride = width * channels
    prev = bytearray(stride)
    rows: list[bytes] = []
    pos = 0
    for _ in range(height):
        filter_type = raw[pos]
        pos += 1
        line = bytearray(raw[pos : pos + stride])
        pos += stride
        if filter_type == 1:
            for i in range(channels, stride):
                line[i] = (line[i] + line[i - channels]) & 0xFF
        elif filter_type == 2:
            for i in range(stride):
                line[i] = (line[i] + prev[i]) & 0xFF
        elif filter_type == 3:
            for i in range(stride):
                left = line[i - channels] if i >= channels else 0
                line[i] = (line[i] + ((left + prev[i]) >> 1)) & 0xFF
        elif filter_type == 4:
            for i in range(stride):
                left = line[i - channels] if i >= channels else 0
                upper_left = prev[i - channels] if i >= channels else 0
                up = prev[i]
                pa = abs(up - upper_left)
                pb = abs(left - upper_left)
                pc = abs(left + up - 2 * upper_left)
                if pa <= pb and pa <= pc:
                    predictor = left
                elif pb <= pc:
                    predictor = up
                else:
                    predictor = upper_left
                line[i] = (line[i] + predictor) & 0xFF
        elif filter_type != 0:
            raise ValueError(f"unsupported PNG filter {filter_type}")
        rows.append(bytes(line))
        prev = line
    return rows


def _chunk(tag: bytes, payload: bytes) -> bytes:
    return (
        struct.pack(">I", len(payload))
        + tag
        + payload
        + struct.pack(">I", zlib.crc32(tag + payload) & 0xFFFFFFFF)
    )


def crop(path: str, keep_height: int) -> None:
    data = open(path, "rb").read()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError(f"{path} is not a PNG")

    pos, idat, ihdr = 8, b"", None
    while pos < len(data):
        (length,) = struct.unpack(">I", data[pos : pos + 4])
        tag = data[pos + 4 : pos + 8]
        payload = data[pos + 8 : pos + 8 + length]
        if tag == b"IHDR":
            ihdr = struct.unpack(">IIBBBBB", payload)
        elif tag == b"IDAT":
            idat += payload
        pos += 12 + length

    width, height, depth, colour, compression, filt, interlace = ihdr
    if depth != 8 or interlace != 0 or colour not in (2, 6):
        raise ValueError(f"unsupported PNG: depth={depth} colour={colour} interlace={interlace}")
    channels = 3 if colour == 2 else 4

    keep = min(keep_height, height)
    rows = _unfilter(zlib.decompress(idat), width, height, channels)[:keep]

    body = b"".join(b"\x00" + row for row in rows)
    out = (
        b"\x89PNG\r\n\x1a\n"
        + _chunk(b"IHDR", struct.pack(">IIBBBBB", width, keep, 8, colour, 0, 0, 0))
        + _chunk(b"IDAT", zlib.compress(body, 9))
        + _chunk(b"IEND", b"")
    )
    open(path, "wb").write(out)


if __name__ == "__main__":
    if len(sys.argv) != 3:
        print("usage: crop_png.py FILE HEIGHT", file=sys.stderr)
        raise SystemExit(2)
    crop(sys.argv[1], int(sys.argv[2]))
