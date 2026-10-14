#!/usr/bin/env python3
"""Diagnose published helper upgrade behavior on a disposable Actions runner only."""

import hashlib
import json
import os
from pathlib import Path
import plistlib
import re
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
ACCESS_LABEL = "com.ivan.televybackup.snapshot-access"
ACCESS_SERVICE = f"gui/{os.geteuid()}/{ACCESS_LABEL}"


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


def access_job_state():
    result = run("/bin/launchctl", "print", ACCESS_SERVICE)
    match = re.search(r"^\s*pid = (\d+)\s*$", result.stdout, re.MULTILINE)
    assert match is not None, "synthetic Access LaunchAgent has no running process"
    return int(match.group(1)), result.stdout


def refresh_probe(job_output, root, bundle_version):
    output_file = root / "synthetic-launchd-state.txt"
    output_file.write_text(job_output)
    harness = root / "RefreshProbe.swift"
    harness.write_text('''import Foundation
@main struct RefreshProbe {
    static func main() throws {
        let output = try String(contentsOfFile: CommandLine.arguments[1], encoding: .utf8)
        let result = SnapshotAccessLaunchdState.needsRefresh(
            output: output,
            expectedBundleIdentifier: "com.ivan.televybackup",
            expectedBundleVersion: CommandLine.arguments[2])
        print(result ? "true" : "false")
    }
}
''')
    executable = root / "refresh-probe"
    run("swiftc", "macos/TelevyBackupApp/SnapshotAccessLaunchdState.swift", str(harness),
        "-o", str(executable))
    return run(str(executable), str(output_file), bundle_version).stdout.strip() == "true"


