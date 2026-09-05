#!/usr/bin/env bash
set -euo pipefail

root_dir="$(git rev-parse --show-toplevel)"
for workflow in ci-pr.yml ci-main.yml label-gate.yml package-ci.yml release-preparation.yml release-completion.yml release.yml notify-release-failure.yml; do
  ruby -ryaml -e 'YAML.parse_file(ARGV.fetch(0))' "$root_dir/.github/workflows/$workflow"
done
for workflow in release-preparation.yml release.yml; do
  text="$(<"$root_dir/.github/workflows/$workflow")"
  [[ "$text" == *"GITHUB_TOKEN"* || "$text" == *"github.token"* ]]
  [[ "$text" == *"expectedHeadOid"* ]]
done
notify_text="$(<"$root_dir/.github/workflows/notify-release-failure.yml")"
[[ "$notify_text" == *"- Release Product"* ]]
[[ "$notify_text" != *"Release exact-tag backfill"* ]]
[[ "$notify_text" == *"recovery:"* ]]
echo "release workflow contract tests passed"
