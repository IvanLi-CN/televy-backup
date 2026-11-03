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


def bootout_service(service):
    result = run("sudo", "-n", "launchctl", "bootout", service, check=False)
    if result.returncode not in (0, 3, 5):
        result.check_returncode()


def restart_root_service(service, plist, socket_path):
    bootout_service(service)
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        if run("sudo", "-n", "launchctl", "print", service, check=False).returncode != 0:
            break
        time.sleep(0.1)
    else:
        raise AssertionError("root helper did not stop before journal restart")
    if socket_path.exists():
        run("sudo", "-n", "rm", "-f", str(socket_path))
    deadline = time.monotonic() + 15
    while socket_path.exists() and time.monotonic() < deadline:
        time.sleep(0.1)
    assert not socket_path.exists(), "root helper socket remained after bootout"
    run("sudo", "-n", "launchctl", "bootstrap", "system", str(plist))
    deadline = time.monotonic() + 15
    while not socket_path.exists() and time.monotonic() < deadline:
        time.sleep(0.1)
    assert socket_path.exists(), "root helper socket did not return after restart"
    assert request(socket_path, "status")["ok"], "root helper failed after journal restart"


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


def write_future_state_row(journal, uid, volume_uuid, device, lease_id):
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
        ) VALUES (?, ?, ?, ?, '', '', '', '[]', '[]', 1, 'future_state', datetime('now'), datetime('now'))
    """,
    (lease_id, int(uid), volume_uuid, device),
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


def write_malformed_released_row(journal, uid, volume_uuid, device, lease_id):
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
        ) VALUES (?, ?, ?, ?, '', '', '', '[]', 'not-json', 0, 'released', datetime('now'), datetime('now'))
    """,
    (lease_id, int(uid), volume_uuid, device),
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


def ensure_uid_mount_root(uid):
    """Create only the root-owned per-user parent required by the legacy repair fixture."""
    managed_root = Path("/private/var/tmp/televybackup-snapshot-mounts")
    uid_root = managed_root / str(uid)
    if uid_root.is_symlink() or (uid_root.exists() and not uid_root.is_dir()):
        raise AssertionError("managed UID mount root is not a directory")
    created = not uid_root.exists()
    if created:
        run("sudo", "-n", "mkdir", "-p", str(uid_root))
        run("sudo", "-n", "chown", "root:wheel", str(uid_root))
        run("sudo", "-n", "chmod", "711", str(uid_root))
    return uid_root, created


def insert_legacy_repair_row(
    journal,
    uid,
    volume_uuid,
    device,
    lease_id,
    snapshot_uuid,
    reservation_snapshot_uuid,
):
    """Seed one stale pre-device-identity row whose managed root is already gone."""
    mount_root = Path("/private/var/tmp/televybackup-snapshot-mounts") / str(uid) / lease_id
    snapshot_manifest = json.dumps(
        [{"device_identifier": device, "uuid": snapshot_uuid}],
        separators=(",", ":"),
    )
    reservation_manifest = json.dumps(
        [{"device_identifier": device, "uuid": reservation_snapshot_uuid}],
        separators=(",", ":"),
    )
    script = '''
import sqlite3, sys
journal, uid, volume_uuid, device, lease_id, mount_root, snapshot_manifest, reservation_manifest = sys.argv[1:]
connection = sqlite3.connect(journal)
connection.execute(
    """
        INSERT INTO mounts (
            lease_id, uid, volume_uuid, device_identifier, source_path,
            snapshot_name, mount_root, mount_device, snapshot_manifest,
            reservation_manifest, reservation_manifest_valid, state,
            created_at, last_renewed_at
        ) VALUES (?, ?, ?, ?, '', '', ?, 0, ?, ?, 1, 'cleanup_pending',
                  datetime('now', '-301 seconds'), datetime('now', '-301 seconds'))
    """,
    (lease_id, int(uid), volume_uuid, device, mount_root, snapshot_manifest, reservation_manifest),
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
        str(mount_root),
        snapshot_manifest,
        reservation_manifest,
    )


def insert_legacy_empty_reservation_rows(journal, uid, volume_uuid, device, count, state="reserved"):
    """Seed stale reservations that predate the persisted baseline fields."""
    script = '''
