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
JOURNAL_SENTINEL_KEY = "ci-fixture"
JOURNAL_SENTINEL_VALUE = "journal-preserved-v1"


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


def asset_backup_glob(path, suffix):
    """Mirror Rust's Path::with_extension backup naming for dotted asset names."""
    return path.parent.glob(f"{path.stem}.{suffix}-*")


def root_path_exists(path, directory=False):
    kind = "directory" if directory else "path"
    script = (
        "import os, stat, sys; "
        "path = sys.argv[1]; "
        "metadata = os.stat(path); "
        "ok = stat.S_ISDIR(metadata.st_mode) if sys.argv[2] == 'directory' else True; "
        "print('1' if ok else '0')"
    )
    result = run(
        "sudo",
        "-n",
        "/usr/bin/python3",
        "-c",
        script,
        str(path),
        kind,
        check=False,
    )
    return result.returncode == 0 and result.stdout.strip() == "1"


def root_metadata(path):
    script = "import os, sys; stat = os.stat(sys.argv[1]); print(stat.st_uid, stat.st_gid, oct(stat.st_mode & 0o7777))"
    output = run("sudo", "-n", "/usr/bin/python3", "-c", script, str(path)).stdout.strip()
    uid, gid, mode = output.split()
    return int(uid), int(gid), int(mode, 8)


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


def seed_journal_sentinel():
    """Create fixture-only journal content that install and uninstall must preserve."""
    script = r'''
import pathlib, sqlite3, sys
journal_dir = pathlib.Path(sys.argv[1])
journal_dir.mkdir(mode=0o700, parents=True, exist_ok=True)
db = journal_dir / "journal.sqlite"
with sqlite3.connect(db) as connection:
    connection.execute(
        "CREATE TABLE IF NOT EXISTS ci_fixture_sentinel (key TEXT PRIMARY KEY, value TEXT NOT NULL)"
    )
    connection.execute(
        "INSERT OR REPLACE INTO ci_fixture_sentinel (key, value) VALUES (?, ?)",
        (sys.argv[2], sys.argv[3]),
    )
    connection.commit()
db.chmod(0o600)
'''
    run(
        "sudo", "-n", "/usr/bin/python3", "-c", script,
        str(JOURNAL), JOURNAL_SENTINEL_KEY, JOURNAL_SENTINEL_VALUE,
    )


