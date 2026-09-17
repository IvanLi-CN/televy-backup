#!/usr/bin/env python3
"""Render and verify the TelevyBackup Homebrew Cask release contract."""

from __future__ import annotations

import argparse
import hashlib
import json
import plistlib
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any


REPOSITORY = "IvanLi-CN/televy-backup"
PRODUCT = "TelevyBackup"
APP_BUNDLE_ID = "com.ivan.televybackup"
MIN_MACOS = (15, 0)
FINAL_VERSION_RE = re.compile(r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$")
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")


class CaskReleaseError(RuntimeError):
    """Raised when a release cannot be represented by the Cask."""


def read_json(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise CaskReleaseError(f"cannot read JSON {path}: {error}") from error
    if not isinstance(value, dict):
        raise CaskReleaseError(f"JSON value must be an object: {path}")
    return value


def final_version(value: str) -> str:
    if FINAL_VERSION_RE.fullmatch(value) is None:
        raise CaskReleaseError(f"Cask version must be a stable final version: {value!r}")
    return value


def parse_checksums(path: Path) -> dict[str, str]:
    records: dict[str, str] = {}
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except OSError as error:
        raise CaskReleaseError(f"cannot read checksums {path}: {error}") from error
    for line_number, line in enumerate(lines, 1):
        if not line.strip():
            continue
        fields = line.split()
        if len(fields) != 2 or SHA256_RE.fullmatch(fields[0]) is None:
            raise CaskReleaseError(f"malformed SHA256SUMS entry at line {line_number}")
        if fields[1] in records:
            raise CaskReleaseError(f"duplicate SHA256SUMS asset: {fields[1]}")
        records[fields[1]] = fields[0]
    if not records:
        raise CaskReleaseError("SHA256SUMS is empty")
    return records


def release_asset(version: str, checksums_path: Path, manifest_path: Path) -> tuple[str, str]:
    version = final_version(version)
    checksums = parse_checksums(checksums_path)
    manifest = read_json(manifest_path)
    if manifest.get("product") != PRODUCT:
        raise CaskReleaseError("BUILD-MANIFEST.json product is not TelevyBackup")
    if manifest.get("release_version") != version:
        raise CaskReleaseError("BUILD-MANIFEST.json release_version does not match the Cask version")
    architectures = manifest.get("architectures")
    if not isinstance(architectures, list) or "universal2" not in architectures:
        raise CaskReleaseError("stable Cask release must declare a Universal 2 artifact")
    name = f"TelevyBackup-{version}.dmg"
    digest = checksums.get(name)
    if digest is None:
        raise CaskReleaseError(f"SHA256SUMS is missing {name}")
    assets = manifest.get("assets")
    if not isinstance(assets, list):
        raise CaskReleaseError("BUILD-MANIFEST.json assets must be an array")
    matches = [asset for asset in assets if isinstance(asset, dict) and asset.get("name") == name]
    if len(matches) != 1:
        raise CaskReleaseError(f"BUILD-MANIFEST.json must contain exactly one {name}")
    asset = matches[0]
    if asset.get("sha256") != digest:
        raise CaskReleaseError(f"SHA256SUMS and BUILD-MANIFEST.json disagree for {name}")
    if not isinstance(asset.get("bytes"), int) or asset["bytes"] <= 0:
        raise CaskReleaseError(f"BUILD-MANIFEST.json has no valid byte count for {name}")
    return name, digest


def cask_text(version: str, digest: str) -> str:
    return f'''# frozen_string_literal: true

cask "televybackup" do
  version "{version}"
  sha256 "{digest}"

  url "https://github.com/{REPOSITORY}/releases/download/v#{{version}}/TelevyBackup-#{{version}}.dmg"
  name "TelevyBackup"
  desc "Encrypted backup client for macOS"
  homepage "https://github.com/{REPOSITORY}"

  depends_on macos: :sequoia

  app "TelevyBackup.app"

  caveats do
    <<~EOS
      TelevyBackup releases are ad-hoc signed and are not notarized by Apple. Homebrew
      leaves macOS quarantine intact. After verifying the download, open the app from
      Finder and approve it through macOS Gatekeeper if prompted.
    EOS
  end

  livecheck do
    url :url
    strategy :github_latest
  end
end
'''


def parse_cask(path: Path) -> dict[str, str]:
    try:
        text = path.read_text(encoding="utf-8")
    except OSError as error:
        raise CaskReleaseError(f"cannot read Cask {path}: {error}") from error
    patterns = {
        "name": r'^cask "([^"]+)" do$',
        "version": r'^  version "([^"]+)"$',
        "sha256": r'^  sha256 "([^"]+)"$',
        "url": r'^  url "([^"]+)"$',
    }
    values: dict[str, str] = {}
    for key, pattern in patterns.items():
        matches = re.findall(pattern, text, re.MULTILINE)
        if len(matches) != 1:
            raise CaskReleaseError(f"Cask must contain exactly one {key} declaration")
        values[key] = matches[0]
    if values["name"] != "televybackup":
        raise CaskReleaseError("unexpected Cask name")
    if '  app "TelevyBackup.app"' not in text:
        raise CaskReleaseError("Cask must install TelevyBackup.app")
    if '  depends_on macos: :sequoia' not in text:
        raise CaskReleaseError("Cask must require macOS Sequoia or newer")
    if "com.ivan.televybackup.dev" in text or "packaging/homebrew" in text:
        raise CaskReleaseError("Cask contains a development or legacy path")
    return values


def verify_cask(path: Path, version: str, checksums_path: Path, manifest_path: Path) -> None:
    name, digest = release_asset(version, checksums_path, manifest_path)
    values = parse_cask(path)
    if values["version"] != version or values["sha256"] != digest:
        raise CaskReleaseError("Cask version or SHA-256 does not match the stable release")
    expected_url = f"https://github.com/{REPOSITORY}/releases/download/v#{{version}}/TelevyBackup-#{{version}}.dmg"
    if values["url"] != expected_url:
        raise CaskReleaseError("Cask URL does not use the canonical same-repository Release asset")


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    try:
        with path.open("rb") as handle:
            for chunk in iter(lambda: handle.read(1024 * 1024), b""):
                digest.update(chunk)
    except OSError as error:
        raise CaskReleaseError(f"cannot read DMG {path}: {error}") from error
    return digest.hexdigest()


def command_output(args: list[str]) -> str:
    try:
        result = subprocess.run(args, check=True, text=True, capture_output=True)
    except (OSError, subprocess.CalledProcessError) as error:
        detail = getattr(error, "stderr", "") or str(error)
        raise CaskReleaseError(f"command failed: {' '.join(args)}: {detail.strip()}") from error
    return result.stdout


def macos_version(value: object) -> tuple[int, ...]:
    if not isinstance(value, str):
        raise CaskReleaseError("app LSMinimumSystemVersion is missing")
    match = re.fullmatch(r"(\d+)(?:\.(\d+))?(?:\.(\d+))?", value)
    if match is None:
        raise CaskReleaseError(f"invalid LSMinimumSystemVersion: {value!r}")
    return tuple(int(part or 0) for part in match.groups())


def verify_dmg(dmg: Path, version: str, checksums_path: Path, manifest_path: Path) -> None:
    name, expected_digest = release_asset(version, checksums_path, manifest_path)
    if dmg.name != name:
        raise CaskReleaseError(f"downloaded DMG must be named {name}")
    actual_digest = sha256_file(dmg)
    if actual_digest != expected_digest:
        raise CaskReleaseError("downloaded DMG SHA-256 does not match SHA256SUMS")
    command_output(["hdiutil", "verify", str(dmg)])
    directory = Path(tempfile.mkdtemp(prefix="televybackup-cask-"))
    mount_point = directory.resolve()
    device: str | None = None
    try:
        try:
            attach = command_output(
                [
                    "hdiutil",
                    "attach",
                    "-plist",
                    "-nobrowse",
                    "-readonly",
                    "-mountpoint",
                    str(mount_point),
                    str(dmg),
                ]
            )
            payload = plistlib.loads(attach.encode("utf-8"))
            entities = payload.get("system-entities", [])
            device = next(
                (
                    entity.get("dev-entry")
                    for entity in entities
                    if entity.get("mount-point") == str(mount_point) and entity.get("dev-entry")
                ),
                None,
            )
            if not isinstance(device, str):
                device = next(
                    (
                        entity.get("dev-entry")
                        for entity in entities
                        if entity.get("dev-entry")
                        and isinstance(entity.get("mount-point"), str)
                        and Path(entity["mount-point"]).resolve() == mount_point
                    ),
                    None,
                )
            if not isinstance(device, str):
                raise CaskReleaseError("hdiutil did not return the exact mounted device")
            app = mount_point / "TelevyBackup.app"
            info_path = app / "Contents/Info.plist"
            if not app.is_dir() or not info_path.is_file():
                raise CaskReleaseError("DMG does not contain TelevyBackup.app")
            with info_path.open("rb") as handle:
                info = plistlib.load(handle)
            if info.get("CFBundleIdentifier") != APP_BUNDLE_ID:
                raise CaskReleaseError("DMG app bundle id is not the prod bundle id")
            if macos_version(info.get("LSMinimumSystemVersion")) < MIN_MACOS:
                raise CaskReleaseError("DMG app minimum macOS version is below macOS 15")
            executable = str(info.get("CFBundleExecutable") or "TelevyBackup")
            binary = app / "Contents/MacOS" / executable
            if not binary.is_file():
                raise CaskReleaseError("DMG app executable is missing")
            architecture_info = command_output(["lipo", "-info", str(binary)])
            if "arm64" not in architecture_info or "x86_64" not in architecture_info:
                raise CaskReleaseError("DMG app executable is not Universal 2")
            print(
                json.dumps(
                    {
                        "asset": name,
                        "bundle_id": info["CFBundleIdentifier"],
                        "architectures": ["arm64", "x86_64"],
                        "minimum_macos": info["LSMinimumSystemVersion"],
                        "sha256": actual_digest,
                        "version": version,
                    },
                    sort_keys=True,
                )
            )
        finally:
            detach_target = device or str(mount_point)
            try:
                command_output(["hdiutil", "detach", detach_target])
            except CaskReleaseError:
                if device is not None:
                    raise
    finally:
        shutil.rmtree(directory, ignore_errors=True)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)

    render = subparsers.add_parser("render")
    render.add_argument("--version", required=True)
    render.add_argument("--checksums", type=Path, required=True)
    render.add_argument("--manifest", type=Path, required=True)
    render.add_argument("--output", type=Path, required=True)

    inspect = subparsers.add_parser("cask-version")
    inspect.add_argument("--cask", type=Path, required=True)

    verify = subparsers.add_parser("verify-cask")
    verify.add_argument("--cask", type=Path, required=True)
    verify.add_argument("--version", required=True)
    verify.add_argument("--checksums", type=Path, required=True)
    verify.add_argument("--manifest", type=Path, required=True)

    dmg = subparsers.add_parser("verify-dmg")
    dmg.add_argument("--dmg", type=Path, required=True)
    dmg.add_argument("--version", required=True)
    dmg.add_argument("--checksums", type=Path, required=True)
    dmg.add_argument("--manifest", type=Path, required=True)

    args = parser.parse_args(argv)
    try:
        if args.command == "render":
            _, digest = release_asset(args.version, args.checksums, args.manifest)
            args.output.write_text(cask_text(args.version, digest), encoding="utf-8")
        elif args.command == "cask-version":
            print(parse_cask(args.cask)["version"])
        elif args.command == "verify-cask":
            verify_cask(args.cask, args.version, args.checksums, args.manifest)
        elif args.command == "verify-dmg":
            verify_dmg(args.dmg, args.version, args.checksums, args.manifest)
        return 0
    except CaskReleaseError as error:
        print(f"cask_release.py: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
