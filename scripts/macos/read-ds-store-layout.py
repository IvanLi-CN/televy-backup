#!/usr/bin/env python3
"""Read the Finder geometry records needed by the DMG layout contract."""

import argparse
import json
import plistlib
import re
import struct
from pathlib import Path


class DSStoreReader:
    def __init__(self, path: Path):
        self.data = path.read_bytes()
        magic, signature, root_offset, root_size, root_offset_2, _ = struct.unpack_from(
            ">I4sIII16s", self.data, 0
        )
        if magic != 1 or signature != b"Bud1" or root_offset != root_offset_2:
            raise ValueError("not a valid DS_Store buddy file")
        root = self._block_at(root_offset, root_size)
        offset_count, _ = struct.unpack_from(">II", root, 0)
        table_count = (offset_count + 255) & ~255
        table_start = 8
        self.offsets = list(
            struct.unpack_from(f">{table_count}I", root, table_start)[:offset_count]
        )
        pos = table_start + table_count * 4
        toc_count = struct.unpack_from(">I", root, pos)[0]
        pos += 4
        self.toc = {}
        for _ in range(toc_count):
            name_length = root[pos]
            pos += 1
            name = bytes(root[pos : pos + name_length])
            pos += name_length
            self.toc[name] = struct.unpack_from(">I", root, pos)[0]
            pos += 4

    def _block(self, block_id: int) -> bytes:
        address = self.offsets[block_id]
        return self._block_at(address & ~0x1F, 1 << (address & 0x1F))

    def _block_at(self, address: int, size: int) -> bytes:
        start = address + 4
        end = start + size
        if end > len(self.data):
            raise ValueError("DS_Store block exceeds file bounds")
        return self.data[start:end]

    @staticmethod
    def _entry(block: bytes, pos: int) -> tuple[str, bytes, bytes, bytes, int]:
        name_length = struct.unpack_from(">I", block, pos)[0]
        pos += 4
        name_end = pos + name_length * 2
        name = block[pos:name_end].decode("utf-16be")
        pos = name_end
        code, type_code = struct.unpack_from(">4s4s", block, pos)
        pos += 8
        if type_code == b"blob":
            value_length = struct.unpack_from(">I", block, pos)[0]
            pos += 4
            value = block[pos : pos + value_length]
            pos += value_length
        elif type_code == b"long":
            value = block[pos : pos + 4]
            pos += 4
        elif type_code == b"shor":
            value = block[pos : pos + 2]
            pos += 2
        elif type_code == b"bool":
            value = block[pos : pos + 1]
            pos += 1
        elif type_code == b"ustr":
            value_length = struct.unpack_from(">I", block, pos)[0]
            pos += 4
            value = block[pos : pos + value_length * 2]
            pos += value_length * 2
        elif type_code == b"type":
            value = block[pos : pos + 4]
            pos += 4
        elif type_code == b"comp" or type_code == b"dutc":
            value = block[pos : pos + 8]
            pos += 8
        else:
            raise ValueError(f"unknown DS_Store record type: {type_code!r}")
        return name, code, type_code, value, pos

    def _walk(self, block_id: int):
        block = self._block(block_id)
        next_node, count = struct.unpack_from(">II", block, 0)
        pos = 8
        for _ in range(count):
            if next_node:
                child = struct.unpack_from(">I", block, pos)[0]
                pos += 4
                yield from self._walk(child)
            entry = self._entry(block, pos)
            pos = entry[4]
            yield entry[:4]
        if next_node:
            yield from self._walk(next_node)

    def records(self):
        super_block_id = self.toc.get(b"DSDB")
        if super_block_id is None:
            raise ValueError("DS_Store is missing its DSDB table")
        super_block = self._block(super_block_id)
        root_node, _, _, _, _ = struct.unpack_from(">IIIII", super_block, 0)
        return self._walk(root_node)


def parse_window_bounds(value: bytes) -> dict[str, list[int] | int]:
    payload = plistlib.loads(value)
    bounds = payload.get("WindowBounds")
    if not isinstance(bounds, str):
        raise ValueError("DS_Store is missing Finder WindowBounds")
    match = re.fullmatch(
        r"\{\{\s*(-?\d+)\s*,\s*(-?\d+)\s*\},\s*\{\s*(\d+)\s*,\s*(\d+)\s*\}\}",
        bounds,
    )
    if not match:
        raise ValueError(f"unsupported Finder WindowBounds: {bounds!r}")
    x, y, width, height = (int(value) for value in match.groups())
    return {"origin": [x, y], "width": width, "height": height}


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--store", required=True, type=Path)
    parser.add_argument("--layout", required=True, type=Path)
    args = parser.parse_args()
    layout = json.loads(args.layout.read_text(encoding="utf-8"))
    locations = {}
    window = None
    for name, code, type_code, value in DSStoreReader(args.store).records():
        if name == "." and code == b"bwsp" and type_code == b"blob":
            window = parse_window_bounds(value)
        elif code == b"Iloc" and type_code == b"blob" and len(value) >= 8:
            locations[name] = list(struct.unpack_from(">II", value, 0))
    expected_locations = layout["icon_locations"]
    if locations.get("TelevyBackup.app") != expected_locations["TelevyBackup.app"]:
        raise SystemExit("DS_Store TelevyBackup.app icon location differs from schema")
    if locations.get("Applications") != expected_locations["Applications"]:
        raise SystemExit("DS_Store Applications icon location differs from schema")
    if window != layout["window"]:
        raise SystemExit(f"DS_Store window differs from schema: {window!r}")
    print(json.dumps({"icon_locations": locations, "window": window}, sort_keys=True))


if __name__ == "__main__":
    main()