def main():
    assert os.environ.get("GITHUB_ACTIONS") == "true", "GitHub Actions required"
    assert os.environ.get("RUNNER_ENVIRONMENT") == "github-hosted", "disposable hosted runner required"
    assert sys.platform == "darwin" and os.geteuid() != 0
    assert os.environ.get("GITHUB_REPOSITORY") == REPOSITORY
    assert not any(path.exists() for path in (BINARY, PLIST, MOUNT_SOCKET, JOURNAL)), \
        "existing helper state: refusing to replace it"
    assert run("sudo", "-n", "/bin/launchctl", "print", SERVICE, check=False).returncode != 0
    assert run("/bin/launchctl", "print", ACCESS_SERVICE, check=False).returncode != 0, \
        "existing Access registration: refusing to replace it"
    run("sudo", "-n", "true")
    access_registered = False
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
            new_metadata = json.loads(run(str(new_mount / "TelevyBackup.app" / ACCESS_RELATIVE / EXECUTABLE),
                                          "--component-metadata").stdout)
            assert new_metadata["mountHelperProtocolVersion"] == new_protocol
            report("published-identities", old_tag=OLD_TAG, new_tag=NEW_TAG,
                   old_mount_protocol=old_protocol, new_mount_protocol=new_protocol,
                   old_access_version=old_component["component_version"],
                   new_access_version=new_component["component_version"],
                   actual_new_access_mount_protocol=new_metadata["mountHelperProtocolVersion"],
                   access_binary_changed=old_component["sha256"] != new_component["sha256"])
            app = root / "TelevyBackup.app"
            shutil.copytree(old_mount / "TelevyBackup.app", app, symlinks=True)
            config, data = root / "config", root / "data"
            config.mkdir(mode=0o700)
            data.mkdir(mode=0o700)
            (config / "config.toml").write_text("targets = []\n")
            (data / "snapshot-access").mkdir(mode=0o700)
            access_socket = data / "snapshot-access/access.sock"
            env = {"TELEVYBACKUP_CONFIG_DIR": str(config),
                   "TELEVYBACKUP_DATA_DIR": str(data),
                   "TELEVYBACKUP_SNAPSHOT_SOCKET": str(access_socket),
                   "TELEVYBACKUP_SNAPSHOT_JOURNAL": str(data / "journal.sqlite")}
            cli = app / "Contents/MacOS/televybackup-cli"
            access = app / ACCESS_RELATIVE / EXECUTABLE
            installed = True
            run("sudo", "-n", str(cli), "--json", "--config-dir", str(config),
                "--data-dir", str(data), "snapshot-mount-helper", "install")
            wait_for(lambda: request(MOUNT_SOCKET, old_protocol), lambda reply: reply.get("ok") is True)
            with (root / "synthetic-access.log").open("w"):
                agent = plistlib.loads((app / "Contents/Library/LaunchAgents" / (ACCESS_LABEL + ".plist")).read_bytes())
                assert agent["Label"] == ACCESS_LABEL and agent["KeepAlive"] is True
                agent["Program"] = str(access)
                agent["EnvironmentVariables"] = env
                agent["StandardOutPath"] = str(root / "synthetic-access.log")
                agent["StandardErrorPath"] = str(root / "synthetic-access.log")
                agent_path = root / "synthetic-access-agent.plist"
                agent_path.write_bytes(plistlib.dumps(agent))
                run("/bin/launchctl", "bootstrap", f"gui/{os.geteuid()}", str(agent_path))
                access_registered = True
                before = wait_for(lambda: request(access_socket, old_component["protocol_version"]),
                                  lambda reply: reply.get("result", {}).get("mount_helper_reachable") is True)
                old_pid, _ = access_job_state()
                report("old-pair", access_reachable=before["ok"], mount_reachable=True)
                # Replace on-disk binaries at the same canonical paths while preserving
                # the running Access process, as can happen during an app upgrade.
                new_app = new_mount / "TelevyBackup.app"
                replace_file(new_app / "Contents/MacOS/televybackup-cli", cli)
                replace_file(new_app / "Contents/MacOS/televybackup-snapshot-mount-helper",
                             app / "Contents/MacOS/televybackup-snapshot-mount-helper")
                replace_file(new_app / ACCESS_RELATIVE / EXECUTABLE, access)
                assert access_job_state()[0] == old_pid, "old Access exited before the upgrade probe"
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
                outer_status = json.loads(run(str(cli), "--json", "--config-dir", str(config),
                                             "--data-dir", str(data), "snapshot-access", "status").stdout)
                assert outer_status["serviceReachable"] is True
                assert outer_status["accessAppVersion"] == new_component["component_version"]
                assert outer_status["accessAppPath"] == str(app / ACCESS_RELATIVE)
                assert outer_status["mountHelperReachable"] is False
                assert PROTOCOL_ERROR in outer_status["mountHelperError"]
                stable_probes = 0
                for _ in range(3):
                    mount_state = request(MOUNT_SOCKET, new_protocol)
                    access_state = request(access_socket, new_component["protocol_version"])["result"]
                    assert mount_state["ok"] is True and mount_state["version"] == new_protocol
                    assert access_state["mount_helper_reachable"] is False
                    assert PROTOCOL_ERROR in access_state["mount_helper_error"]
                    stable_probes += 1
                current_pid, job_output = access_job_state()
                new_info = plistlib.loads((new_app / "Contents/Info.plist").read_bytes())
                needs_refresh = refresh_probe(job_output, root, str(new_info["CFBundleVersion"]))
                bootstrap = run("/bin/launchctl", "bootstrap", f"gui/{os.geteuid()}",
                                str(agent_path), check=False)
                bootstrap_message = (bootstrap.stdout + bootstrap.stderr).lower()
                report("launch-agent-registration", production_needs_refresh=needs_refresh,
                       parent_bundle_version_present="parent bundle version = " in job_output,
                       bootstrap_exit_code=bootstrap.returncode,
                       already_loaded_marker="already loaded" in bootstrap_message,
                       service_exists_marker="service exists" in bootstrap_message,
                       input_output_error="input/output error" in bootstrap_message,
                       old_access_still_running=access_job_state()[0] == old_pid)
                assert needs_refresh is False, "production refresh gate detected the stale process"
                assert bootstrap.returncode != 0
                assert "already loaded" not in bootstrap_message and "service exists" not in bootstrap_message
                report("after-install", cli_ready=installation["helperReady"],
                       direct_mount_protocol=direct["version"],
                       access_component_version=state["access_app_version"],
                       access_path_matches_current_bundle=state.get("access_app_path") == str(app / ACCESS_RELATIVE),
                       old_access_process_alive=current_pid == old_pid,
                       current_cli_accepts_old_access=outer_status["serviceReachable"],
                       mount_socket_override_present="TELEVYBACKUP_SNAPSHOT_MOUNT_SOCKET" in env,
                       consecutive_matching_probes=stable_probes,
                       mount_reachable=state["mount_helper_reachable"],
                       exact_protocol_error=reproduction)
                run("/bin/launchctl", "kickstart", "-k", ACCESS_SERVICE)
                restarted = wait_for(lambda: request(access_socket, new_component["protocol_version"]),
                                    lambda reply: reply.get("result", {}).get("mount_helper_reachable") is True)
                report("restart-only", mount_reachable=restarted["result"]["mount_helper_reachable"],
                       mount_error_absent=restarted["result"].get("mount_helper_error") is None)
        finally:
            if access_registered:
                run("/bin/launchctl", "bootout", ACCESS_SERVICE, check=False)
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
