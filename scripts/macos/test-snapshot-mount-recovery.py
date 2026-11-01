#!/usr/bin/env python3
"""Exercise stale mount classification and daemon inventory reconciliation on hosted macOS."""

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


def run(*args, check=True, env=None):
    return subprocess.run(args, capture_output=True, check=check, timeout=45, env=env)


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


def control_request(path, method, params=None):
    request_id = str(uuid.uuid4())
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
        stream.settimeout(15)
        stream.connect(str(path))
        stream.sendall(
            json.dumps(
                {
                    "type": "control.request",
                    "id": request_id,
                    "method": method,
                    "params": params or {},
                }
            ).encode()
            + b"\n"
        )
        reply = json.loads(stream.makefile("rb").readline())
    assert reply["id"] == request_id and reply["type"] == "control.response"
    return reply


def wait_for_operation(path, operation_id):
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        reply = control_request(
            path,
            "operation.get",
            {"operationId": operation_id},
        )
        assert reply["ok"], reply
        result = reply["result"]
        if result["state"] in ("succeeded", "failed"):
            return result
        time.sleep(0.1)
    raise AssertionError("daemon operation did not reach a terminal state")


def write_fixture_rows(
    journal,
    uid,
    volume_uuid,
    device,
    stale_id,
    pending_id,
    unknown_id,
    malformed_id,
    released_drift_id,
    future_state_id,
    malformed_released_id,
):
    script = '''
import sqlite3, sys
journal, uid, volume_uuid, device, stale_id, pending_id, unknown_id, malformed_id, released_drift_id, future_state_id, malformed_released_id = sys.argv[1:]
connection = sqlite3.connect(journal)
rows = [
    (stale_id, int(uid), volume_uuid, device, '', '', '', '[]', '[]', 1, 'reserved'),
    (pending_id, int(uid), volume_uuid, device, '', '', '', '[]', '[]', 1, 'cleanup_pending'),
    (unknown_id, int(uid), volume_uuid, device, '', '', '', '[]', 'not-json', 0, 'active'),
    (malformed_id, int(uid), volume_uuid, device, '', '', '', '[]', 'not-json', 0, 'cleanup_pending'),
    (released_drift_id, int(uid), volume_uuid, device, '', '', '', '[]', '[{"device_identifier":"%s","uuid":"00000000-0000-0000-0000-000000000001"}]' % device, 1, 'released'),
    (future_state_id, int(uid), volume_uuid, device, '', '', '', '[]', '[]', 1, 'future_state'),
    (malformed_released_id, int(uid), volume_uuid, device, '', '', '', '[]', 'not-json', 0, 'released'),
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
        released_drift_id,
        future_state_id,
        malformed_released_id,
    )


def delete_fixture_rows(journal, lease_ids):
    script = """
import sqlite3, sys
journal, *lease_ids = sys.argv[1:]
connection = sqlite3.connect(journal)
connection.executemany("DELETE FROM mounts WHERE lease_id = ?", ((lease_id,) for lease_id in lease_ids))
connection.commit()
connection.close()
"""
    run(
        "sudo",
        "-n",
        "/usr/bin/python3",
        "-c",
        script,
        str(journal),
        *lease_ids,
    )


def insert_inventory_drift_row(journal, uid, volume_uuid, device, lease_id):
    script = '''
