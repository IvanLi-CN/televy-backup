#!/usr/bin/env python3
"""Verify the protected evidence required before publishing a stable macOS build."""

import argparse
import hashlib
import json
import os
import re
import stat as stat_module
import subprocess
import sys
import tempfile
from pathlib import Path


def fail(message: str) -> "NoReturn":
    raise SystemExit(f"macOS RC acceptance evidence rejected: {message}")


def required_string(value, name: str) -> str:
    if not isinstance(value, str) or not value:
        fail(f"{name} must be a non-empty string")
    return value


def requirement_cdhashes(requirement: str, name: str) -> set[str]:
    values = {
        value.lower()
        for value in re.findall(r'\bcdhash\s+H"([0-9A-Fa-f]+)"', requirement)
    }
    if not values:
        fail(f"{name} designated requirement has no CDHash identities")
    return values


def validate_cdhash_identity(expected, actual, requirement: str, name: str) -> None:
    expected_value = required_string(expected, f"{name}.manifest_cdhash")
    actual_value = required_string(actual, f"{name}.actual_cdhash")
    values = requirement_cdhashes(requirement, name)
    if {expected_value.lower(), actual_value.lower()} - values:
        fail(f"{name} CDHash identity is not covered by its designated requirement")


def required_identity(evidence, manifest, name: str, fields: tuple[str, ...]) -> None:
    if not isinstance(evidence, dict):
        fail(f"{name} identity must be an object")
    manifest_requirement = required_string(
        manifest.get("designated_requirement"), f"manifest {name}.designated_requirement"
    )
    for field in fields:
        evidence_value = required_string(evidence.get(field), f"{name}.{field}")
        manifest_value = required_string(manifest.get(field), f"manifest {name}.{field}")
        if field == "cdhash":
            validate_cdhash_identity(manifest_value, evidence_value, manifest_requirement, name)
        elif evidence_value != manifest_value:
            fail(f"{name}.{field} does not match BUILD-MANIFEST.json")


def equal_identity(first, second, name: str, fields: tuple[str, ...]) -> None:
    if not isinstance(first, dict) or not isinstance(second, dict):
        fail(f"{name} RC identities must be objects")
    first_requirement = required_string(first.get("designated_requirement"), f"{name}.rc1.designated_requirement")
    second_requirement = required_string(second.get("designated_requirement"), f"{name}.rc2.designated_requirement")
    for field in fields:
        first_value = required_string(first.get(field), f"{name}.rc1.{field}")
        second_value = required_string(second.get(field), f"{name}.rc2.{field}")
        if field == "cdhash":
            if first_requirement != second_requirement:
                fail(f"{name}.designated_requirement changed between RC1 and RC2")
            validate_cdhash_identity(first_value, second_value, first_requirement, name)
        elif first_value != second_value:
            fail(f"{name}.{field} changed between RC1 and RC2")


def artifact_sha256(path: Path, canonical: bool = True) -> str:
    digest = hashlib.sha256()
    if path.is_file():
        digest.update(path.read_bytes())
        return digest.hexdigest()
    for root, directories, files in os.walk(path, followlinks=False):
        directories.sort()
        files.sort()
        relative_root = os.path.relpath(root, path)
        if relative_root == ".":
            relative_root = ""
        for name in directories + files:
            entry = Path(root) / name
            relative = os.path.join(relative_root, name)
            entry_stat = os.lstat(entry)
            if canonical:
                if stat_module.S_ISLNK(entry_stat.st_mode):
                    permissions = 0o777
                elif stat_module.S_ISDIR(entry_stat.st_mode) or entry_stat.st_mode & 0o111:
                    permissions = 0o755
                else:
                    permissions = 0o644
                mode = (entry_stat.st_mode & ~0o777) | permissions
            else:
                mode = entry_stat.st_mode
            digest.update(b"entry\0" + relative.encode() + b"\0")
            digest.update(str(mode).encode() + b"\0")
            if os.path.islink(entry):
                digest.update(b"link\0" + os.readlink(entry).encode() + b"\0")
            elif entry.is_file():
                digest.update(b"file\0" + entry.read_bytes())
            else:
                digest.update(b"other\0")
    return digest.hexdigest()


def command_output(command: list[str], name: str) -> str:
    result = subprocess.run(command, capture_output=True, text=True)
    if result.returncode != 0:
        detail = (result.stdout + result.stderr).strip()
        fail(f"{name} failed: {detail or result.returncode}")
    return result.stdout + result.stderr