def read_journal_sentinel():
    script = r'''
import pathlib, sqlite3, sys
db = pathlib.Path(sys.argv[1]) / "journal.sqlite"
with sqlite3.connect(db) as connection:
    row = connection.execute(
        "SELECT value FROM ci_fixture_sentinel WHERE key = ?", (sys.argv[2],)
    ).fetchone()
if row is None:
    raise SystemExit("journal sentinel is missing")
print(row[0])
'''
    return run(
        "sudo", "-n", "/usr/bin/python3", "-c", script,
        str(JOURNAL), JOURNAL_SENTINEL_KEY,
    ).stdout.strip()


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
    parser.add_argument(
        "--legacy-build-only",
        action="store_true",
        help="make the intermediate helper report one row with build identity but no classification",
    )
    parser.add_argument(
        "--stale-partial-classification",
        action="store_true",
        help="make the previous component report a stale build with inventory classification missing",
    )
    parser.add_argument(
        "--bootstrap-failure-rollback",
        action="store_true",
        help="replace the candidate helper with a failing executable and verify the previous helper is restored",
    )
    parser.add_argument(
        "--orphaned-backup-recovery",
        action="store_true",
        help="seed root-owned interrupted-install backups and verify the installer recovers them",
    )
    parser.add_argument(
        "--uninstall",
        action="store_true",
        help="install the candidate, then verify the privileged uninstall transaction removes its assets",
    )
    args = parser.parse_args()
    legacy_fixture = args.legacy_blocking or args.legacy_build_only or args.stale_partial_classification
    assert sum((args.legacy_blocking, args.legacy_build_only, args.stale_partial_classification, args.bootstrap_failure_rollback, args.orphaned_backup_recovery, args.uninstall)) <= 1, "fixture modes are mutually exclusive"
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
    uninstall_sentinel = None
    candidate_helper = app / "Contents/MacOS/televybackup-snapshot-mount-helper"
    candidate_helper_original = candidate_helper.read_bytes()
    candidate_helper_modified = False
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
    old_server.write_text('''import hashlib, json, os, pathlib, socket
marker_path = "__OLD_HELPER_MARKER__"
binary_path = "__OLD_HELPER_BINARY__"
plist_path = "__OLD_HELPER_PLIST__"
pathlib.Path(marker_path).write_text(json.dumps({
    "binary": hashlib.sha256(pathlib.Path(binary_path).read_bytes()).hexdigest() if pathlib.Path(binary_path).exists() else None,
    "plist": hashlib.sha256(pathlib.Path(plist_path).read_bytes()).hexdigest() if pathlib.Path(plist_path).exists() else None,
}))
path = "/private/var/run/televybackup/snapshot-mount-helper.sock"
os.makedirs(os.path.dirname(path), mode=0o711, exist_ok=True)
try:
    os.unlink(path)
except FileNotFoundError:
    pass
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
    old_helper_marker = fixture / "old-helper-started.json"
    old_server.write_text(old_server.read_text()
        .replace('"__OLD_HELPER_MARKER__"', repr(str(old_helper_marker)))
        .replace('"__OLD_HELPER_BINARY__"', repr(str(BINARY)))
        .replace('"__OLD_HELPER_PLIST__"', repr(str(PLIST))))
    if args.expect_build_commit:
        old_server.write_text(old_server.read_text().replace('"version":1', '"version":4')
            .replace('"helper_version":"0.9.8"', '"helper_version":"0.5.0","build_commit":"' + '0' * 40 + '"'))
    elif args.bootstrap_failure_rollback:
        old_server.write_text(old_server.read_text().replace('"version":1', '"version":4')
            .replace('"helper_version":"0.9.8"', '"helper_version":"0.5.0"'))
    elif args.orphaned_backup_recovery:
        old_server.write_text(old_server.read_text().replace('"version":1', '"version":4')
            .replace('"helper_version":"0.9.8"', '"helper_version":"0.5.0"'))
    elif args.stale_partial_classification:
        old_server.write_text(old_server.read_text().replace('"version":1', '"version":4')
            .replace('"helper_version":"0.9.8"',
                     '"helper_version":"0.5.0","component_version":"0.5.5",'
                     '"build_commit":"' + '0' * 40 + '","pending_cleanup":0,'
                     '"live_mounts":0,"recoverable_stale_mounts":0,"unknown_blocking_mounts":1'))
    elif legacy_fixture:
        replacement = '"helper_version":"0.5.0"'
        if args.legacy_build_only:
            replacement += ',"build_commit":"' + '0' * 40 + '"'
        old_server.write_text(old_server.read_text().replace('"version":1', '"version":4')
            .replace('"helper_version":"0.9.8"', replacement))
    if legacy_fixture:
        old_server.write_text(old_server.read_text().replace('"active_mounts":0', '"active_mounts":1'))
    seed_plist = fixture / "old.plist"
    seed_plist.write_bytes(plistlib.dumps({
        "Label": LABEL,
        "ProgramArguments": ["/usr/bin/python3", str(old_server)],
        "RunAtLoad": True,
        "KeepAlive": True,
    }))
    root_plist = root / "old.plist"
    old_binary_hash = None
    if args.bootstrap_failure_rollback or args.orphaned_backup_recovery:
        old_binary_source = fixture / "old-helper"
        old_binary_source.write_text("#!/usr/bin/python3\n" + old_server.read_text())
        old_binary_source.chmod(0o755)
        old_binary_hash = hashlib.sha256(old_binary_source.read_bytes()).hexdigest()
        binary_destination = BINARY if args.bootstrap_failure_rollback else BINARY.with_name(BINARY.stem + ".backup-fixture")
        run("sudo", "-n", "cp", str(old_binary_source), str(binary_destination))
        run("sudo", "-n", "chown", "root:wheel", str(binary_destination))
        run("sudo", "-n", "chmod", "755", str(binary_destination))
        canonical_plist = fixture / "canonical-old.plist"
        canonical_plist.write_bytes(plistlib.dumps({
            "Label": LABEL,
            "ProgramArguments": [str(BINARY)],
            "RunAtLoad": True,
            "KeepAlive": True,
        }))
        old_plist_hash = hashlib.sha256(canonical_plist.read_bytes()).hexdigest()
        plist_destination = PLIST if args.bootstrap_failure_rollback else PLIST.with_name(PLIST.stem + ".backup-fixture")
        run("sudo", "-n", "cp", str(canonical_plist), str(plist_destination))
        run("sudo", "-n", "chown", "root:wheel", str(plist_destination))
        run("sudo", "-n", "chmod", "644", str(plist_destination))
        if args.orphaned_backup_recovery:
            current_plist = fixture / "interrupted-current.plist"
            current_plist.write_bytes(plistlib.dumps({
                "Label": LABEL,
                "ProgramArguments": ["/usr/bin/false"],
                "RunAtLoad": True,
                "KeepAlive": True,
            }))
            run("sudo", "-n", "cp", str(app / "Contents/MacOS/televybackup-snapshot-mount-helper"), str(BINARY))
            run("sudo", "-n", "chown", "root:wheel", str(BINARY))
            run("sudo", "-n", "chmod", "755", str(BINARY))
            run("sudo", "-n", "cp", str(current_plist), str(PLIST))
            run("sudo", "-n", "chown", "root:wheel", str(PLIST))
            run("sudo", "-n", "chmod", "644", str(PLIST))
    else:
        run("sudo", "-n", "cp", str(seed_plist), str(root_plist))
        run("sudo", "-n", "chown", "root:wheel", str(root_plist))
        run("sudo", "-n", "chmod", "644", str(root_plist))
    if args.bootstrap_failure_rollback:
        seed_journal_sentinel()
    try:
        if not args.orphaned_backup_recovery:
            run(
                "sudo",
                "-n",
                "/bin/launchctl",
                "bootstrap",
                "system",
                str(PLIST if args.bootstrap_failure_rollback else root_plist),
            )
            wait_for(lambda: request(MOUNT_SOCKET, expected_protocol), lambda reply: reply["version"] == (4 if (args.expect_build_commit or legacy_fixture or args.bootstrap_failure_rollback) else 1))
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
        elif args.bootstrap_failure_rollback:
            assert before["result"]["mount_helper_reachable"] is True
            print("SETUP: previous helper is reachable before the failing candidate install", flush=True)
        elif args.orphaned_backup_recovery:
            assert before["result"]["mount_helper_reachable"] is False
            print("SETUP: interrupted helper backups are present while the service is absent", flush=True)
        elif args.uninstall:
            assert before["result"]["mount_helper_reachable"] is False
            uninstall_sentinel = JOURNAL / "uninstall-sentinel"
            seed_journal_sentinel()
            run("sudo", "-n", "mkdir", "-p", str(JOURNAL))
            run("sudo", "-n", "touch", str(uninstall_sentinel))
            run("sudo", "-n", "chown", "root:wheel", str(uninstall_sentinel))
            run("sudo", "-n", "chmod", "600", str(uninstall_sentinel))
            print("SETUP: legacy helper is reachable only through the compatibility probe", flush=True)
        elif legacy_fixture:
            legacy = request(MOUNT_SOCKET, expected_protocol)["result"]
            assert legacy["active_mounts"] == 1
            if args.stale_partial_classification:
                assert legacy["component_version"] == "0.5.5"
                assert legacy["build_commit"] == "0" * 40
                assert legacy["pending_cleanup"] == 0
                assert legacy["live_mounts"] == 0
                assert legacy["recoverable_stale_mounts"] == 0
                assert legacy["unknown_blocking_mounts"] == 1
                assert "inventory_drift_mounts" not in legacy
                print("REPRODUCED: stale current helper omits inventory-drift classification", flush=True)
            elif args.legacy_build_only:
                assert "pending_cleanup" not in legacy and "component_version" not in legacy
                assert legacy["build_commit"] == "0" * 40
                print("REPRODUCED: intermediate helper reports build identity without mount classification", flush=True)
            else:
                assert "pending_cleanup" not in legacy and "component_version" not in legacy
                assert "build_commit" not in legacy
                print("REPRODUCED: legacy helper reports an unclassified blocking mount row", flush=True)
        else:
            assert before["result"]["mount_helper_reachable"] is False
            assert "unsupported mount helper response version" in before["result"]["mount_helper_error"]
            print("REPRODUCED: published Access reports unsupported mount helper response version for the legacy service", flush=True)
        if legacy_fixture:
            stale_lease = seed_legacy_stale_journal(os.getuid())
            print(
                "SEEDED: one age-gated legacy reservation row before candidate helper startup",
                flush=True,
            )
        # Same privileged CLI entrypoint as the GUI's password-authorized command.
        if args.bootstrap_failure_rollback:
            assert args.require_readiness, "bootstrap failure fixture requires the candidate readiness check"
            candidate_helper_modified = True
            candidate_helper.write_text("#!/bin/sh\nexit 78\n")
            candidate_helper.chmod(0o755)
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
        installed = run(
            "sudo",
            "-n",
            str(cli),
            "--json",
            "--config-dir",
            str(config),
            "--data-dir",
            str(data),
            "snapshot-mount-helper",
            "install",
            check=not args.bootstrap_failure_rollback,
        )
        if args.bootstrap_failure_rollback:
            assert installed.returncode != 0, "failing candidate unexpectedly installed"
            error = json.loads(installed.stderr)
            assert error["code"] == "snapshot_mount_helper.readiness_failed"
            restored = wait_for(
                lambda: request(MOUNT_SOCKET, expected_protocol),
                lambda reply: reply.get("ok") is True and reply.get("result", {}).get("active_mounts") == 0,
            )
            assert restored["result"]["helper_version"] == "0.5.0"
            assert hashlib.sha256(BINARY.read_bytes()).hexdigest() == old_binary_hash
            assert hashlib.sha256(PLIST.read_bytes()).hexdigest() == old_plist_hash
            assert read_journal_sentinel() == JOURNAL_SENTINEL_VALUE
            assert not any(asset_backup_glob(BINARY, "backup"))
            assert not any(asset_backup_glob(PLIST, "backup"))
            print("PASS: failed candidate bootstrap restores the previous helper and clears quarantine backups", flush=True)
        else:
            assert installed.returncode == 0, installed.stderr
        installation = json.loads(installed.stdout) if not args.bootstrap_failure_rollback else {}
        if not args.bootstrap_failure_rollback:
            assert installation["installed"] is True
        if args.require_readiness:
            if not args.bootstrap_failure_rollback:
                assert installation["helperReady"] is True
                assert installation["helperVersion"] == "0.5.6"
        if args.orphaned_backup_recovery:
            binary_backups = sorted(asset_backup_glob(BINARY, "backup"))
            plist_backups = sorted(asset_backup_glob(PLIST, "backup"))
            assert not binary_backups and not plist_backups, (
                "interrupted helper backups remain after recovery: "
                f"binary={binary_backups!r}, plist={plist_backups!r}"
            )
            marker = wait_for(
                lambda: json.loads(old_helper_marker.read_text()),
                lambda value: value.get("binary") is not None and value.get("plist") is not None,
            )
            assert marker["binary"] == old_binary_hash
            assert marker["plist"] == old_plist_hash
            print("PASS: interrupted helper backups were recovered before the candidate install", flush=True)
        if legacy_fixture:
            assert installation["helperVersion"] == "0.5.6"
            print("PASS: privileged CLI reports the replacement helper ready", flush=True)
        if journal_locker is not None:
            assert journal_locker.wait(timeout=10) == 0
            assert time.monotonic() - install_started >= 1, "startup delay did not cover the old subsecond retry window"
            print("PASS: real privileged installer waits for delayed journal initialization without a manual refresh", flush=True)
        direct = wait_for(lambda: request(MOUNT_SOCKET, expected_protocol), lambda reply: reply["version"] == expected_protocol and reply.get("ok") is True)
        assert direct["result"]["helper_version"] == "0.5.0"
        if legacy_fixture:
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
            assert direct["result"]["component_version"] == "0.5.6"
            candidate = app / "Contents/MacOS/televybackup-snapshot-mount-helper"
            digest = hashlib.sha256(candidate.read_bytes()).hexdigest()
            installed_digest = hashlib.sha256(BINARY.read_bytes()).hexdigest()
            assert direct["result"]["build_commit"] == args.expect_build_commit
            assert installation["buildCommit"] == args.expect_build_commit
            assert installation["binarySha256"] == digest == installed_digest
            print("PASS: installed file hash and running IPC build match the candidate", flush=True)
        if args.uninstall:
            removed = run(
                "sudo", "-n", str(cli), "--json", "--config-dir", str(config), "--data-dir", str(data),
                "snapshot-mount-helper", "uninstall",
            )
            assert removed.returncode == 0, removed.stderr
            removal = json.loads(removed.stdout)
            assert removal["uninstalled"] is True
            assert not BINARY.exists() and not PLIST.exists()
            assert uninstall_sentinel is not None and root_path_exists(uninstall_sentinel)
            assert root_path_exists(JOURNAL, directory=True)
            assert root_metadata(JOURNAL) == (0, 0, 0o700)
            assert root_metadata(JOURNAL / "journal.sqlite") == (0, 0, 0o600)
            assert root_metadata(uninstall_sentinel) == (0, 0, 0o600)
            assert read_journal_sentinel() == JOURNAL_SENTINEL_VALUE
            after = wait_for(
                lambda: request(access_socket, metadata["protocolVersion"]),
                lambda reply: reply.get("result", {}).get("mount_helper_reachable") is False,
            )
            assert after["result"]["mount_helper_error"] is not None
            print("PASS: privileged uninstall removes helper assets and leaves Snapshot Access unreachable", flush=True)
        else:
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
        if candidate_helper_modified:
            candidate_helper.write_bytes(candidate_helper_original)
            candidate_helper.chmod(0o755)
        # These targets were proved absent before this test created them.
        run(
            "sudo",
            "-n",
            "rm",
            "-f",
            str(BINARY),
            str(PLIST),
            str(BINARY.with_name(BINARY.stem + ".backup-fixture")),
            str(PLIST.with_name(PLIST.stem + ".backup-fixture")),
            str(MOUNT_SOCKET),
            check=False,
        )
        for path in [
            *asset_backup_glob(BINARY, "backup"),
            *asset_backup_glob(BINARY, "uninstall-backup"),
            *asset_backup_glob(PLIST, "backup"),
            *asset_backup_glob(PLIST, "uninstall-backup"),
        ]:
            run("sudo", "-n", "rm", "-f", str(path), check=False)
        run("sudo", "-n", "rm", "-rf", str(JOURNAL), str(root), check=False)
        assert not any(asset_backup_glob(BINARY, "backup"))
        assert not any(asset_backup_glob(BINARY, "uninstall-backup"))
        assert not any(asset_backup_glob(PLIST, "backup"))
        assert not any(asset_backup_glob(PLIST, "uninstall-backup"))
        assert not any(os.path.lexists(path) for path in (BINARY, PLIST, MOUNT_SOCKET, JOURNAL)), "fixture cleanup incomplete"
        shutil.rmtree(fixture)


if __name__ == "__main__":
    main()
