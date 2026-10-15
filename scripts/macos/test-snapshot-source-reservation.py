#!/usr/bin/env python3
"""Replay production Reserve on synthetic APFS storage, only on hosted CI."""

import argparse
import json
import os
from pathlib import Path
import plistlib
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import uuid


def run(*args, check=True):
    return subprocess.run(args, capture_output=True, check=check, timeout=45)


def request(path, method, **fields):
    identity = str(uuid.uuid4())
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
        stream.settimeout(15)
        stream.connect(str(path))
        stream.sendall(json.dumps(dict(version=4, request_id=identity, method=method, **fields)).encode() + b"\n")
        reply = json.loads(stream.makefile("rb").readline())
    assert reply["request_id"] == identity and reply["version"] == 4
    return reply


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--helper", required=True, type=Path)
    parser.add_argument("--expect", required=True, choices=("legacy", "fixed"))
    args = parser.parse_args()
    assert os.environ.get("GITHUB_ACTIONS") == "true", "GitHub Actions required"
    assert sys.platform == "darwin" and os.geteuid() != 0, "macOS user session required"
    helper = args.helper.resolve(strict=True)
    fixture = Path(tempfile.mkdtemp(prefix="tbs-", dir="/private/tmp"))
    os.chmod(fixture, 0o755)
    label = "com.ivan.televybackup.ci-source-" + uuid.uuid4().hex
    service = "system/" + label
    mount = fixture / "volume"
    mount.mkdir()
    root = fixture / "root"
    image = fixture / "fixture.dmg"
    attached = loaded = False
    managed_root = Path("/private/var/tmp/televybackup-snapshot-mounts")
    managed_root_existed = os.path.lexists(managed_root)
    try:
        run("hdiutil", "create", "-size", "128m", "-fs", "APFS", "-volname", "SyntheticReserve", str(image))
        run("hdiutil", "attach", "-nobrowse", "-owners", "on", "-mountpoint", str(mount), str(image))
        attached = True
        source = mount / "nested" / "source"
        source.mkdir(parents=True)
        info = plistlib.loads(run("/usr/sbin/diskutil", "info", "-plist", str(mount)).stdout)
        device = info["DeviceIdentifier"]
        volume_uuid = info.get("APFSVolumeUUID", info.get("VolumeUUID"))
        assert volume_uuid and info["FilesystemName"].lower() == "apfs"
        direct = run("/usr/sbin/diskutil", "info", "-plist", str(source), check=False)
        inventory = run("/usr/sbin/diskutil", "apfs", "listSnapshots", "-plist", device, check=False)
        inventory_keys = sorted(plistlib.loads(inventory.stdout)) if inventory.stdout.startswith(b"<?xml") else []
        print(f"PROBE: device-info=0 nested-info={direct.returncode} nested-stderr-bytes={len(direct.stderr)} inventory-exit={inventory.returncode} inventory-keys={inventory_keys}", flush=True)
        run("sudo", "-n", "mkdir", str(root))
        run("sudo", "-n", "chmod", "755", str(root))
        launch = fixture / "service.plist"
        launch.write_bytes(plistlib.dumps(dict(
            Label=label, ProgramArguments=[str(helper)], RunAtLoad=True,
            EnvironmentVariables=dict(
                TELEVYBACKUP_SNAPSHOT_MOUNT_SOCKET=str(root / "ipc" / "helper.sock"),
                TELEVYBACKUP_SNAPSHOT_MOUNT_JOURNAL=str(root / "journal" / "state.sqlite")),
        )))
        root_plist = root / "service.plist"
        run("sudo", "-n", "cp", str(launch), str(root_plist))
        run("sudo", "-n", "chown", "root:wheel", str(root_plist))
        run("sudo", "-n", "chmod", "644", str(root_plist))
        run("sudo", "-n", "launchctl", "bootstrap", "system", str(root_plist))
        loaded = True
        ipc = root / "ipc" / "helper.sock"
        deadline = time.monotonic() + 15
        while not ipc.exists() and time.monotonic() < deadline:
            time.sleep(0.1)
        assert request(ipc, "status")["ok"], "production helper failed to start"
        if args.expect == "fixed":
            assert request(ipc, "status")["result"]["build_commit"] == os.environ["GITHUB_SHA"]
            link = mount / "link"
            link.symlink_to(source, target_is_directory=True)
            denied = mount / "denied"
            denied.mkdir()
            run("sudo", "-n", "chown", "root:wheel", str(denied))
            run("sudo", "-n", "chmod", "700", str(denied))
            cases = (
                dict(volume_uuid=str(uuid.uuid4())),
                dict(device_identifier="disk999999s1"),
                dict(source_path=str(link)),
                dict(source_path=str(mount / "missing")),
                dict(source_path=str(denied)),
                dict(uid=os.getuid() + 1),
            )
            for override in cases:
                fields = dict(uid=os.getuid(), lease_id=str(uuid.uuid4()), volume_uuid=volume_uuid,
                              device_identifier=device, source_path=str(source))
                fields.update(override)
                reply = request(ipc, "reserve", **fields)
                assert not reply["ok"], "unsafe Reserve was accepted"
                assert str(mount) not in reply.get("message", ""), "diagnostic leaked source path"
            assert request(ipc, "status")["result"]["active_mounts"] == 0
            print("PASS: wrong UUID/device, symlink, missing source, denied directory and denied peer rejected without a lease", flush=True)
        for attempt in range(2):
            lease_id = str(uuid.uuid4())
            reply = request(ipc, "reserve", uid=os.getuid(), lease_id=lease_id,
                            volume_uuid=volume_uuid, device_identifier=device, source_path=str(source))
            if args.expect == "legacy":
                assert direct.returncode == 1, "nested diskutil hypothesis not reproduced"
                assert not reply["ok"] and reply["code"] == "command_failed", "unexpected Reserve failure stage"
                assert "/usr/sbin/diskutil failed (exit status: 1)" in reply["message"]
                print(f"REPRODUCED {attempt + 1}: production Reserve -> command_failed -> diskutil exit 1", flush=True)
            else:
                assert reply["ok"], reply.get("message")
                assert reply["result"]["cleanup_state"] == "reserved"
                query = "SELECT uid,volume_uuid,device_identifier,source_path,state,reservation_manifest,reservation_manifest_valid FROM mounts WHERE lease_id=?"
                reader = "import sqlite3,json,sys; print(json.dumps(sqlite3.connect(sys.argv[1]).execute(sys.argv[2],(sys.argv[3],)).fetchone()))"
                row = json.loads(run("sudo", "-n", "/usr/bin/python3", "-c", reader, str(root / "journal" / "state.sqlite"), query, lease_id).stdout)
                assert row == [os.getuid(), volume_uuid, device, str(source), "reserved", "[]", 1]
                after = plistlib.loads(run("/usr/sbin/diskutil", "apfs", "listSnapshots", "-plist", device).stdout)
                assert after == plistlib.loads(inventory.stdout), "Reserve created or changed snapshots"
                print("PASS: production Reserve accepted nested source; journal identity and empty baseline match; no snapshot created", flush=True)
                break
    finally:
        # Never remove executable/journal state until launchd proves shutdown.
        if loaded:
            run("sudo", "-n", "launchctl", "bootout", service)
            stopped = run("sudo", "-n", "launchctl", "print", service, check=False)
            assert stopped.returncode != 0 and b"Could not find service" in stopped.stderr, "fixture shutdown unverified; preserving state"
        if not managed_root_existed and managed_root.exists():
            run("sudo", "-n", "rmdir", str(managed_root))
        if attached:
            run("hdiutil", "detach", str(mount))
        if root.exists():
            run("sudo", "-n", "rm", "-r", str(root))
        shutil.rmtree(fixture)
        print("CLEANUP: fixture service, image, journal and source removed", flush=True)


if __name__ == "__main__":
    main()