def require_universal2(path: Path, name: str) -> None:
    architectures = command_output(["lipo", "-info", str(path)], f"{name} architecture check")
    if "arm64" not in architectures or "x86_64" not in architectures:
        fail(f"{name} must be a Universal 2 binary")


def helper_identity_from_dmg(dmg_path: str, name: str) -> dict[str, str | int]:
    mount_path = Path(tempfile.mkdtemp(prefix="televybackup-rc-"))
    mounted = False
    try:
        command_output(
            [
                "hdiutil",
                "attach",
                "-nobrowse",
                "-readonly",
                "-mountpoint",
                str(mount_path),
                dmg_path,
            ],
            f"{name} DMG mount",
        )
        mounted = True
        top_level_apps = sorted(
            path.name
            for path in mount_path.iterdir()
            if path.is_dir() and path.name.endswith(".app")
        )
        if top_level_apps != ["TelevyBackup.app"]:
            fail(f"{name} DMG must contain exactly one top-level TelevyBackup.app")
        main_binary = mount_path / "TelevyBackup.app/Contents/MacOS/TelevyBackup"
        if not main_binary.is_file():
            fail(f"{name} DMG is missing the main TelevyBackup executable")
        require_universal2(main_binary, f"{name} main app")
        helper = mount_path / "TelevyBackup.app" / "Contents/Library/LoginItems/TelevyBackup Snapshot Access.app"
        if not helper.is_dir():
            fail(f"{name} DMG is missing the embedded Snapshot Access app")
        binary = helper / "Contents/MacOS/televybackup-snapshot-access"
        if not binary.is_file():
            fail(f"{name} DMG is missing the Snapshot Access executable")
        require_universal2(binary, f"{name} Snapshot Access")
        signature = command_output(["codesign", "-dvvv", str(helper)], f"{name} Snapshot Access signature")
        if "Signature=adhoc" not in signature:
            fail(f"{name} Snapshot Access must use an ad-hoc signature")
        cdhash = next(
            (line.split("=", 1)[1].strip() for line in signature.splitlines() if line.startswith("CDHash=")),
            "",
        )
        requirement = next(
            (line.strip() for line in command_output(
                ["codesign", "-d", "-r-", str(helper)],
                f"{name} Snapshot Access designated requirement",
            ).splitlines() if line.startswith("designated =>")),
            "",
        )
        if not cdhash or not requirement:
            fail(f"{name} Snapshot Access signature identity is incomplete")
        cdhash_set = requirement_cdhashes(requirement, f"{name} Snapshot Access")
        if cdhash.lower() not in cdhash_set:
            fail(f"{name} Snapshot Access CDHash is not covered by its designated requirement")
        try:
            metadata = json.loads(
                command_output([str(binary), "--component-metadata"], f"{name} Snapshot Access metadata")
            )
        except json.JSONDecodeError as error:
            fail(f"{name} Snapshot Access metadata is invalid: {error}")
        return {
            "sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
            "artifact_sha256": artifact_sha256(helper),
            "artifact_sha256_legacy": artifact_sha256(helper, canonical=False),
            "cdhash": cdhash,
            "designated_requirement": requirement,
            "binary": "Contents/MacOS/televybackup-snapshot-access",
            "bundle_id": required_string(metadata.get("bundleId"), f"{name}.snapshot_access.bundle_id"),
            "relative_path": required_string(metadata.get("relativePath"), f"{name}.snapshot_access.relative_path"),
            "component_version": required_string(metadata.get("componentVersion"), f"{name}.snapshot_access.component_version"),
            "protocol_version": metadata.get("protocolVersion"),
        }
    finally:
        if mounted:
            subprocess.run(
                ["hdiutil", "detach", str(mount_path)],
                capture_output=True,
                text=True,
                check=False,
            )
        mount_path.rmdir()


parser = argparse.ArgumentParser()
parser.add_argument("--evidence", required=True)
parser.add_argument("--manifest", required=True)
parser.add_argument("--stable-version", required=True)
parser.add_argument("--rc1-tag", required=True)
parser.add_argument("--rc2-tag", required=True)
parser.add_argument("--stable-source-commit")
parser.add_argument("--rc1-manifest")
parser.add_argument("--rc1-checksums")
parser.add_argument("--rc1-dmg")
parser.add_argument("--rc1-source-commit")
parser.add_argument("--rc2-manifest")
parser.add_argument("--rc2-checksums")
parser.add_argument("--rc2-dmg")
parser.add_argument("--rc2-source-commit")
args = parser.parse_args()

try:
    evidence = json.loads(args.evidence)
    with open(args.manifest, encoding="utf-8") as handle:
        manifest = json.load(handle)
