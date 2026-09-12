#!/usr/bin/env python3
"""Verify the protected evidence required before publishing a stable macOS build."""

import argparse
import hashlib
import json
import sys


def fail(message: str) -> "NoReturn":
    raise SystemExit(f"macOS RC acceptance evidence rejected: {message}")


def required_string(value, name: str) -> str:
    if not isinstance(value, str) or not value:
        fail(f"{name} must be a non-empty string")
    return value


def required_identity(evidence, manifest, name: str, fields: tuple[str, ...]) -> None:
    if not isinstance(evidence, dict):
        fail(f"{name} identity must be an object")
    for field in fields:
        evidence_value = required_string(evidence.get(field), f"{name}.{field}")
        manifest_value = required_string(manifest.get(field), f"manifest {name}.{field}")
        if evidence_value != manifest_value:
            fail(f"{name}.{field} does not match BUILD-MANIFEST.json")


def equal_identity(first, second, name: str, fields: tuple[str, ...]) -> None:
    if not isinstance(first, dict) or not isinstance(second, dict):
        fail(f"{name} RC identities must be objects")
    for field in fields:
        first_value = required_string(first.get(field), f"{name}.rc1.{field}")
        second_value = required_string(second.get(field), f"{name}.rc2.{field}")
        if first_value != second_value:
            fail(f"{name}.{field} changed between RC1 and RC2")


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
equal_identity(
    root_evidence.get("rc1"),
    root_evidence.get("rc2"),
    "root_mount_helper",
    ("sha256", "artifact_sha256", "cdhash", "designated_requirement"),
)


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
        digest = hashlib.sha256(handle.read()).hexdigest()
    if asset.get("sha256") != digest:
        fail(f"{name} Universal DMG does not match its manifest")
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
    identity = tuple(required_string(helper.get(field), f"{name}.snapshot_access.{field}") for field in (
        "sha256", "artifact_sha256", "cdhash", "designated_requirement"
    ))
    return identity


rc_args = (
    args.rc1_manifest, args.rc1_checksums, args.rc1_dmg, args.rc1_source_commit,
    args.rc2_manifest, args.rc2_checksums, args.rc2_dmg, args.rc2_source_commit,
)
if any(value is not None for value in rc_args) and not all(value is not None for value in rc_args):
    fail("RC artifact verification arguments must be supplied as a complete pair")
if all(value is not None for value in rc_args):
    rc1_identity = verify_rc_artifact(
        args.rc1_manifest, args.rc1_checksums, args.rc1_dmg,
        f"{args.stable_version}-rc.1", args.rc1_source_commit, "RC1",
    )
    rc2_identity = verify_rc_artifact(
        args.rc2_manifest, args.rc2_checksums, args.rc2_dmg,
        f"{args.stable_version}-rc.2", args.rc2_source_commit, "RC2",
    )
    if rc1_identity != rc2_identity:
        fail("Snapshot Access helper identity changed between the RC release artifacts")
    final_identity = tuple(required_string(components["snapshot_access"].get(field), f"manifest snapshot_access.{field}") for field in (
        "sha256", "artifact_sha256", "cdhash", "designated_requirement"
    ))
    if final_identity != rc1_identity:
        fail("stable Snapshot Access identity does not match the accepted RC artifacts")

print("macOS RC1 to RC2 acceptance evidence verified")
