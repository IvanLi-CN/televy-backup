#!/usr/bin/env python3
"""Diagnose published helper upgrade behavior on a disposable Actions runner only."""

import hashlib
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import uuid


REPOSITORY = "IvanLi-CN/televy-backup"
OLD_TAG = "v0.9.9-rc.114"
NEW_TAG = "v0.9.9-rc.120"
NEW_SOURCE = "34639a0f50b998caa7d57d246be30cd479cff205"
LABEL = "com.ivan.televybackup.snapshot-mount-helper"
SERVICE = "system/" + LABEL
BINARY = Path("/Library/PrivilegedHelperTools") / LABEL
PLIST = Path("/Library/LaunchDaemons") / (LABEL + ".plist")
MOUNT_SOCKET = Path("/private/var/run/televybackup/snapshot-mount-helper.sock")
JOURNAL = Path("/var/db/televybackup/snapshot-mount-helper")
ACCESS_RELATIVE = Path("Contents/Library/LoginItems/TelevyBackup Snapshot Access.app")
EXECUTABLE = Path("Contents/MacOS/televybackup-snapshot-access")
PROTOCOL_ERROR = "unsupported mount helper response version"


def run(*args, check=True):
    return subprocess.run(args, capture_output=True, text=True, check=check, timeout=180)


def report(phase, **facts):
    print(json.dumps({"phase": phase, **facts}, sort_keys=True), flush=True)


def request(path, version):
    request_id = str(uuid.uuid4())
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
        stream.settimeout(5)
        stream.connect(str(path))
        stream.sendall(json.dumps({"version": version, "request_id": request_id,
                                   "method": "status"}).encode() + b"\n")
        reply = json.loads(stream.makefile("rb").readline())
    assert reply["request_id"] == request_id
    return reply


def wait_for(probe, predicate):
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        try:
            result = probe()
            if predicate(result):
                return result
        except (OSError, ValueError):
            pass
        time.sleep(0.2)
    raise AssertionError("synthetic helper did not reach the expected state")


def download_release(tag, root):
    destination = root / tag
    destination.mkdir()
    dmg_name = "TelevyBackup-" + tag.removeprefix("v") + ".dmg"
    run("gh", "release", "download", tag, "--repo", REPOSITORY,
        "--pattern", dmg_name, "--pattern", "BUILD-MANIFEST.json",
        "--pattern", "SHA256SUMS", "--dir", str(destination))
    manifest = json.loads((destination / "BUILD-MANIFEST.json").read_text())
    assert manifest["release_version"] == tag.removeprefix("v")
    checksums = {}
    for line in (destination / "SHA256SUMS").read_text().splitlines():
        digest, name = line.split(maxsplit=1)
        checksums[name.lstrip("*")] = digest
    digest = hashlib.sha256()
    with (destination / dmg_name).open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    assert digest.hexdigest() == checksums[dmg_name], "public DMG checksum mismatch"
    mount = destination / "mount"
    mount.mkdir()
    run("hdiutil", "attach", "-readonly", "-nobrowse", "-mountpoint", str(mount),
        str(destination / dmg_name))
    return mount, manifest


def replace_file(source, destination):
    staged = destination.with_name(destination.name + ".upgrade")
    shutil.copy2(source, staged)
    os.replace(staged, destination)


def stop(process):
    if process is not None and process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)


