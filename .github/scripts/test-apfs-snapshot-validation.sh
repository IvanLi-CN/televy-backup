#!/usr/bin/env bash
set -euo pipefail

root_dir="$(git rev-parse --show-toplevel)"
workflow="$root_dir/.github/workflows/apfs-snapshot-acceptance.yml"
pr_workflow="$root_dir/.github/workflows/ci-pr.yml"
actionlint_config="$root_dir/.github/actionlint.yaml"
ci_script="$root_dir/scripts/macos/verify-apfs-snapshot-ci.sh"
long_read_script="$root_dir/scripts/macos/verify-apfs-snapshot-long-read.sh"
integration_script="$root_dir/scripts/macos/verify-apfs-snapshot-integration.sh"

ruby -ryaml -e 'YAML.parse_file(ARGV.fetch(0))' "$workflow"
ruby -ryaml -e 'YAML.parse_file(ARGV.fetch(0))' "$pr_workflow"
grep -F 'televybackup-apfs' "$actionlint_config" >/dev/null

assert_contains() {
  local label="$1"
  local haystack="$2"
  local needle="$3"
  [[ "$haystack" == *"$needle"* ]] || {
    printf 'missing %s: %s\n' "$label" "$needle" >&2
    exit 1
  }
}

assert_not_contains() {
  local label="$1"
  local haystack="$2"
  local needle="$3"
  [[ "$haystack" != *"$needle"* ]] || {
    printf 'unexpected %s: %s\n' "$label" "$needle" >&2
    exit 1
  }
}

workflow_text="$(<"$workflow")"
pr_workflow_text="$(<"$pr_workflow")"
ci_text="$(<"$ci_script")"
long_read_text="$(<"$long_read_script")"
integration_text="$(<"$integration_script")"

assert_contains "workflow name" "$workflow_text" "name: macOS APFS snapshot acceptance"
assert_contains "same-repository label trigger" "$workflow_text" "types: [labeled]"
assert_contains "acceptance label gate" "$workflow_text" "apfs-acceptance"
assert_contains "same-repository safety gate" "$workflow_text" "head.repo.full_name == github.repository"
assert_contains "manual dispatch trigger" "$workflow_text" "workflow_dispatch:"
assert_not_contains "unsafe pull request target trigger" "$workflow_text" "pull_request_target"
assert_contains "read-only workflow permission" "$workflow_text" $'permissions:\n  contents: read'
assert_contains "runner inventory permission" "$workflow_text" "actions: read"
assert_contains "non-preemptive acceptance concurrency" "$workflow_text" "cancel-in-progress: false"
assert_contains "dedicated runner label" "$workflow_text" "runs-on: [self-hosted, macOS, televybackup-apfs]"
assert_contains "runner registration preflight" "$workflow_text" "actions/runners?per_page=100"
assert_contains "runner preflight dependency" "$workflow_text" "needs: [contract, runner-preflight]"
assert_contains "stable app root variable" "$workflow_text" "vars.TELEVYBACKUP_APFS_APP_ROOT"
assert_contains "stable signing identity variable" "$workflow_text" "vars.TELEVYBACKUP_APFS_CODESIGN_IDENTITY"
assert_contains "passwordless sudo preflight" "$workflow_text" "sudo -n true"
assert_contains "FDA preflight message" "$workflow_text" "FDA pre-granted"
assert_contains "candidate orchestrator invocation" "$workflow_text" "verify-apfs-snapshot-ci.sh"
assert_contains "sanitized summary artifact" "$workflow_text" "summary.json"
assert_contains "sanitized timepoint artifact" "$workflow_text" "a5.json"
assert_contains "sanitized long-read artifact" "$workflow_text" "a6.json"
assert_not_contains "raw access log upload" "$workflow_text" "access.log"
assert_not_contains "raw mount log upload" "$workflow_text" "mount-helper.log"
assert_not_contains "mutable checkout action" "$workflow_text" "actions/checkout@v4"
assert_not_contains "mutable toolchain action" "$workflow_text" "dtolnay/rust-toolchain@v1"
assert_not_contains "mutable artifact action" "$workflow_text" "actions/upload-artifact@v4"

