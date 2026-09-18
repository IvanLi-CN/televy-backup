#!/usr/bin/env bash
set -euo pipefail

root_dir="$(git rev-parse --show-toplevel)"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT
export RUNNER_TEMP="$tmp_dir/runner"
mkdir -p "$RUNNER_TEMP"
remote_dir="$tmp_dir/remote.git"
repo_dir="$tmp_dir/repo"
bin_dir="$tmp_dir/bin"
mkdir -p "$bin_dir"
git init --bare -q "$remote_dir"
git clone -q "$remote_dir" "$repo_dir"
git -C "$repo_dir" config user.name fixture
git -C "$repo_dir" config user.email fixture@example.com
printf '0.9.9\n' > "$repo_dir/VERSION"
printf 'fixture\n' > "$repo_dir/README"
git -C "$repo_dir" add .
git -C "$repo_dir" commit -qm source
git -C "$repo_dir" branch -M main
git -C "$repo_dir" push -q origin main
release_sha="$(git -C "$repo_dir" rev-parse HEAD)"
mkdir -p "$repo_dir/.github/scripts" "$repo_dir/scripts"
cp "$root_dir/.github/scripts/release_chain.py" "$repo_dir/.github/scripts/release_chain.py"
cp "$root_dir/scripts/product-version.py" "$repo_dir/scripts/product-version.py"

ruby -ryaml - "$root_dir/.github/workflows/release.yml" "$tmp_dir" <<'RUBY'
workflow = YAML.load_file(ARGV.fetch(0))
output = ARGV.fetch(1)
steps = workflow.fetch("jobs").fetch("publish").fetch("steps")
{
  "Create or verify immutable product tag" => "tag.sh",
  "Create or update GitHub Release" => "release.sh",
}.each do |name, file|
  run = steps.find { |step| step["name"] == name }.fetch("run")
  File.write(File.join(output, file), "#!/usr/bin/env bash\n#{run}")
