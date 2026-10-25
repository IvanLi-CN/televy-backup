#!/usr/bin/env python3
"""Exercise the real bundled installer and both IPC peers on a disposable CI runner."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import plistlib
import socket
import subprocess
import sys
import tempfile
import shutil
import time
import uuid


LABEL = "com.ivan.televybackup.snapshot-mount-helper"
SERVICE = "system/" + LABEL
BINARY = Path("/Library/PrivilegedHelperTools") / LABEL
PLIST = Path("/Library/LaunchDaemons") / (LABEL + ".plist")
MOUNT_SOCKET = Path("/private/var/run/televybackup/snapshot-mount-helper.sock")
JOURNAL = Path("/var/db/televybackup/snapshot-mount-helper")


def run(*args, check=True):
    return subprocess.run(args, check=check, capture_output=True, text=True, timeout=30)


def request(path, version):
    request_id = str(uuid.uuid4())
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
        stream.settimeout(5)
        stream.connect(str(path))
        stream.sendall(json.dumps({"version": version, "request_id": request_id, "method": "status"}).encode() + b"\n")
        reply = json.loads(stream.makefile("rb").readline())
    assert reply["request_id"] == request_id, "IPC response identity mismatch"
    return reply


def wait_for(probe, assertion):
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        try:
            result = probe()
            if assertion(result):
                return result
        except (OSError, ValueError):
            pass
        time.sleep(0.2)
    raise AssertionError("helper did not reach the expected IPC state within 20 seconds")


def seed_legacy_stale_journal(uid):
    """Create one age-gated reservation row before the candidate helper starts."""
    lease_id = str(uuid.uuid4())
    volume_uuid = "00000000-0000-4000-8000-000000000001"
    device = "disk999s1"
    script = r'''
import os, pathlib, sqlite3, sys
journal_dir, uid, volume_uuid, device, lease_id = sys.argv[1:]
journal_dir = pathlib.Path(journal_dir)
journal_dir.mkdir(mode=0o700, parents=True, exist_ok=True)
db = journal_dir / "journal.sqlite"
connection = sqlite3.connect(db)
connection.execute(
    """
    CREATE TABLE mounts (
        lease_id TEXT PRIMARY KEY,
        uid INTEGER NOT NULL,
        volume_uuid TEXT NOT NULL,
        device_identifier TEXT NOT NULL,
        source_path TEXT NOT NULL DEFAULT '',
        snapshot_name TEXT NOT NULL,
        mount_root TEXT NOT NULL,
        mount_device INTEGER NOT NULL DEFAULT 0,
        snapshot_manifest TEXT NOT NULL,
        reservation_manifest TEXT NOT NULL DEFAULT '[]',
        reservation_manifest_valid INTEGER NOT NULL DEFAULT 0,
        state TEXT NOT NULL,
        created_at TEXT NOT NULL,
        last_renewed_at TEXT NOT NULL DEFAULT ''
    )
    """
)
connection.execute(
    """
    INSERT INTO mounts (
        lease_id, uid, volume_uuid, device_identifier, source_path,
        snapshot_name, mount_root, mount_device, snapshot_manifest,
        reservation_manifest, reservation_manifest_valid, state,
        created_at, last_renewed_at
    ) VALUES (?, ?, ?, ?, '', '', '', 0, '[]', '[]', 1, 'reserved',
              datetime('now', '-301 seconds'), datetime('now', '-301 seconds'))
    """,
    (lease_id, int(uid), volume_uuid, device),
)
connection.commit()
connection.close()
os.chmod(db, 0o600)
'''
    run(
        "sudo",
        "-n",
        "/usr/bin/python3",
        "-c",
        script,
        str(JOURNAL),
        str(uid),
        volume_uuid,
        device,
        lease_id,
    )
    return lease_id


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--app", required=True, type=Path)
    parser.add_argument("--require-readiness", action="store_true")
    parser.add_argument("--expect-build-commit")
    parser.add_argument("--delay-startup", action="store_true")
    parser.add_argument(
        "--legacy-blocking",
        action="store_true",
        help="make the legacy wire-compatible helper report one unclassified blocking row",
    )
    args = parser.parse_args()
    # Never run this integration test on an owner's machine.
    assert os.environ.get("GITHUB_ACTIONS") == "true", "GitHub Actions runner required"
    assert sys.platform == "darwin" and os.geteuid() != 0, "macOS user session required"
    app = args.app.resolve()
    cli = app / "Contents/MacOS/televybackup-cli"
    access = app / "Contents/Library/LoginItems/TelevyBackup Snapshot Access.app/Contents/MacOS/televybackup-snapshot-access"
    metadata = json.loads(run(str(access), "--component-metadata").stdout)
    expected_protocol = metadata["mountHelperProtocolVersion"]
    assert expected_protocol == 4, "unexpected published component protocol"
    assert not any(os.path.lexists(path) for path in (BINARY, PLIST, MOUNT_SOCKET, JOURNAL)), "runner already has a mount helper; refusing to replace it"
    absent = run("sudo", "-n", "/bin/launchctl", "print", SERVICE, check=False)
    assert absent.returncode != 0 and "could not find service" in (absent.stdout + absent.stderr).lower(), "runner helper absence unverified"
    run("sudo", "-n", "true")
    process = None
    journal_locker = None
    root = None
    # Preserve all fixture state if process shutdown cannot be verified.
    temp = tempfile.mkdtemp(prefix="televy-helper-install-ci-", dir="/private/tmp")
    fixture = Path(temp)
    os.chmod(fixture, 0o755)
    root = fixture / "root"
    run("sudo", "-n", "mkdir", str(root))
    run("sudo", "-n", "chmod", "755", str(root))
    config = fixture / "config"
    data = fixture / "data"
    config.mkdir(mode=0o700)
    data.mkdir(mode=0o700)
    (config / "config.toml").write_text("targets = []\n")
    access_socket = data / "snapshot-access/access.sock"
    # A pre-upgrade service replies with the historical protocol, not a mock
    # client. The bundled Access executable must surface the exact UI symptom.
    old_server = fixture / "old_server.py"
    old_server.write_text('''import json, os, socket
path = "/private/var/run/televybackup/snapshot-mount-helper.sock"
os.makedirs(os.path.dirname(path), mode=0o711, exist_ok=True)
server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
server.bind(path)
os.chmod(path, 0o666)
server.listen()
while True:
    connection, _ = server.accept()
    with connection:
        request = json.loads(connection.makefile("rb").readline())
        response = {"version":1,"request_id":request["request_id"],"ok":True,
                    "result":{"kind":"status","helper_version":"0.9.8","active_mounts":0,
                              "blocked_volume_uuids":[],"recovery_leases":[]}}
        connection.sendall(json.dumps(response).encode() + b"\\n")
''')
    if args.expect_build_commit:
        old_server.write_text(old_server.read_text().replace('"version":1', '"version":4')
            .replace('"helper_version":"0.9.8"', '"helper_version":"0.5.0","build_commit":"' + '0' * 40 + '"'))
    elif args.legacy_blocking:
        old_server.write_text(old_server.read_text().replace('"version":1', '"version":4')
            .replace('"helper_version":"0.9.8"', '"helper_version":"0.5.0"'))
    if args.legacy_blocking:
        old_server.write_text(old_server.read_text().replace('"active_mounts":0', '"active_mounts":1'))
    seed_plist = fixture / "old.plist"
    seed_plist.write_bytes(plistlib.dumps({
        "Label": LABEL,
        "ProgramArguments": ["/usr/bin/python3", str(old_server)],
        "RunAtLoad": True,
        "KeepAlive": True,
    }))
    root_plist = root / "old.plist"
    run("sudo", "-n", "cp", str(seed_plist), str(root_plist))
    run("sudo", "-n", "chown", "root:wheel", str(root_plist))
    run("sudo", "-n", "chmod", "644", str(root_plist))
    try:
        run("sudo", "-n", "/bin/launchctl", "bootstrap", "system", str(root_plist))
        wait_for(lambda: request(MOUNT_SOCKET, expected_protocol), lambda reply: reply["version"] == (4 if (args.expect_build_commit or args.legacy_blocking) else 1))
        env = os.environ.copy()
        env.update({
            "TELEVYBACKUP_CONFIG_DIR": str(config),
            "TELEVYBACKUP_DATA_DIR": str(data),
            "TELEVYBACKUP_SNAPSHOT_SOCKET": str(access_socket),
            "TELEVYBACKUP_SNAPSHOT_JOURNAL": str(data / "journal.sqlite"),
        })
        with (fixture / "access.log").open("w") as log:
            process = subprocess.Popen([str(access)], env=env, stdout=log, stderr=log)
        before = wait_for(lambda: request(access_socket, metadata["protocolVersion"]), lambda reply: reply.get("ok") is True)
        if args.expect_build_commit:
            stale = request(MOUNT_SOCKET, expected_protocol)["result"]
            assert stale["helper_version"] == "0.5.0" and stale["build_commit"] != args.expect_build_commit
            print("PROBE: same compatible component version has a stale running build", flush=True)
        elif args.legacy_blocking:
            legacy = request(MOUNT_SOCKET, expected_protocol)["result"]
            assert legacy["active_mounts"] == 1
            assert "pending_cleanup" not in legacy and "component_version" not in legacy
            print("REPRODUCED: legacy helper reports an unclassified blocking mount row", flush=True)
        else:
            assert before["result"]["mount_helper_reachable"] is False
            assert "unsupported mount helper response version" in before["result"]["mount_helper_error"]
            print("REPRODUCED: published Access reports unsupported mount helper response version for the legacy service", flush=True)
        if args.legacy_blocking:
            stale_lease = seed_legacy_stale_journal(os.getuid())
            print(
                "SEEDED: one age-gated legacy reservation row before candidate helper startup",
                flush=True,
            )
        # Same privileged CLI entrypoint as the GUI's password-authorized command.
        if args.delay_startup:
            assert args.require_readiness, "delayed startup requires the candidate readiness check"
            # Delay real helper initialization with a fixture-only SQLite lock.
            # No production sleep, command override, or TCC changes are needed.
            locker_script = fixture / "startup_lock.py"
            locker_ready = fixture / "startup-lock-ready"
            locker_script.write_text('''import os, pathlib, sqlite3, sys, time
journal = pathlib.Path(sys.argv[1])
ready = pathlib.Path(sys.argv[2])
journal.mkdir(mode=0o700)
with sqlite3.connect(journal / "journal.sqlite") as connection:
    os.chmod(journal / "journal.sqlite", 0o600)
    connection.execute("CREATE TABLE ci_startup_gate (value INTEGER)")
    connection.commit()
    connection.execute("BEGIN EXCLUSIVE")
    ready.touch()
    time.sleep(2)
    connection.commit()
''')
            journal_locker = subprocess.Popen([
                "sudo", "-n", "/usr/bin/python3", str(locker_script), str(JOURNAL), str(locker_ready)
            ])
            wait_for(lambda: locker_ready.exists(), bool)
        install_started = time.monotonic()
        installed = run("sudo", "-n", str(cli), "--json", "--config-dir", str(config), "--data-dir", str(data), "snapshot-mount-helper", "install")
        installation = json.loads(installed.stdout)
        assert installation["installed"] is True
        if args.require_readiness:
            assert installation["helperReady"] is True
            assert installation["helperVersion"] == "0.5.1"
        if journal_locker is not None:
            assert journal_locker.wait(timeout=10) == 0
            assert time.monotonic() - install_started >= 1, "startup delay did not cover the old subsecond retry window"
            print("PASS: real privileged installer waits for delayed journal initialization without a manual refresh", flush=True)
        direct = wait_for(lambda: request(MOUNT_SOCKET, expected_protocol), lambda reply: reply["version"] == expected_protocol and reply.get("ok") is True)
        assert direct["result"]["helper_version"] == "0.5.0"
        if args.legacy_blocking:
            candidate = direct["result"]
            assert candidate["active_mounts"] == 1
            assert candidate["recoverable_stale_mounts"] == 1
            assert candidate["unknown_blocking_mounts"] == 0
            assert [row["lease_id"] for row in candidate["recovery_leases"]] == [stale_lease]
            print(
                "PASS: CLI replacement starts the candidate helper and classifies the legacy journal row as recoverable",
                flush=True,
            )
        if args.expect_build_commit:
            assert direct["result"]["component_version"] == "0.5.1"
            candidate = app / "Contents/MacOS/televybackup-snapshot-mount-helper"
            digest = hashlib.sha256(candidate.read_bytes()).hexdigest()
            installed_digest = hashlib.sha256(BINARY.read_bytes()).hexdigest()
            assert direct["result"]["build_commit"] == args.expect_build_commit
            assert installation["buildCommit"] == args.expect_build_commit
            assert installation["binarySha256"] == digest == installed_digest
            print("PASS: installed file hash and running IPC build match the candidate", flush=True)
        after = wait_for(lambda: request(access_socket, metadata["protocolVersion"]), lambda reply: reply.get("result", {}).get("mount_helper_reachable") is True)
        assert after["result"]["mount_helper_error"] is None
        assert process.poll() is None, "Access unexpectedly exited during installation"
        print("PASS: real bundled install replaces the legacy service and the same Access process becomes reachable", flush=True)
    finally:
        if journal_locker is not None:
            # Preserve the journal/tree if its fixture lock owner cannot be reaped.
            journal_locker.wait(timeout=10)
        if process is not None:
            process.terminate()
            process.wait(timeout=10)
        run("sudo", "-n", "/bin/launchctl", "bootout", SERVICE, check=False)
        def absent(reply):
            return reply.returncode != 0 and "could not find service" in (reply.stdout + reply.stderr).lower()
        wait_for(lambda: run("sudo", "-n", "/bin/launchctl", "print", SERVICE, check=False), absent)
        # These targets were proved absent before this test created them.
        run("sudo", "-n", "rm", "-f", str(BINARY), str(PLIST), str(MOUNT_SOCKET), check=False)
        run("sudo", "-n", "rm", "-rf", str(JOURNAL), str(root), check=False)
        assert not any(os.path.lexists(path) for path in (BINARY, PLIST, MOUNT_SOCKET, JOURNAL)), "fixture cleanup incomplete"
        shutil.rmtree(fixture)


if __name__ == "__main__":
    main()