def main():
    assert os.environ.get("GITHUB_ACTIONS") == "true", "GitHub Actions required"
    assert os.environ.get("RUNNER_ENVIRONMENT") == "github-hosted", "disposable hosted runner required"
    assert sys.platform == "darwin" and os.geteuid() != 0
    assert os.environ.get("GITHUB_REPOSITORY") == REPOSITORY
    assert not any(path.exists() for path in (BINARY, PLIST, MOUNT_SOCKET, JOURNAL)), \
        "existing helper state: refusing to replace it"
    assert run("sudo", "-n", "/bin/launchctl", "print", SERVICE, check=False).returncode != 0
    run("sudo", "-n", "true")
    process = None
    mounts = []
    installed = False
    reproduction = None
    with tempfile.TemporaryDirectory(prefix="televy-upgrade-ci-", dir="/private/tmp") as temp:
        root = Path(temp)
        os.chmod(root, 0o755)
        try:
            old_mount, old_manifest = download_release(OLD_TAG, root)
            mounts.append(old_mount)
            new_mount, new_manifest = download_release(NEW_TAG, root)
            mounts.append(new_mount)
            assert new_manifest["source_commit"] == NEW_SOURCE
            old_component = old_manifest["components"]["snapshot_access"]
            new_component = new_manifest["components"]["snapshot_access"]
            old_protocol = old_manifest["components"]["snapshot_mount_helper"]["protocol_version"]
            new_protocol = new_component["mount_helper_protocol_version"]
            assert old_protocol == 1 and new_protocol == 4
            report("published-identities", old_tag=OLD_TAG, new_tag=NEW_TAG,
                   old_mount_protocol=old_protocol, new_mount_protocol=new_protocol,
                   old_access_version=old_component["component_version"],
                   new_access_version=new_component["component_version"],
                   access_binary_changed=old_component["sha256"] != new_component["sha256"])
            app = root / "TelevyBackup.app"
            shutil.copytree(old_mount / "TelevyBackup.app", app, symlinks=True)
            config, data = root / "config", root / "data"
            config.mkdir(mode=0o700)
            data.mkdir(mode=0o700)
            (config / "config.toml").write_text("targets = []\n")
            access_socket = data / "access.sock"
            env = os.environ.copy()
            env.update({"TELEVYBACKUP_CONFIG_DIR": str(config),
                        "TELEVYBACKUP_DATA_DIR": str(data),
                        "TELEVYBACKUP_SNAPSHOT_SOCKET": str(access_socket),
                        "TELEVYBACKUP_SNAPSHOT_JOURNAL": str(data / "journal.sqlite")})
            env.pop("TELEVYBACKUP_SNAPSHOT_MOUNT_SOCKET", None)
            cli = app / "Contents/MacOS/televybackup-cli"
            access = app / ACCESS_RELATIVE / EXECUTABLE
            installed = True
            run("sudo", "-n", str(cli), "--json", "--config-dir", str(config),
                "--data-dir", str(data), "snapshot-mount-helper", "install")
            wait_for(lambda: request(MOUNT_SOCKET, old_protocol), lambda reply: reply.get("ok") is True)
            with (root / "synthetic-access.log").open("w") as log:
                process = subprocess.Popen([str(access)], env=env, stdout=log, stderr=log)
                before = wait_for(lambda: request(access_socket, old_component["protocol_version"]),
                                  lambda reply: reply.get("result", {}).get("mount_helper_reachable") is True)
                report("old-pair", access_reachable=before["ok"], mount_reachable=True)
                # Replace on-disk binaries at the same canonical paths while preserving
                # the running Access process, as can happen during an app upgrade.
                new_app = new_mount / "TelevyBackup.app"
                replace_file(new_app / "Contents/MacOS/televybackup-cli", cli)
                replace_file(new_app / "Contents/MacOS/televybackup-snapshot-mount-helper",
                             app / "Contents/MacOS/televybackup-snapshot-mount-helper")
                replace_file(new_app / ACCESS_RELATIVE / EXECUTABLE, access)
                assert process.poll() is None, "old Access exited before the upgrade probe"
                installation = json.loads(run("sudo", "-n", str(cli), "--json", "--config-dir",
                                              str(config), "--data-dir", str(data),
                                              "snapshot-mount-helper", "install").stdout)
                assert installation["helperReady"] is True
                assert installation["helperVersion"] == "0.5.0"
                direct = request(MOUNT_SOCKET, new_protocol)
                assert direct["version"] == new_protocol and direct["ok"] is True
                after = request(access_socket, new_component["protocol_version"])
                assert after["ok"] is True
                state = after["result"]
                reproduction = state["mount_helper_reachable"] is False and \
                    PROTOCOL_ERROR in (state.get("mount_helper_error") or "")
                report("after-install", cli_ready=installation["helperReady"],
                       direct_mount_protocol=direct["version"],
                       access_component_version=state["access_app_version"],
                       access_path_matches_current_bundle=state.get("access_app_path") == str(app / ACCESS_RELATIVE),
                       old_access_process_alive=process.poll() is None,
                       mount_reachable=state["mount_helper_reachable"],
                       exact_protocol_error=reproduction)
                stop(process)
                process = subprocess.Popen([str(access)], env=env, stdout=log, stderr=log)
                restarted = wait_for(lambda: request(access_socket, new_component["protocol_version"]),
                                    lambda reply: reply.get("result", {}).get("mount_helper_reachable") is True)
                report("restart-only", mount_reachable=restarted["result"]["mount_helper_reachable"],
                       mount_error_absent=restarted["result"].get("mount_helper_error") is None)
        finally:
            stop(process)
            if installed:
                run("sudo", "-n", "/bin/launchctl", "bootout", SERVICE, check=False)
                # Every target was proved absent before this disposable fixture created it.
                run("sudo", "-n", "rm", "-f", str(BINARY), str(PLIST), str(MOUNT_SOCKET))
                run("sudo", "-n", "rm", "-rf", str(JOURNAL))
            for mount in reversed(mounts):
                run("hdiutil", "detach", str(mount))
    assert reproduction is not None
    assert not reproduction, \
        "REPRODUCED: installer is ready but a surviving old Access process rejects mount protocol 4"


if __name__ == "__main__":
    main()