end
RUBY
chmod +x "$tmp_dir"/*.sh

cat > "$bin_dir/gh" <<'GH'
#!/usr/bin/env bash
set -euo pipefail
state_dir="${GH_FIXTURE_STATE:?}"
repo_dir="${GH_FIXTURE_REPO:?}"
mkdir -p "$state_dir"
if [[ "${1:-}" == api ]]; then
  shift
  method=GET
  if [[ "${1:-}" == --method ]]; then method="$2"; shift 2; fi
  endpoint="$1"; shift
  tag=""; object=""; ref=""
  while (($#)); do
    case "$1" in
      -f)
        case "$2" in
          tag=*) tag="${2#tag=}" ;;
          object=*) object="${2#object=}" ;;
          ref=*) ref="${2#ref=}" ;;
        esac
        shift 2 ;;
      --jq) shift 2 ;;
      *) shift ;;
    esac
  done
  if [[ "$method" == GET && "$endpoint" == */releases/latest ]]; then
    latest_tag=""
    if [[ -f "$state_dir/latest" ]]; then
      latest_tag="$(<"$state_dir/latest")"
    fi
    if [[ -z "$latest_tag" || ! -f "$state_dir/$latest_tag" ]]; then
      echo "HTTP 404: Not Found" >&2
      exit 1
    fi
    printf '{"tag_name":"%s"}\n' "$latest_tag"
    exit 0
  fi
  if [[ "$method" == GET && "$endpoint" == */releases/tags/* ]]; then
    tag="${endpoint##*/releases/tags/}"
    if [[ ! -f "$state_dir/$tag" ]]; then
      echo "HTTP 404: Not Found" >&2
      exit 1
    fi
    cat "$state_dir/$tag"
    exit 0
  fi
  if [[ "$method" == POST && "$endpoint" == */git/tags ]]; then
    git -C "$repo_dir" -c user.name='github-actions[bot]' \
      -c user.email='41898282+github-actions[bot]@users.noreply.github.com' \
      tag -a "$tag" -m "$tag" "$object"
    printf '{"sha":"%s"}\n' "$(git -C "$repo_dir" rev-parse "refs/tags/$tag")"
    exit 0
  fi
  if [[ "$method" == POST && "$endpoint" == */git/refs ]]; then
    tag="${ref#refs/tags/}"
    git -C "$repo_dir" push -q origin "refs/tags/$tag:refs/tags/$tag"
    printf '{}\n'
    exit 0
  fi
  echo "unsupported fixture gh api call: $method $endpoint" >&2
  exit 1
fi
if [[ "$1" == release && "$2" == create ]]; then
  tag="$3"
  printf '%s\n' "$*" > "$state_dir/$tag.created"
  draft_state=false
  [[ "$*" == *'--draft'* ]] && draft_state=true
  printf '{"tag_name":"%s","draft":%s,"prerelease":%s}\n' "$tag" "$draft_state" \
    "$([[ "$*" == *'--prerelease'* ]] && echo true || echo false)" > "$state_dir/$tag"
  if [[ "$*" == *'--latest=true'* ]]; then
    printf '%s\n' "$tag" > "$state_dir/latest"
  fi
  exit 0
fi
if [[ "$1" == release && "$2" == upload ]]; then
  printf '%s\n' "$*" >> "$state_dir/uploads"
  if [[ "${GH_FIXTURE_PUBLISH_ON_UPLOAD:-0}" == 1 ]]; then
    printf '{"tag_name":"%s","draft":false,"prerelease":false}\n' "$3" > "$state_dir/$3"
  else
    release_json="$(<"$state_dir/$3")"
    uploaded_name="$(basename "$4")"
    uploaded_digest="sha256:$(shasum -a 256 "$4" | awk '{print $1}')"
    printf '%s' "$release_json" | jq -c --arg name "$uploaded_name" --arg digest "$uploaded_digest" \
      '.assets = ((.assets // []) + [{name: $name, digest: $digest}])' > "$state_dir/$3.tmp"
    mv "$state_dir/$3.tmp" "$state_dir/$3"
  fi
  exit 0
fi
if [[ "$1" == release && "$2" == edit ]]; then
  tag="$3"
  printf '{"tag_name":"%s","draft":false,"prerelease":%s}\n' "$tag" \
    "$([[ "$*" == *'--prerelease'* ]] && echo true || echo false)" > "$state_dir/$tag"
  if [[ "$*" == *'--latest=true'* ]]; then
    printf '%s\n' "$tag" > "$state_dir/latest"
  fi
  exit 0
fi
echo "unsupported fixture gh call: $*" >&2
exit 1
GH
chmod +x "$bin_dir/gh"

run_publish_fixture() {
  local version="$1"
  local channel="$2"
  local expected_flags="$3"
  local tag="v${version}"
  rm -rf "$repo_dir/release-assets"
  mkdir -p "$repo_dir/release-assets"
  printf '%s\n' "$tag" > "$repo_dir/release-assets/asset.txt"
  printf '{"assets":[{"name":"asset.txt"}]}\n' > "$repo_dir/release-assets/BUILD-MANIFEST.json"
  printf 'placeholder  asset.txt\n' > "$repo_dir/release-assets/SHA256SUMS"
  (
    cd "$repo_dir"
    export PATH="$bin_dir:$PATH"
    export GH_FIXTURE_REPO="$repo_dir" GH_FIXTURE_STATE="$tmp_dir/state"
    export GITHUB_REPOSITORY=fixture/repo GITHUB_API_URL=https://fixture.invalid GH_TOKEN=fixture
  export PRODUCT_TAG="$tag" PRODUCT_VERSION="$version" PRODUCT_CHANNEL="$channel"
  export RELEASE_SHA="$release_sha" RELEASE_STATE=missing BOUND_IDENTITY=present
  bash "$tmp_dir/tag.sh"
  bash "$tmp_dir/release.sh"
  grep -F -- "$expected_flags" "$tmp_dir/state/$tag.created"
  grep -F -- '--draft' "$tmp_dir/state/$tag.created"
    git show-ref --verify --quiet "refs/tags/$tag"
    [[ "$(git rev-parse "refs/tags/$tag^{commit}")" == "$release_sha" ]]
  )
}

run_publish_fixture "1.2.3" prod --latest=true
run_publish_fixture "1.2.4-rc.1" rc --prerelease

published_tag="v1.2.3"
rm -f "$tmp_dir/state/$published_tag.created"
(
  cd "$repo_dir"
  export PATH="$bin_dir:$PATH"
  export GH_FIXTURE_REPO="$repo_dir" GH_FIXTURE_STATE="$tmp_dir/state"
  export GITHUB_REPOSITORY=fixture/repo GITHUB_API_URL=https://fixture.invalid GH_TOKEN=fixture
  export PRODUCT_TAG="$published_tag" PRODUCT_VERSION=1.2.3 PRODUCT_CHANNEL=prod
  export RELEASE_SHA="$release_sha" RELEASE_STATE=published BOUND_IDENTITY=present
  bash "$tmp_dir/release.sh"
  [[ ! -f "$tmp_dir/state/$published_tag.created" ]]
)

draft_tag="v1.2.5"
draft_asset_digest="$(shasum -a 256 "$repo_dir/release-assets/asset.txt" | awk '{print $1}')"
draft_manifest_digest="$(shasum -a 256 "$repo_dir/release-assets/BUILD-MANIFEST.json" | awk '{print $1}')"
draft_checksums_digest="$(shasum -a 256 "$repo_dir/release-assets/SHA256SUMS" | awk '{print $1}')"
jq -cn \
  --arg tag "$draft_tag" \
  --arg asset_digest "$draft_asset_digest" \
  --arg manifest_digest "$draft_manifest_digest" \
  --arg checksums_digest "$draft_checksums_digest" \
  '{tag_name:$tag,draft:true,prerelease:false,assets:[
    {name:"asset.txt",digest:("sha256:" + $asset_digest)},
    {name:"BUILD-MANIFEST.json",digest:("sha256:" + $manifest_digest)},
    {name:"SHA256SUMS",digest:("sha256:" + $checksums_digest)}
  ]}' > "$tmp_dir/state/$draft_tag"
