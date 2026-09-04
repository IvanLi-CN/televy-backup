#!/usr/bin/env bash
set -euo pipefail

root_dir="$(git rev-parse --show-toplevel)"
brand_dir="${1:-$root_dir/assets/brand}"

python3 - "$brand_dir" <<'PY'
import re
import sys
import xml.etree.ElementTree as ET
from pathlib import Path

brand_dir = Path(sys.argv[1]).resolve()
expected = {
    "televybackup-logo.svg": ("#ffffff", "#263238", "#1677ff"),
    "televybackup-logo-dark.svg": ("none", "#74869c", "#5aa9ff"),
    "televybackup-logo-monochrome.svg": ("#ffffff", "#000000", "#000000"),
    "televybackup-logo-ui.svg": ("none", "#263238", "#1677ff"),
    "televybackup-logo-template.svg": ("none", "#000000", "#000000"),
}

def local_name(tag: str) -> str:
    return tag.rsplit("}", 1)[-1]

geometry = None
for filename, colors in expected.items():
    path = brand_dir / filename
    if not path.is_file():
        raise SystemExit(f"missing brand SVG: {path}")
    raw = path.read_text(encoding="utf-8")
    if re.search(r"<(?:image|use)\b|\b(?:href|xlink:href)=", raw, re.IGNORECASE):
        raise SystemExit(f"brand SVG contains embedded or linked raster/reference data: {path}")
    try:
        root = ET.fromstring(raw)
    except ET.ParseError as error:
        raise SystemExit(f"invalid brand SVG {path}: {error}")
    if local_name(root.tag) != "svg" or root.attrib.get("viewBox") != "0 0 1254 1254":
        raise SystemExit(f"unexpected SVG root/viewBox: {path}")
    elements = [element for element in root.iter() if local_name(element.tag) in {"rect", "path"}]
    if [local_name(element.tag) for element in elements] != ["rect", "path", "path"]:
        raise SystemExit(f"brand SVG must contain one rect and two paths: {path}")
    paths = tuple(element.attrib.get("d", "") for element in elements[1:])
    if not all(paths):
        raise SystemExit(f"brand SVG has an empty path: {path}")
    style = " ".join(element.text or "" for element in root.iter() if local_name(element.tag) == "style")
    found = tuple(re.search(rf"\.({name})\s*\{{\s*fill:\s*([^;]+);", style).group(2).lower() for name in ("canvas", "disk", "wing"))
    if found != colors:
        raise SystemExit(f"unexpected color parameters in {path}: {found}")
    if geometry is None:
        geometry = paths
    elif paths != geometry:
        raise SystemExit(f"brand SVG geometry differs from canonical paths: {path}")

print(f"brand SVG assets verified: {len(expected)} variants, shared geometry, no raster data")
PY
