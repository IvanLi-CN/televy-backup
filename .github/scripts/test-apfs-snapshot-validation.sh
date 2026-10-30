#!/usr/bin/env bash
set -euo pipefail

root_dir="$(git rev-parse --show-toplevel)"
pr_workflow="$root_dir/.github/workflows/ci-pr.yml"
main_workflow="$root_dir/.github/workflows/ci-main.yml"
ci_script="$root_dir/scripts/macos/verify-apfs-snapshot-ci.sh"
long_read_script="$root_dir/scripts/macos/verify-apfs-snapshot-long-read.sh"
integration_script="$root_dir/scripts/macos/verify-apfs-snapshot-integration.sh"
mount_helper_source="$root_dir/crates/snapshot-helper/src/mount_helper.rs"
access_source="$root_dir/crates/snapshot-helper/src/main.rs"

ruby -ryaml -e 'YAML.parse_file(ARGV.fetch(0))' "$pr_workflow"
ruby -ryaml -e 'YAML.parse_file(ARGV.fetch(0))' "$main_workflow"

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

pr_workflow_text="$(<"$pr_workflow")"
main_workflow_text="$(<"$main_workflow")"
ci_text="$(<"$ci_script")"
long_read_text="$(<"$long_read_script")"
integration_text="$(<"$integration_script")"
mount_helper_integration_text="$(<"$root_dir/scripts/macos/mount-helper-install-integration-test.py")"
mount_helper_text="$(<"$mount_helper_source")"
access_text="$(<"$access_source")"

assert_contains "manual regression job" "$pr_workflow_text" "manual-snapshot-regression"
assert_contains "manual regression dispatch gate" "$pr_workflow_text" "github.event_name == 'workflow_dispatch'"
assert_contains "manual regression Snapshot Access tests" "$pr_workflow_text" "cargo test --locked -p televybackup-snapshot-access"
assert_contains "manual regression core test" "$pr_workflow_text" "strict_snapshot_read_errors_keep_the_snapshot_access_code"
assert_contains "manual regression daemon test" "$pr_workflow_text" "brokered_source_reports_snapshot_access_failures_without_invalidating_config"
assert_contains "manual regression snapshot timepoint fixture" "$pr_workflow_text" "snapshot_timepoint_reads_pre_mutation_after_live_change"
assert_contains "manual regression mount health fixture" "$pr_workflow_text" "long_read_fails_closed_after_mount_identity_loss"
assert_contains "manual regression stale mount recovery fixture" "$pr_workflow_text" "test-snapshot-mount-recovery.py"
assert_contains "legacy helper blocking upgrade fixture" "$pr_workflow_text" "--legacy-blocking"
assert_contains "manual regression contract test" "$pr_workflow_text" "test-apfs-snapshot-validation.sh"
assert_contains "manual regression pinned checkout action" "$pr_workflow_text" "actions/checkout@11d5960a326750d5838078e36cf38b85af677262"
assert_contains "manual regression pinned toolchain action" "$pr_workflow_text" "dtolnay/rust-toolchain@6c977a6ca4077a0ceb28ffbe03f59d46e9ac8772"
assert_not_contains "removed unsupported acceptance job" "$pr_workflow_text" "manual-apfs-real-acceptance"
assert_not_contains "removed unsupported runner label" "$pr_workflow_text" "televybackup-apfs"
assert_not_contains "removed unsupported acceptance label" "$pr_workflow_text" "apfs-acceptance"
assert_not_contains "removed unsupported evidence artifact" "$pr_workflow_text" "apfs-snapshot-hosted-evidence"
[[ ! -e "$root_dir/.github/workflows/apfs-snapshot-acceptance.yml" ]] || {
  echo "unsupported standalone APFS acceptance workflow still exists" >&2
  exit 1
}

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
assert_contains "hosted stale mount recovery fixture" "$main_workflow_text" "test-snapshot-mount-recovery.py"
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
assert_contains "legacy journal seed" "$mount_helper_integration_text" "seed_legacy_stale_journal"
assert_contains "legacy journal classification assertion" "$mount_helper_integration_text" "recoverable_stale_mounts"

assert_contains "root-owned snapshot creation" "$mount_helper_text" '"/usr/bin/tmutil"'
assert_contains "snapshot creation diagnostic stage" "$mount_helper_text" '"snapshot_create"'
assert_contains "single snapshot ownership result" "$mount_helper_text" 'manifest.len() != 1'
assert_contains "pre-journal inventory fence" "$mount_helper_text" 'snapshot inventory changed while recording cleanup'
assert_contains "post-journal inventory fence" "$mount_helper_text" 'snapshot inventory changed after cleanup was recorded'
assert_contains "release inventory fence" "$mount_helper_text" 'inventory_matches_expected('
assert_contains "released tombstone audit" "$mount_helper_text" 'audit_released_tombstones(journal, volume_uuid, device_identifier)'
assert_contains "released tombstone has a valid baseline" "$mount_helper_text" 'reservation_manifest_valid = 1'
assert_contains "stale inventory change reblocks" "$mount_helper_text" "released snapshot tombstone inventory changed"
assert_contains "creation baseline is exact" "$mount_helper_text" 'if !manifests_equal(&current_manifest, &reservation_manifest)'
assert_contains "deletion requires original baseline" "$mount_helper_text" "A post-deletion inventory change is unowned"
assert_contains "partial deletion retains unconfirmed manifest" "$mount_helper_text" "retain_unconfirmed_manifest_after_delete_failure"
assert_contains "deletion clears recorded manifest" "$mount_helper_text" "snapshot_manifest = '[]', reservation_manifest = ?"
assert_contains "fresh reservation release remains blocked" "$mount_helper_text" 'if state == "reserved" && !stale_reserved'
assert_contains "pending cleanup retry bypasses reservation age gate" "$mount_helper_text" 'if (state == "cleanup_pending" || stale_reserved)'
assert_contains "lease source-path binding" "$mount_helper_text" 'authorize_source_path(uid, source_path, &source)'
assert_contains "mount source-path reauthorization" "$mount_helper_text" 'authorize_source_path(uid, stored_source_path, &source)'
assert_contains "source-path authorization uses exec boundary" "$mount_helper_text" 'Command::new(executable)'
assert_contains "source-path authorization has pre-runtime entrypoint" "$mount_helper_text" 'maybe_authorize_source_path'
assert_not_contains "source-path authorization does not fork Tokio helper" "$mount_helper_text" 'libc::fork()'
assert_not_contains "access-side snapshot creation" "$access_text" 'run_command("/usr/bin/tmutil", ["localsnapshot"])'

echo "APFS snapshot validation workflow contract passed"