rm -f "$tmp_dir/state/uploads"
(
  cd "$repo_dir"
  export PATH="$bin_dir:$PATH"
  export GH_FIXTURE_REPO="$repo_dir" GH_FIXTURE_STATE="$tmp_dir/state"
  export GITHUB_REPOSITORY=fixture/repo GITHUB_API_URL=https://fixture.invalid GH_TOKEN=fixture
  export PRODUCT_TAG="$draft_tag" PRODUCT_VERSION=1.2.5 PRODUCT_CHANNEL=prod
  export RELEASE_SHA="$release_sha" RELEASE_STATE=draft BOUND_IDENTITY=present
  bash "$tmp_dir/release.sh"
  [[ ! -f "$tmp_dir/state/uploads" ]]
)

extra_tag="v1.2.56"
printf '{"tag_name":"%s","draft":true,"prerelease":false,"assets":[]}\n' \
  "$extra_tag" > "$tmp_dir/state/$extra_tag"
printf 'unlisted asset\n' > "$repo_dir/release-assets/unlisted.txt"
if (
  cd "$repo_dir"
  export PATH="$bin_dir:$PATH"
  export GH_FIXTURE_REPO="$repo_dir" GH_FIXTURE_STATE="$tmp_dir/state"
  export GITHUB_REPOSITORY=fixture/repo GITHUB_API_URL=https://fixture.invalid GH_TOKEN=fixture
  export PRODUCT_TAG="$extra_tag" PRODUCT_VERSION=1.2.56 PRODUCT_CHANNEL=prod
  export RELEASE_SHA="$release_sha" RELEASE_STATE=draft BOUND_IDENTITY=present
  bash "$tmp_dir/release.sh"
); then
  echo "draft publication accepted an unlisted release asset" >&2
  exit 1
