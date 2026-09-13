#!/usr/bin/env bash
set -euo pipefail

root_dir="$(git rev-parse --show-toplevel)"
text="$(<"$root_dir/.github/workflows/notify-release-failure.yml")"
for marker in identity_status no-identity resolver-error merge_sha recovery_candidate reservation_ref; do
  [[ "$text" == *"$marker"* ]] || { echo "missing failure context marker: $marker" >&2; exit 1; }
done
[[ "$text" == *"not-applicable: superseded_by_product_tag"* ]]
[[ "$text" != *"contents/VERSION"* ]]
[[ "$text" != *"base64.b64decode"* ]]
[[ "$text" == *"no version or tag was inferred"* ]]
echo "release failure context contract tests passed"