except (json.JSONDecodeError, OSError) as error:
    fail(str(error))

if not isinstance(evidence, dict):
    fail("root value must be an object")
if evidence.get("schema_version") != 1:
    fail("schema_version must be 1")
if evidence.get("product") != "TelevyBackup":
    fail("product must be TelevyBackup")
if evidence.get("stable_version") != args.stable_version:
    fail("stable_version does not match the release")
if evidence.get("rc1_tag") != args.rc1_tag or evidence.get("rc2_tag") != args.rc2_tag:
    fail("RC tags do not match the release sequence")
if manifest.get("release_version") != args.stable_version:
    fail("BUILD-MANIFEST.json has the wrong stable version")
if args.stable_source_commit and manifest.get("source_commit") != args.stable_source_commit:
    fail("BUILD-MANIFEST.json source_commit does not match the stable release source")

for field in (
    "legacy_registration_migrated",
    "strict_backup_rc1",
    "strict_backup_rc2",
    "fda_regrant_requested",
    "root_mount_helper_unchanged",
):
    if not isinstance(evidence.get(field), bool):
        fail(f"{field} must be boolean")
if not evidence["legacy_registration_migrated"]:
    fail("legacy registration was not migrated")
if not evidence["strict_backup_rc1"] or not evidence["strict_backup_rc2"]:
    fail("strict backup evidence is incomplete")
if evidence["fda_regrant_requested"]:
    fail("RC2 requested a new FDA grant")
if evidence["root_mount_helper_unchanged"] is not True:
    fail("root mount helper was not unchanged")
if evidence.get("fda_grants") != 1:
    fail("fda_grants must equal exactly 1")

components = manifest.get("components")
if not isinstance(components, dict):
    fail("manifest components are missing")
required_identity(
    evidence.get("snapshot_access"),
    components.get("snapshot_access", {}),
    "snapshot_access",
    ("sha256", "artifact_sha256", "cdhash", "designated_requirement"),
)

root_evidence = evidence.get("root_mount_helper")
root_manifest = components.get("snapshot_mount_helper", {})
if not isinstance(root_evidence, dict):
    fail("root_mount_helper identity must be an object")
if required_string(root_evidence.get("install_path"), "root_mount_helper.install_path") != required_string(
    root_manifest.get("install_path"), "manifest root_mount_helper.install_path"
):
    fail("root_mount_helper.install_path does not match BUILD-MANIFEST.json")
if root_evidence.get("component_version") != root_manifest.get("component_version"):
    compatible_versions = root_manifest.get("compatible_component_versions", [])
    if root_evidence.get("component_version") not in compatible_versions:
        fail("root_mount_helper.component_version is not compatible with BUILD-MANIFEST.json")
if root_evidence.get("protocol_version") != root_manifest.get("protocol_version"):
    fail("root_mount_helper.protocol_version does not match BUILD-MANIFEST.json")
root_identity_fields = ("sha256", "artifact_sha256", "cdhash", "designated_requirement")
for rc_name in ("rc1", "rc2"):
    required_identity(
        root_evidence.get(rc_name),
        root_manifest,
        f"root_mount_helper.{rc_name}",
        root_identity_fields,
    )
equal_identity(root_evidence.get("rc1"), root_evidence.get("rc2"), "root_mount_helper", root_identity_fields)