fi
rm -f "$repo_dir/release-assets/unlisted.txt"

upload_tag="v1.2.55"
printf '{"tag_name":"%s","draft":true,"prerelease":false,"assets":[]}\n' \
  "$upload_tag" > "$tmp_dir/state/$upload_tag"
rm -f "$tmp_dir/state/uploads"
(
  cd "$repo_dir"
  export PATH="$bin_dir:$PATH"
  export GH_FIXTURE_REPO="$repo_dir" GH_FIXTURE_STATE="$tmp_dir/state"
  export GITHUB_REPOSITORY=fixture/repo GITHUB_API_URL=https://fixture.invalid GH_TOKEN=fixture
  export PRODUCT_TAG="$upload_tag" PRODUCT_VERSION=1.2.55 PRODUCT_CHANNEL=prod
  export RELEASE_SHA="$release_sha" RELEASE_STATE=draft BOUND_IDENTITY=present
  bash "$tmp_dir/release.sh"
  [[ "$(wc -l < "$tmp_dir/state/uploads")" -eq 3 ]]
)

conflict_tag="v1.2.6"
printf '{"tag_name":"%s","draft":true,"prerelease":false,"assets":[{"name":"asset.txt","digest":"sha256:%064d"}]}\n' \
  "$conflict_tag" 0 > "$tmp_dir/state/$conflict_tag"
if (
  cd "$repo_dir"
  export PATH="$bin_dir:$PATH"
  export GH_FIXTURE_REPO="$repo_dir" GH_FIXTURE_STATE="$tmp_dir/state"
  export GITHUB_REPOSITORY=fixture/repo GITHUB_API_URL=https://fixture.invalid GH_TOKEN=fixture
  export PRODUCT_TAG="$conflict_tag" PRODUCT_VERSION=1.2.6 PRODUCT_CHANNEL=prod
  export RELEASE_SHA="$release_sha" RELEASE_STATE=draft BOUND_IDENTITY=present
  bash "$tmp_dir/release.sh"
); then
  echo "draft publication accepted a conflicting existing asset digest" >&2
  exit 1
fi

race_tag="v1.2.7"
printf '{"tag_name":"%s","draft":true,"prerelease":false,"assets":[]}\n' \
  "$race_tag" > "$tmp_dir/state/$race_tag"
printf 'second asset\n' > "$repo_dir/release-assets/second.txt"
jq '.assets += [{"name":"second.txt"}]' "$repo_dir/release-assets/BUILD-MANIFEST.json" > "$repo_dir/release-assets/BUILD-MANIFEST.json.tmp"
mv "$repo_dir/release-assets/BUILD-MANIFEST.json.tmp" "$repo_dir/release-assets/BUILD-MANIFEST.json"
rm -f "$tmp_dir/state/uploads"
if (
  cd "$repo_dir"
  export PATH="$bin_dir:$PATH"
  export GH_FIXTURE_REPO="$repo_dir" GH_FIXTURE_STATE="$tmp_dir/state"
  export GITHUB_REPOSITORY=fixture/repo GITHUB_API_URL=https://fixture.invalid GH_TOKEN=fixture
  export PRODUCT_TAG="$race_tag" PRODUCT_VERSION=1.2.7 PRODUCT_CHANNEL=prod
  export RELEASE_SHA="$release_sha" RELEASE_STATE=draft BOUND_IDENTITY=present
  export GH_FIXTURE_PUBLISH_ON_UPLOAD=1
  bash "$tmp_dir/release.sh"
); then
  echo "draft publication ignored a Release publication race" >&2
  exit 1
fi
[[ "$(wc -l < "$tmp_dir/state/uploads")" -eq 1 ]]

echo "release workflow execution fixture tests passed"
