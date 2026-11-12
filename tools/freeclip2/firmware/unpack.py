#!/usr/bin/env python3
"""Unpack a FreeClip 2 firmware package (updatepkg.bin) into its sections.

Usage: python3 -I tools/freeclip2/firmware/unpack.py (update_accessory_fw.zip | updatepkg.bin) OUT_DIR

Writes OUT_DIR/secNN.bin (the section as stored) and, for compressed sections,
OUT_DIR/secNN.img (the 0x200-byte section header followed by the decompressed
payload). Every SHA-256 in the package header is checked; a mismatch is an error.
Layout: docs/devices/freeclip2/FINDINGS.md "Firmware image".
"""

import hashlib
import lzma
import os
import struct
import sys
import zipfile

TABLE_OFF = 0xB4  # u16 kind, u16 table length, then 48-byte entries
ENTRY_LEN = 48
SECTION_HDR = 0x200  # per-section header (0x180 bytes, then zero padding)
PLAIN, COMPRESSED = 0xAC, 0xCA
MAGIC = b"hwcompress10"
DICT_SIZE = 0x2000  # LZMA dictionary size in every compressed section header (FINDINGS "Firmware image")
PKG_IN_ZIP = "data/accessory/firmware/para_data/firmware/btft0027/updatepkg.bin"


def hwdecompress(payload):
    """hwcompress10: [0x40][u32 BE raw size][0x40][LZMA props][u32 LE dict],
    then blocks of [u32 BE length][raw LZMA1 stream, 64 KiB of output]."""
    assert payload[0x20:0x2C] == MAGIC, payload[0x20:0x2C]
    p = 0x2C
    assert payload[p] == 0x40 and payload[p + 5] == 0x40, payload[p : p + 6].hex()
    total = struct.unpack(">I", payload[p + 1 : p + 5])[0]
    assert payload[p + 6] == 0x5D, "expected LZMA props lc=3 lp=0 pb=2"
    dict_size = struct.unpack("<I", payload[p + 7 : p + 11])[0]
    assert dict_size == DICT_SIZE, f"LZMA dictionary size {dict_size:#x}, FINDINGS says {DICT_SIZE:#x}"
    p += 11
    out = bytearray()
    while len(out) < total:
        n = struct.unpack(">I", payload[p : p + 4])[0]
        p += 4
        dec = lzma.LZMADecompressor(
            lzma.FORMAT_RAW,
            filters=[{"id": lzma.FILTER_LZMA1, "dict_size": dict_size, "lc": 3, "lp": 0, "pb": 2}],
        )
        out += dec.decompress(payload[p : p + n])
        assert dec.eof, f"LZMA block at {p:#x} did not end"
        p += n
    assert len(out) == total, (len(out), total)
    assert p == len(payload), f"{len(payload) - p} bytes after the last block"
    return bytes(out)


def main(pkg_path, out_dir):
    if pkg_path.endswith(".zip"):
        d = zipfile.ZipFile(pkg_path).read(PKG_IN_ZIP)
    else:
        d = open(pkg_path, "rb").read()
    hdr_len = struct.unpack(">I", d[4:8])[0]
    table_len = struct.unpack(">H", d[TABLE_OFF - 2 : TABLE_OFF])[0]
    assert table_len % ENTRY_LEN == 0, table_len
    entries = []
    for k in range(table_len // ENTRY_LEN):
        e = d[TABLE_OFF + k * ENTRY_LEN : TABLE_OFF + (k + 1) * ENTRY_LEN]
        sid, _, kind, stored, raw = struct.unpack(">HHIII", e[:16])
        entries.append((sid, kind, stored, raw, e[16:]))
    print(f"name={d[12:44].rstrip(b'\0').decode()} version={d[0x4C:0x6C].rstrip(b'\0').decode()} "
          f"header={hdr_len:#x} sections={len(entries)}")
    os.makedirs(out_dir, exist_ok=True)
    pos = hdr_len
    for sid, kind, stored, raw, digest in entries:
        assert kind in (PLAIN, COMPRESSED), f"section {sid}: unknown kind {kind:#x}"
        size = stored if kind == COMPRESSED else raw
        sec = d[pos : pos + size]
        assert struct.unpack(">H", sec[-2:])[0] == sid, f"section {sid}: trailer {sec[-2:].hex()}"
        payload = sec[SECTION_HDR:-2]
        open(os.path.join(out_dir, f"sec{sid:02}.bin"), "wb").write(sec)
        if kind == COMPRESSED:
            img = hwdecompress(payload)
            assert SECTION_HDR + len(img) + 2 == raw, f"section {sid}: size {len(img):#x}"
            open(os.path.join(out_dir, f"sec{sid:02}.img"), "wb").write(sec[:SECTION_HDR] + img)
            checked = img
        else:
            checked = payload
        assert hashlib.sha256(checked).digest() == digest, f"section {sid}: sha256 mismatch"
        print(f"sec{sid:02} {'lzma ' if kind == COMPRESSED else 'plain'} at {pos:#09x} "
              f"stored {size:#09x} image {len(checked):#09x} sha256 ok")
        pos += size
    assert len(d) - pos == 0x200, f"expected a 0x200-byte trailer, got {len(d) - pos:#x}"


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    main(sys.argv[1], sys.argv[2])