import json, sqlite3, sys, uuid
journal, uid, volume_uuid, device, count, state = sys.argv[1:]
connection = sqlite3.connect(journal)
for _ in range(int(count)):
    lease_id = str(uuid.uuid4())
    root = f"/private/var/tmp/televybackup-snapshot-mounts/{uid}/{lease_id}" if state == "released" else ""
    manifest = json.dumps([{"device_identifier": device, "uuid": str(uuid.uuid4())}]) if state == "released" else "[]"
    connection.execute(
        """
            INSERT INTO mounts (
                lease_id, uid, volume_uuid, device_identifier, source_path,
                snapshot_name, mount_root, mount_device, snapshot_manifest,
                reservation_manifest, reservation_manifest_valid, state,
                created_at, last_renewed_at
            ) VALUES (?, ?, ?, ?, '', '', ?, 0, ?, '[]', 0, ?,
                      datetime('now', '-301 seconds'), datetime('now', '-301 seconds'))
        """,
        (lease_id, int(uid), volume_uuid, device, root, manifest, state),
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
        str(count),
        state,
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
    root = fixture / "root"
    image = fixture / "fixture.dmg"
    device = None
    attached = loaded = False
    uid_root_created = False
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
        source.mkdir(parents=True)
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
        write_future_state_row(journal, os.getuid(), volume_uuid, device, future_state_id)
        restart_root_service(service, root_plist, ipc)
        restarted = request(ipc, "status")
        assert restarted["ok"]
        assert restarted["result"]["active_mounts"] == 1
        assert restarted["result"]["unknown_blocking_mounts"] == 1
        delete_fixture_rows(journal, [future_state_id])
        write_malformed_released_row(
            journal, os.getuid(), volume_uuid, device, malformed_released_id
        )
        malformed_reserve = request(
            ipc,
            "reserve",
            uid=os.getuid(),
            lease_id=str(uuid.uuid4()),
            volume_uuid=volume_uuid,
            device_identifier=device,
            source_path=str(source),
        )
        assert not malformed_reserve["ok"]
        assert malformed_reserve["code"] == "invalid_request", malformed_reserve
        assert "tombstone inventory changed" in malformed_reserve["message"]
        delete_fixture_rows(journal, [malformed_released_id])
        write_future_state_row(journal, os.getuid(), volume_uuid, device, future_state_id)
        future_reserve = request(
            ipc,
            "reserve",
            uid=os.getuid(),
            lease_id=str(uuid.uuid4()),
            volume_uuid=volume_uuid,
            device_identifier=device,
            source_path=str(source),
        )
        assert not future_reserve["ok"]
        assert future_reserve["code"] == "invalid_request", future_reserve
        assert "active mount or pending snapshot cleanup" in future_reserve["message"]
        delete_fixture_rows(journal, [future_state_id])
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
        assert status["active_mounts"] == 6
        assert status["pending_cleanup"] == 1
        assert status["recoverable_stale_mounts"] == 1
        assert status["unknown_blocking_mounts"] == 3
        assert status["inventory_drift_mounts"] == 1
        assert [row["lease_id"] for row in status["pending_cleanup_leases"]] == [pending_id]
        assert [row["lease_id"] for row in status["recovery_leases"]] == [stale_id]
        assert [row["lease_id"] for row in status["inventory_drift_leases"]] == [released_drift_id]
        delete_fixture_rows(journal, [malformed_released_id, released_drift_id])
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
        assert status["active_mounts"] == 3
        assert status["pending_cleanup"] == 0
        assert status["recoverable_stale_mounts"] == 0
        assert status["unknown_blocking_mounts"] == 2
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
        assert blocked["error"]["details"]["unknown"] == 2

        delete_fixture_rows(journal, [unknown_id, malformed_id])
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
        uid_root, uid_root_created = ensure_uid_mount_root(os.getuid())
        # Hosted macOS does not permit creating a local snapshot for a disposable
        # APFS image (`tmutil localsnapshot` exits 73), and `diskutil apfs` has no
        # snapshot-create verb. Use structurally valid but unobservable legacy and
        # reservation manifests that do not match the empty inventory. This proves
        # the production repair path remains fail-closed when snapshot identity
        # cannot be proven; it must retain cleanup_pending rather than release the
        # row. The successful journal-bound Release path is covered by deterministic
        # helper tests; this fixture must not claim deletion of a snapshot it could
        # not safely create or inventory.
        legacy_empty_count = 65
        insert_legacy_empty_reservation_rows(
            journal,
            os.getuid(),
            volume_uuid,
            device,
            legacy_empty_count,
        )
        empty_before = request(ipc, "status")
        assert empty_before["ok"]
        assert empty_before["result"]["active_mounts"] == legacy_empty_count
        assert empty_before["result"]["unknown_blocking_mounts"] == legacy_empty_count
        empty_repair_start = control_request(control, "snapshot.mount_helper.repair_legacy")
        assert empty_repair_start["ok"]
        empty_repaired = wait_for_operation(control, empty_repair_start["result"]["operationId"])
        assert empty_repaired["state"] == "succeeded", empty_repaired
        assert empty_repaired["result"]["audited"] == legacy_empty_count
        assert empty_repaired["result"]["repaired"] == legacy_empty_count
        assert empty_repaired["result"]["released"] == legacy_empty_count
        assert empty_repaired["result"]["migrated"] == legacy_empty_count
        assert empty_repaired["result"]["blocked"] == 0
        empty_after = request(ipc, "status")
        assert empty_after["ok"] and empty_after["result"]["active_mounts"] == 0

        # Old helpers persisted terminal rows without clearing managed roots or
        # manifests. Status audits those tombstones, so legacy repair must audit
        # and normalize them too, through the production daemon control path.
        insert_legacy_empty_reservation_rows(
            journal, os.getuid(), volume_uuid, device, 65, state="released"
        )
        released_before = request(ipc, "status")
        assert released_before["result"]["unknown_blocking_mounts"] == 65
        released_start = control_request(control, "snapshot.mount_helper.repair_legacy")
        assert released_start["ok"]
        released_result = wait_for_operation(control, released_start["result"]["operationId"])
        assert released_result["state"] == "succeeded", released_result
        for key in ("audited", "repaired", "released", "migrated"):
            assert released_result["result"][key] == 65, released_result
        assert released_result["result"]["blocked"] == 0
        assert request(ipc, "status")["result"]["active_mounts"] == 0

        legacy_daemon_id = str(uuid.uuid4())
        legacy_daemon_snapshot_uuid = str(uuid.uuid4())
        legacy_daemon_reservation_uuid = str(uuid.uuid4())
        insert_legacy_repair_row(
            journal,
            os.getuid(),
            volume_uuid,
            device,
            legacy_daemon_id,
            legacy_daemon_snapshot_uuid,
            legacy_daemon_reservation_uuid,
        )
        legacy_before = request(ipc, "status")
        assert legacy_before["ok"]
        assert legacy_before["result"]["active_mounts"] == 1
        assert legacy_before["result"]["unknown_blocking_mounts"] == 1
        repair_start = control_request(control, "snapshot.mount_helper.repair_legacy")
        assert repair_start["ok"]
        repaired = wait_for_operation(control, repair_start["result"]["operationId"])
        assert repaired["state"] == "failed", repaired
        assert repaired["error"]["code"] == "snapshot_mount_helper.repair_required"
        assert repaired["error"]["details"]["pending"] == 1
        daemon_repair_final = request(ipc, "status")
        assert daemon_repair_final["ok"]
        assert daemon_repair_final["result"]["active_mounts"] == 1
        assert daemon_repair_final["result"]["pending_cleanup"] == 1
        assert daemon_repair_final["result"]["unknown_blocking_mounts"] == 0

        legacy_cli_id = str(uuid.uuid4())
        legacy_cli_snapshot_uuid = str(uuid.uuid4())
        legacy_cli_reservation_uuid = str(uuid.uuid4())
        insert_legacy_repair_row(
            journal,
            os.getuid(),
            volume_uuid,
            device,
            legacy_cli_id,
            legacy_cli_snapshot_uuid,
            legacy_cli_reservation_uuid,
        )
        cli_repair = run(
            str(cli),
            "--json",
            "--config-dir",
            str(config),
            "--data-dir",
            str(data),
            "snapshot-mount-helper",
            "repair-legacy",
            check=False,
            env=cli_environment,
        )
        assert cli_repair.returncode != 0
        cli_repair_error = cli_repair.stderr.decode(errors="replace")
        assert "snapshot_mount_helper.repair_required" in cli_repair_error
        cli_repair_final = request(ipc, "status")
        assert cli_repair_final["ok"]
        assert cli_repair_final["result"]["active_mounts"] == 2
        assert cli_repair_final["result"]["pending_cleanup"] == 1
        assert cli_repair_final["result"]["unknown_blocking_mounts"] == 1
        print(
            "PASS: helper classification, daemon/CLI inventory reconciliation, legacy repair fencing, and unknown blocker preservation verified",
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
            bootout_service(service)
            stopped = run("sudo", "-n", "launchctl", "print", service, check=False)
            assert (
                stopped.returncode != 0 and b"Could not find service" in stopped.stderr
            ), "fixture shutdown unverified; preserving state"
            if ipc.exists():
                run("sudo", "-n", "rm", "-f", str(ipc))
        if uid_root_created:
            run("sudo", "-n", "rmdir", str(uid_root))
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
