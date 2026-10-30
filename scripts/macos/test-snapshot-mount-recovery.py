#!/usr/bin/env python3
"""Exercise stale mount classification and exact Release recovery on hosted macOS."""

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
    request_id = str(uuid.uuid4())
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
        stream.settimeout(15)
        stream.connect(str(path))
        stream.sendall(
            json.dumps(
                dict(version=4, request_id=request_id, method=method, **fields)
            ).encode()
            + b"\n"
        )
        reply = json.loads(stream.makefile("rb").readline())
    assert reply["request_id"] == request_id and reply["version"] == 4
    return reply


def write_fixture_rows(
    journal, uid, volume_uuid, device, stale_id, pending_id, unknown_id, malformed_id
):
    script = '''
import sqlite3, sys
journal, uid, volume_uuid, device, stale_id, pending_id, unknown_id, malformed_id = sys.argv[1:]
connection = sqlite3.connect(journal)
rows = [
    (stale_id, int(uid), volume_uuid, device, '', '', '', '[]', '[]', 1, 'reserved'),
    (pending_id, int(uid), volume_uuid, device, '', '', '', '[]', '[]', 1, 'cleanup_pending'),
    (unknown_id, int(uid), volume_uuid, device, '', '', '', '[]', 'not-json', 0, 'active'),
    (malformed_id, int(uid), volume_uuid, device, '', '', '', '[]', 'not-json', 0, 'cleanup_pending'),
]
connection.executemany(
    """
        INSERT INTO mounts (
            lease_id, uid, volume_uuid, device_identifier, source_path,
            snapshot_name, mount_root, snapshot_manifest, reservation_manifest,
            reservation_manifest_valid, state, created_at, last_renewed_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, datetime('now', '-301 seconds'), datetime('now', '-301 seconds'))
    """,
    rows,
)
connection.execute(
    "UPDATE mounts SET created_at = datetime('now'), last_renewed_at = datetime('now') WHERE lease_id = ?",
    (pending_id,),
)
connection.commit()
connection.close()
'''
    run(
        "sudo",
        "-n",
        "/usr/bin/python3",
        "-c",
        script,
        str(journal),
        str(uid),
        volume_uuid,
        device,
        stale_id,
        pending_id,
        unknown_id,
        malformed_id,
    )


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--helper", required=True, type=Path)
    args = parser.parse_args()
    assert os.environ.get("GITHUB_ACTIONS") == "true", "GitHub Actions required"
    assert sys.platform == "darwin" and os.geteuid() != 0, "macOS user session required"
    helper = args.helper.resolve(strict=True)
    fixture = Path(tempfile.mkdtemp(prefix="tbs-reconcile-", dir="/private/tmp"))
    os.chmod(fixture, 0o755)
    label = "com.ivan.televybackup.ci-reconcile-" + uuid.uuid4().hex
    service = "system/" + label
    mount = fixture / "volume"
    mount.mkdir()
    root = fixture / "root"
    image = fixture / "fixture.dmg"
    attached = loaded = False
    managed_root = Path("/private/var/tmp/televybackup-snapshot-mounts")
    managed_root_existed = os.path.lexists(managed_root)
    try:
        run(
            "hdiutil",
            "create",
            "-size",
            "128m",
            "-fs",
            "APFS",
            "-volname",
            "SyntheticReconcile",
            str(image),
        )
        run(
            "hdiutil",
            "attach",
            "-nobrowse",
            "-owners",
            "on",
            "-mountpoint",
            str(mount),
            str(image),
        )
        attached = True
        info = plistlib.loads(run("/usr/sbin/diskutil", "info", "-plist", str(mount)).stdout)
        device = info["DeviceIdentifier"]
        volume_uuid = info.get("APFSVolumeUUID", info.get("VolumeUUID"))
        assert volume_uuid and info["FilesystemName"].lower() == "apfs"
        run("sudo", "-n", "mkdir", str(root))
        run("sudo", "-n", "chmod", "755", str(root))
        launch = fixture / "service.plist"
        launch.write_bytes(
            plistlib.dumps(
                dict(
                    Label=label,
                    ProgramArguments=[str(helper)],
                    RunAtLoad=True,
                    EnvironmentVariables=dict(
                        TELEVYBACKUP_SNAPSHOT_MOUNT_SOCKET=str(root / "ipc" / "helper.sock"),
                        TELEVYBACKUP_SNAPSHOT_MOUNT_JOURNAL=str(root / "journal" / "state.sqlite"),
                    ),
                )
            )
        )
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
        journal = root / "journal" / "state.sqlite"
        stale_id = str(uuid.uuid4())
        pending_id = str(uuid.uuid4())
        unknown_id = str(uuid.uuid4())
        malformed_id = str(uuid.uuid4())
        write_fixture_rows(
            journal,
            os.getuid(),
            volume_uuid,
            device,
            stale_id,
            pending_id,
            unknown_id,
            malformed_id,
        )
        before = request(ipc, "status")
        assert before["ok"]
        status = before["result"]
        assert status["active_mounts"] == 4
        assert status["pending_cleanup"] == 1
        assert status["recoverable_stale_mounts"] == 1
        assert status["unknown_blocking_mounts"] == 2
        assert [row["lease_id"] for row in status["pending_cleanup_leases"]] == [pending_id]
        assert [row["lease_id"] for row in status["recovery_leases"]] == [stale_id]
        pending = request(ipc, "release", uid=os.getuid(), lease_id=pending_id)
        assert pending["ok"] and pending["result"]["cleanup_state"] == "released"
        released = request(ipc, "release", uid=os.getuid(), lease_id=stale_id)
        assert released["ok"] and released["result"]["cleanup_state"] == "released"
        after = request(ipc, "status")
        assert after["ok"]
        status = after["result"]
        assert status["active_mounts"] == 2
        assert status["pending_cleanup"] == 0
        assert status["recoverable_stale_mounts"] == 0
        assert status["unknown_blocking_mounts"] == 2
        assert not status["recovery_leases"]
        print(
            "PASS: stale verified row released through helper IPC; unknown row remained blocking",
            flush=True,
        )
    finally:
        if loaded:
            run("sudo", "-n", "launchctl", "bootout", service)
            stopped = run("sudo", "-n", "launchctl", "print", service, check=False)
            assert (
                stopped.returncode != 0 and b"Could not find service" in stopped.stderr
            ), "fixture shutdown unverified; preserving state"
        if not managed_root_existed and managed_root.exists():
            run("sudo", "-n", "rmdir", str(managed_root))
        if attached:
            run("hdiutil", "detach", str(mount))
        if root.exists():
            run("sudo", "-n", "rm", "-r", str(root))
        shutil.rmtree(fixture)
        print("CLEANUP: recovery fixture service, image and journal removed", flush=True)


if __name__ == "__main__":
    main()