def verify_rc_artifact(
    manifest_path: str,
    checksums_path: str,
    dmg_path: str,
    expected_version: str,
    expected_source_commit: str | None,
    name: str,
):
    try:
        with open(manifest_path, encoding="utf-8") as handle:
            rc_manifest = json.load(handle)
        with open(checksums_path, encoding="utf-8") as handle:
            checksum_lines = handle.read().splitlines()
    except (OSError, json.JSONDecodeError) as error:
        fail(f"{name} release metadata cannot be read: {error}")
    if rc_manifest.get("release_version") != expected_version:
        fail(f"{name} manifest has the wrong release version")
    if expected_source_commit and rc_manifest.get("source_commit") != expected_source_commit:
        fail(f"{name} manifest source_commit does not match its tag")
    assets = rc_manifest.get("assets")
    if not isinstance(assets, list):
        fail(f"{name} manifest assets must be a list")
    dmg_name = dmg_path.rsplit("/", 1)[-1]
    asset = next(
        (item for item in assets if isinstance(item, dict) and item.get("name") == dmg_name),
        None,
    )
    if not isinstance(asset, dict):
        fail(f"{name} manifest does not describe its Universal DMG")

    with open(dmg_path, "rb") as handle:
        dmg_bytes = handle.read()
    digest = hashlib.sha256(dmg_bytes).hexdigest()
    if asset.get("sha256") != digest:
        fail(f"{name} Universal DMG does not match its manifest")
    if asset.get("bytes") != len(dmg_bytes):
        fail(f"{name} Universal DMG size does not match its manifest")
    checksum = next(
        (line.split()[0] for line in checksum_lines if line.rstrip().endswith("  " + dmg_name)),
        None,
    )
    if checksum != digest:
        fail(f"{name} Universal DMG does not match SHA256SUMS")
    components = rc_manifest.get("components")
    if not isinstance(components, dict):
        fail(f"{name} manifest components must be an object")
    helper = components.get("snapshot_access")
    if not isinstance(helper, dict):
        fail(f"{name} manifest snapshot_access component is missing")
    identity_fields = (
        "sha256", "artifact_sha256", "cdhash", "designated_requirement"
    )
    actual = helper_identity_from_dmg(dmg_path, name)
    for field in identity_fields + ("binary", "bundle_id", "relative_path", "component_version"):
        expected = required_string(helper.get(field), f"{name}.snapshot_access.{field}")
        if field == "artifact_sha256":
            matches_legacy = expected == actual["artifact_sha256_legacy"]
            matches_canonical = expected == actual["artifact_sha256"]
            if not (matches_canonical or matches_legacy):
                fail(f"{name}. Snapshot Access {field} does not match its BUILD-MANIFEST.json")
        elif field == "cdhash":
            validate_cdhash_identity(expected, actual[field], actual["designated_requirement"], f"{name}.snapshot_access")
        elif actual[field] != expected:
            fail(f"{name}. Snapshot Access {field} does not match its BUILD-MANIFEST.json")
    if helper.get("protocol_version") != actual["protocol_version"]:
        fail(f"{name}. Snapshot Access protocol_version does not match its BUILD-MANIFEST.json")
    actual_cdhashes = tuple(sorted(requirement_cdhashes(
        actual["designated_requirement"], f"{name} Snapshot Access"
    )))
    return (
        tuple(
            actual_cdhashes if field == "cdhash" else str(actual[field])
            for field in identity_fields
        ),
        {actual["artifact_sha256"], actual["artifact_sha256_legacy"]},
    )


rc_args = (
    args.rc1_manifest, args.rc1_checksums, args.rc1_dmg, args.rc1_source_commit,
    args.rc2_manifest, args.rc2_checksums, args.rc2_dmg, args.rc2_source_commit,
)
if any(value is not None for value in rc_args) and not all(value is not None for value in rc_args):
    fail("RC artifact verification arguments must be supplied as a complete pair")
if all(value is not None for value in rc_args):
    rc1_identity, rc1_artifact_digests = verify_rc_artifact(
        args.rc1_manifest, args.rc1_checksums, args.rc1_dmg,
        f"{args.stable_version}-rc.1", args.rc1_source_commit, "RC1",
    )
    rc2_identity, _ = verify_rc_artifact(
        args.rc2_manifest, args.rc2_checksums, args.rc2_dmg,
        f"{args.stable_version}-rc.2", args.rc2_source_commit, "RC2",
    )
    if rc1_identity != rc2_identity:
        fail("Snapshot Access helper identity changed between the RC release artifacts")
    final_manifest_identity = components["snapshot_access"]
    final_requirement = required_string(
        final_manifest_identity.get("designated_requirement"),
        "manifest snapshot_access.designated_requirement",
    )
    final_cdhash = required_string(final_manifest_identity.get("cdhash"), "manifest snapshot_access.cdhash")
    final_cdhashes = tuple(sorted(requirement_cdhashes(final_requirement, "manifest snapshot_access")))
    if final_cdhash.lower() not in final_cdhashes:
        fail("manifest snapshot_access.cdhash is not covered by its designated requirement")
    final_identity = tuple(
        final_cdhashes if field == "cdhash" else required_string(
            final_manifest_identity.get(field), f"manifest snapshot_access.{field}"
        )
        for field in ("sha256", "artifact_sha256", "cdhash", "designated_requirement")
    )
    if final_identity[0] != rc1_identity[0] or final_identity[2:] != rc1_identity[2:]:
        fail("stable Snapshot Access identity does not match the accepted RC artifacts")
    if final_identity[1] not in rc1_artifact_digests:
        fail("stable Snapshot Access artifact digest does not match the accepted RC artifacts")

print("macOS RC1 to RC2 acceptance evidence verified")
