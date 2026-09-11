#!/usr/bin/env python3
"""Verify the protected evidence required before publishing a stable macOS build."""

import argparse
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
    fail("root_mount_helper.component_version does not match BUILD-MANIFEST.json")
if root_evidence.get("protocol_version") != root_manifest.get("protocol_version"):
    fail("root_mount_helper.protocol_version does not match BUILD-MANIFEST.json")
equal_identity(
    root_evidence.get("rc1"),
    root_evidence.get("rc2"),
    "root_mount_helper",
    ("sha256", "artifact_sha256", "cdhash", "designated_requirement"),
)

print("macOS RC1 to RC2 acceptance evidence verified")