import sqlite3, sys
journal, uid, volume_uuid, device, lease_id = sys.argv[1:]
connection = sqlite3.connect(journal)
connection.execute(
    """
        INSERT INTO mounts (
            lease_id, uid, volume_uuid, device_identifier, source_path,
            snapshot_name, mount_root, snapshot_manifest, reservation_manifest,
            reservation_manifest_valid, state, created_at, last_renewed_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, datetime('now', '-301 seconds'), datetime('now', '-301 seconds'))
    """,
    (
        lease_id,
        int(uid),
        volume_uuid,
        device,
        '',
        '',
        '',
        '[]',
        '[{"device_identifier":"%s","uuid":"00000000-0000-0000-0000-000000000001"}]' % device,
        1,
        'released',
    ),
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
        lease_id,
    )


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--helper", required=True, type=Path)
    parser.add_argument("--access", required=True, type=Path)
    parser.add_argument("--daemon", required=True, type=Path)
    parser.add_argument("--cli", required=True, type=Path)
    args = parser.parse_args()
    assert os.environ.get("GITHUB_ACTIONS") == "true", "GitHub Actions required"
    assert sys.platform == "darwin" and os.geteuid() != 0, "macOS user session required"
    helper = args.helper.resolve(strict=True)
    access = args.access.resolve(strict=True)
    daemon = args.daemon.resolve(strict=True)
    cli = args.cli.resolve(strict=True)
    fixture = Path(tempfile.mkdtemp(prefix="tbs-reconcile-", dir="/private/tmp"))
    os.chmod(fixture, 0o755)
    label = "com.ivan.televybackup.ci-reconcile-" + uuid.uuid4().hex
    service = "system/" + label
    mount = fixture / "volume"
    mount.mkdir()
    source = mount / "nested" / "source"
    source.mkdir(parents=True)
    root = fixture / "root"
    image = fixture / "fixture.dmg"
    attached = loaded = False
    access_process = daemon_process = None
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
        released_drift_id = str(uuid.uuid4())
        future_state_id = str(uuid.uuid4())
        malformed_released_id = str(uuid.uuid4())
        write_fixture_rows(
            journal,
            os.getuid(),
            volume_uuid,
            device,
            stale_id,
            pending_id,
            unknown_id,
            malformed_id,
            released_drift_id,
            future_state_id,
            malformed_released_id,
        )
        before = request(ipc, "status")
        assert before["ok"]
        status = before["result"]
        assert status["active_mounts"] == 7
        assert status["pending_cleanup"] == 1
        assert status["recoverable_stale_mounts"] == 1
        assert status["unknown_blocking_mounts"] == 4
        assert status["inventory_drift_mounts"] == 1
        assert [row["lease_id"] for row in status["pending_cleanup_leases"]] == [pending_id]
        assert [row["lease_id"] for row in status["recovery_leases"]] == [stale_id]
        assert [row["lease_id"] for row in status["inventory_drift_leases"]] == [released_drift_id]
        blocked_release = request(
            ipc,
            "reserve",
            uid=os.getuid(),
            lease_id=str(uuid.uuid4()),
            volume_uuid=volume_uuid,
            device_identifier=device,
            source_path=str(source),
        )
        assert not blocked_release["ok"]
        delete_fixture_rows(journal, [malformed_released_id, released_drift_id])
        blocked_unknown_state = request(
            ipc,
            "reserve",
            uid=os.getuid(),
            lease_id=str(uuid.uuid4()),
            volume_uuid=volume_uuid,
            device_identifier=device,
            source_path=str(source),
        )
        assert not blocked_unknown_state["ok"]
        insert_inventory_drift_row(
            journal, os.getuid(), volume_uuid, device, released_drift_id
        )
        pending = request(ipc, "release", uid=os.getuid(), lease_id=pending_id)
        assert pending["ok"] and pending["result"]["cleanup_state"] == "released"
        released = request(ipc, "release", uid=os.getuid(), lease_id=stale_id)
        assert released["ok"] and released["result"]["cleanup_state"] == "released"
        after = request(ipc, "status")
        assert after["ok"]
        status = after["result"]
        assert status["active_mounts"] == 4
        assert status["pending_cleanup"] == 0
        assert status["recoverable_stale_mounts"] == 0
        assert status["unknown_blocking_mounts"] == 3
        assert status["inventory_drift_mounts"] == 1
        assert not status["recovery_leases"]
        assert [row["lease_id"] for row in status["inventory_drift_leases"]] == [released_drift_id]

        config = fixture / "config"
        data = fixture / "data"
        config.mkdir()
        data.mkdir()
        template = Path(__file__).resolve().parents[2] / "scripts/macos/fixtures/perf-idle/config.toml"
        config.joinpath("config.toml").write_text(
            template.read_text(encoding="utf-8")
            .replace("__FIXTURE_A__", str(mount))
            .replace("__FIXTURE_B__", str(mount)),
            encoding="utf-8",
        )
        access_socket = data / "snapshot-access" / "access.sock"
        access_journal = data / "snapshot-access" / "journal.sqlite"
        access_environment = {
            **os.environ,
            "TELEVYBACKUP_CONFIG_DIR": str(config),
            "TELEVYBACKUP_DATA_DIR": str(data),
            "TELEVYBACKUP_DISABLE_KEYCHAIN": "1",
            "TELEVYBACKUP_SNAPSHOT_SOCKET": str(access_socket),
            "TELEVYBACKUP_SNAPSHOT_JOURNAL": str(access_journal),
            "TELEVYBACKUP_SNAPSHOT_MOUNT_SOCKET": str(ipc),
            "TELEVYBACKUP_SNAPSHOT_MOUNT_JOURNAL": str(journal),
        }
        access_log = fixture / "snapshot-access.log"
        access_stream = access_log.open("wb")
        access_process = subprocess.Popen(
            [str(access)],
            env=access_environment,
            stdout=access_stream,
            stderr=subprocess.STDOUT,
        )
        deadline = time.monotonic() + 15
        while not access_socket.exists() and time.monotonic() < deadline:
            if access_process.poll() is not None:
                raise AssertionError("Snapshot Access exited before its isolated IPC became ready")
            time.sleep(0.1)
        assert access_socket.exists(), "Snapshot Access IPC did not become ready"
        control = data / "ipc" / "control.sock"
        daemon_environment = {
            **os.environ,
            "TELEVYBACKUP_CONFIG_DIR": str(config),
            "TELEVYBACKUP_DATA_DIR": str(data),
            "TELEVYBACKUP_DISABLE_KEYCHAIN": "1",
            "TELEVYBACKUP_SNAPSHOT_MOUNT_SOCKET": str(ipc),
            "TELEVYBACKUP_SNAPSHOT_MOUNT_JOURNAL": str(journal),
        }
        daemon_log = fixture / "daemon.log"
        daemon_stream = daemon_log.open("wb")
        daemon_process = subprocess.Popen(
            [str(daemon)],
            env=daemon_environment,
            stdout=daemon_stream,
            stderr=subprocess.STDOUT,
        )
        deadline = time.monotonic() + 15
        while not control.exists() and time.monotonic() < deadline:
            if daemon_process.poll() is not None:
                raise AssertionError("candidate daemon exited before control IPC became ready")
            time.sleep(0.1)
        assert control.exists(), "candidate daemon control IPC did not become ready"

        blocked_start = control_request(control, "snapshot.mount_helper.reconcile_inventory")
        assert blocked_start["ok"]
        blocked = wait_for_operation(control, blocked_start["result"]["operationId"])
        assert blocked["state"] == "failed"
        assert blocked["error"]["code"] == "snapshot_mount_helper.reconcile_required"
        assert blocked["error"]["details"]["unknown"] == 3

        delete_fixture_rows(journal, [unknown_id, malformed_id, future_state_id])
        ready = request(ipc, "status")
        assert ready["ok"]
        assert ready["result"]["active_mounts"] == 1
        assert ready["result"]["inventory_drift_mounts"] == 1
        reconcile_start = control_request(control, "snapshot.mount_helper.reconcile_inventory")
        assert reconcile_start["ok"]
        reconciled = wait_for_operation(control, reconcile_start["result"]["operationId"])
        assert reconciled["state"] == "succeeded", reconciled
        assert reconciled["result"]["reconciled"] == 1
        assert reconciled["result"]["remaining"] == 0
        assert reconciled["result"]["inventoryDrift"] == 0
        final = request(ipc, "status")
        assert final["ok"] and final["result"]["active_mounts"] == 0
        cli_drift_id = str(uuid.uuid4())
        insert_inventory_drift_row(journal, os.getuid(), volume_uuid, device, cli_drift_id)
        cli_environment = {
            **os.environ,
            "TELEVYBACKUP_CONFIG_DIR": str(config),
            "TELEVYBACKUP_DATA_DIR": str(data),
            "TELEVYBACKUP_DISABLE_KEYCHAIN": "1",
            "TELEVYBACKUP_SNAPSHOT_SOCKET": str(access_socket),
            "TELEVYBACKUP_SNAPSHOT_JOURNAL": str(access_journal),
            "TELEVYBACKUP_SNAPSHOT_MOUNT_SOCKET": str(ipc),
            "TELEVYBACKUP_SNAPSHOT_MOUNT_JOURNAL": str(journal),
        }
        cli_result = run(
            str(cli),
            "--json",
            "--config-dir",
            str(config),
            "--data-dir",
            str(data),
            "snapshot-mount-helper",
            "reconcile-inventory",
            check=False,
            env=cli_environment,
        )
        assert cli_result.returncode == 0, cli_result.stderr.decode(errors="replace")
        cli_payload = json.loads(cli_result.stdout.decode())
        assert cli_payload["reconciled"] is True
        assert cli_payload["records"] == 1
        assert cli_payload["remaining"] == 0
        assert cli_payload["inventoryDrift"] == 0
        cli_final = request(ipc, "status")
        assert cli_final["ok"] and cli_final["result"]["active_mounts"] == 0
        print(
            "PASS: helper classification, daemon reconcile gating, CLI inventory reconciliation, and unknown blocker preservation verified",
            flush=True,
        )
    finally:
        if daemon_process is not None:
            daemon_process.terminate()
            try:
                daemon_process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                daemon_process.kill()
                daemon_process.wait(timeout=10)
        if access_process is not None:
            access_process.terminate()
            try:
                access_process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                access_process.kill()
                access_process.wait(timeout=10)
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