assert_contains "manual regression job" "$pr_workflow_text" "manual-snapshot-regression"
assert_contains "manual regression dispatch gate" "$pr_workflow_text" "github.event_name == 'workflow_dispatch'"
assert_contains "manual regression Snapshot Access tests" "$pr_workflow_text" "cargo test --locked -p televybackup-snapshot-access"
assert_contains "manual regression core test" "$pr_workflow_text" "strict_snapshot_read_errors_keep_the_snapshot_access_code"
assert_contains "manual regression daemon test" "$pr_workflow_text" "brokered_source_reports_snapshot_access_failures_without_invalidating_config"
assert_contains "manual regression contract test" "$pr_workflow_text" "test-apfs-snapshot-validation.sh"
assert_contains "manual regression pinned checkout action" "$pr_workflow_text" "actions/checkout@11d5960a326750d5838078e36cf38b85af677262"
assert_contains "manual regression pinned toolchain action" "$pr_workflow_text" "dtolnay/rust-toolchain@6c977a6ca4077a0ceb28ffbe03f59d46e9ac8772"
assert_contains "manual real APFS job" "$pr_workflow_text" "manual-apfs-real-acceptance"
assert_contains "manual real APFS dispatch gate" "$pr_workflow_text" "github.event_name == 'workflow_dispatch'"
assert_contains "manual real APFS label gate" "$pr_workflow_text" "apfs-acceptance"
assert_contains "manual real APFS hosted runner" "$pr_workflow_text" "runs-on: macos-15-intel"
assert_contains "manual real APFS destination image" "$pr_workflow_text" "hdiutil create"
assert_contains "manual real APFS destination mount" "$pr_workflow_text" "hdiutil attach"
assert_contains "manual real APFS destination registration" "$pr_workflow_text" "tmutil setdestination"
assert_contains "manual real APFS destination cleanup" "$pr_workflow_text" "tmutil removedestination"
assert_contains "manual real APFS candidate build" "$pr_workflow_text" "scripts/macos/build-app.sh"
assert_contains "manual real APFS orchestrator" "$pr_workflow_text" "scripts/macos/verify-apfs-snapshot-ci.sh"
assert_contains "manual real APFS sanitized artifact" "$pr_workflow_text" "apfs-snapshot-hosted-evidence"
assert_contains "manual real APFS pinned artifact action" "$pr_workflow_text" "actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02"

assert_contains "orchestrator fail-closed shell" "$ci_text" "set -euo pipefail"
assert_contains "orchestrator APFS check" "$ci_text" "diskutil info"
assert_contains "orchestrator filesystem device resolution" "$ci_text" 'fixture_device="$(df -P "$source_root"'
assert_not_contains "orchestrator directory diskutil lookup" "$ci_text" 'diskutil info "$source_root"'
assert_contains "orchestrator root helper socket" "$ci_text" "TELEVYBACKUP_SNAPSHOT_MOUNT_SOCKET"
assert_contains "orchestrator status helper socket propagation" "$ci_text" 'TELEVYBACKUP_SNAPSHOT_MOUNT_JOURNAL="$mount_journal"'
assert_contains "orchestrator access socket alignment" "$ci_text" 'access_socket="$data_root/snapshot-access/access.sock"'
assert_contains "orchestrator FDA check" "$ci_text" ".fdaReady == true"
assert_contains "orchestrator sanitized timepoint" "$ci_text" "verify-apfs-snapshot-integration.sh"
assert_contains "orchestrator timepoint access socket" "$ci_text" 'TELEVYBACKUP_SNAPSHOT_SOCKET="$access_socket"'
assert_contains "orchestrator timepoint access journal" "$ci_text" 'TELEVYBACKUP_SNAPSHOT_JOURNAL="$access_journal"'
assert_contains "orchestrator sanitized long read" "$ci_text" "verify-apfs-snapshot-long-read.sh"
assert_contains "synthetic fixture guard" "$ci_text" "TELEVYBACKUP_SANITIZED_FIXTURE_ROOT"
assert_contains "canonical timepoint card" "$ci_text" "snapshotReadPreMutation: true"
assert_contains "canonical long-read card" "$ci_text" "healthyReadCompleted: true"
assert_not_contains "raw timepoint card copy" "$ci_text" 'cp "$a5_output" "$evidence_dir/a5.json"'
assert_not_contains "raw long-read card copy" "$ci_text" 'cp "$a6_output" "$evidence_dir/a6.json"'
assert_contains "orchestrator artifact schema" "$ci_text" "televybackup.apfs_snapshot_ci.v1"
assert_contains "orchestrator no published snapshot assertion" "$ci_text" "publishedSnapshot: false"
# The single-quoted needle intentionally checks the literal shell source.
# shellcheck disable=SC2016
assert_contains "orchestrator narrow root cleanup" "$ci_text" 'sudo -n rm -rf -- "$mount_runtime_root"'
assert_not_contains "orchestrator broad root cleanup" "$ci_text" "rm -rf /"

assert_contains "long read fail-closed shell" "$long_read_text" "set -euo pipefail"
assert_contains "long read exact unmount" "$long_read_text" "sudo -n /sbin/umount -f"
assert_contains "long read diagnostic assertion" "$long_read_text" 'snapshot read failed'
assert_contains "long read errno assertion" "$long_read_text" 'errno='
assert_contains "long read sanitized schema" "$long_read_text" "televybackup.apfs_snapshot_long_read.v1"
assert_contains "long read synthetic fixture guard" "$long_read_text" "TELEVYBACKUP_SANITIZED_FIXTURE_ROOT"
assert_not_contains "long read live fallback" "$long_read_text" "live-source"

assert_contains "timepoint synthetic fixture guard" "$integration_text" "TELEVYBACKUP_SANITIZED_FIXTURE_ROOT"
assert_contains "timepoint synthetic target guard" "$integration_text" '[[ "$target_id" == "apfs-ci" ]]'
assert_not_contains "timepoint transcript persistence in sanitized mode" "$integration_text" 'printf '\''%s\\n'\'' "$last_response" >>"$evidence_path"'

assert_contains "dedicated APFS manual gate" "$workflow_text" "github.event_name == 'workflow_dispatch'"

echo "APFS snapshot validation workflow contract passed"
