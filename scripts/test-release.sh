#!/usr/bin/env bash
# Hermetic release tests: disposable local Git remotes and fake GitHub CLI only.
set -Eeuo pipefail
script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
source "$script_dir/release-version.sh"
for value in 0.0.0 1.2.3 v1.2.3 1.2.3-alpha.0 1.2.3-beta.2 v1.2.3-rc.1; do
  [[ $(release_version "$value") == "${value#v}" ]]
done
for value in '' v 1.2 01.2.3 1.02.3 1.2.03 10000.0.0 1.2.3-rc.01 1.2.3-dev '1.2.3+meta' '1.2.3;echo bad'; do
  if release_version "$value" >/dev/null 2>&1; then echo "accepted invalid version: $value" >&2; exit 1; fi
done
temp=$(mktemp -d)
trap 'rm -rf "$temp"' EXIT
export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null
export GIT_AUTHOR_NAME=ReleaseTest GIT_AUTHOR_EMAIL=release@example.invalid
export GIT_COMMITTER_NAME=$GIT_AUTHOR_NAME GIT_COMMITTER_EMAIL=$GIT_AUTHOR_EMAIL
export GITHUB_ACTIONS=true GITHUB_RUN_ID=123 GH_REPO=example/recon
export FAKE_GH_STATE="$temp/gh-state"
mkdir -p "$FAKE_GH_STATE" "$temp/bin" "$temp/repo/scripts"
export PATH="$temp/bin:$PATH"
cat > "$temp/bin/gh" <<'GH'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "$FAKE_GH_STATE/calls"
case "$1 $2" in
  'workflow run') printf '%s\n' "$@" > "$FAKE_GH_STATE/dispatch"; [[ ! -e "$FAKE_GH_STATE/fail-dispatch" ]] ;;
  'release view') [[ -f "$FAKE_GH_STATE/release.json" ]] || exit 1; cat "$FAKE_GH_STATE/release.json" ;;
  'release create')
    [[ ! -e "$FAKE_GH_STATE/release.json" ]]
    while [[ $# -gt 0 ]]; do
      if [[ $1 == --notes-file ]]; then
        jq -n --rawfile body "$2" '{isDraft: true, body: $body | rtrimstr("\n")}' > "$FAKE_GH_STATE/release.json"
        break
      fi
      shift
    done ;;
  'release upload')
    [[ ! -e "$FAKE_GH_STATE/fail-upload" ]] || exit 1
    for arg in "$@"; do case "$arg" in *.tar.gz|*/SHA256SUMS) [[ -s $arg ]] ;; esac; done ;;
  'release edit')
    jq '.isDraft = false' "$FAKE_GH_STATE/release.json" > "$FAKE_GH_STATE/edit.json"
    mv "$FAKE_GH_STATE/edit.json" "$FAKE_GH_STATE/release.json" ;;
  *) echo "unexpected gh invocation: $*" >&2; exit 1 ;;
esac
GH
chmod +x "$temp/bin/gh"
expect_failure() {
  if "$@" >"$temp/error" 2>&1; then cat "$temp/error" >&2; echo "unexpected success: $*" >&2; exit 1; fi
}
cp "$script_dir/"* "$temp/repo/scripts/"
cp "$script_dir/../"{Cargo.toml,Cargo.lock} "$temp/repo/"
cp "$script_dir/../"{README.md,config.default.toml,CHANGELOG.md} "$temp/repo/"
cp -R "$script_dir/../docs" "$temp/repo/docs"
cd "$temp/repo"
python3 scripts/set-release-version.py v1.2.3-rc.1
python3 - <<'CHECK'
import tomllib
from pathlib import Path
assert tomllib.loads(Path('Cargo.toml').read_text())['package']['version'] == '1.2.3-rc.1'
assert next(p for p in tomllib.loads(Path('Cargo.lock').read_text())['package'] if p['name'] == 'recon')['version'] == '1.2.3-rc.1'
CHECK
expect_failure python3 scripts/set-release-version.py 'invalid'
# First release can use the already committed Cargo version.
python3 scripts/set-release-version.py 1.2.3
for spec in 'macos-x64 x86_64-apple-darwin recon' 'macos-arm64 aarch64-apple-darwin recon' 'windows-x64 x86_64-pc-windows-msvc recon.exe' 'linux-x64 x86_64-unknown-linux-gnu recon'; do
  read -r platform target binary <<< "$spec"
  expect_failure bash scripts/package-release.sh 1.2.3 "$platform"
  mkdir -p "target/$target/release"
  echo binary > "target/$target/release/$binary"
  bash scripts/package-release.sh 1.2.3 "$platform"
  tar -tzf "artifacts/release/recon-1.2.3-$platform.tar.gz" > "$temp/archive-contents"
  for entry in "$binary" README.md config.default.toml CHANGELOG.md docs/GUIDE.md docs/images/overview.png docs/images/file-history.png docs/images/fresh-baseline.png; do
    grep -Fxq "$entry" "$temp/archive-contents"
  done
done
git init -q --bare "$temp/origin.git"
git init -q -b main
printf 'artifacts/\ntarget/\n' > .gitignore
cat > CHANGELOG.md <<'CHANGELOG'
# Changelog

## [Unreleased]

- A noteworthy feature.

## [1.0.0] - 2026-01-01

- An older release.
CHANGELOG
git add scripts docs CHANGELOG.md .gitignore Cargo.toml Cargo.lock README.md config.default.toml
git commit -qm initial
git remote add origin "$temp/origin.git"
git push -q origin main
git --git-dir="$temp/origin.git" symbolic-ref HEAD refs/heads/main
source_sha=$(git rev-parse HEAD)
cp CHANGELOG.md "$temp/original.md"

# Resolve an SSH alias to our local remote, while preserving its visible URL.
git remote set-url origin git@github-test-alias:example/recon.git
git config "url.$temp/origin.git.insteadOf" git@github-test-alias:example/recon.git
export REAL_GIT=$(command -v git)
cat > "$temp/bin/git" <<'GIT'
#!/usr/bin/env bash
if [[ "$*" == 'remote get-url origin' ]]; then echo git@github-test-alias:example/recon.git
else exec "$REAL_GIT" "$@"; fi
GIT
chmod +x "$temp/bin/git"
touch dirty
expect_failure bash scripts/release.sh 1.2.3
rm dirty
git checkout -q --detach
expect_failure bash scripts/release.sh 1.2.3
git checkout -q main
bash scripts/release.sh v1.2.3
grep -Fxq 'version=1.2.3' "$FAKE_GH_STATE/dispatch"
grep -Fxq "expected_sha=$source_sha" "$FAKE_GH_STATE/dispatch"
grep -Fxq 'github.com/example/recon' "$FAKE_GH_STATE/dispatch"
[[ $(git rev-parse HEAD) == "$source_sha" && -z $(git tag) ]]
cmp CHANGELOG.md "$temp/original.md"
touch "$FAKE_GH_STATE/fail-dispatch"
expect_failure bash scripts/release.sh 1.2.3
rm "$FAKE_GH_STATE/fail-dispatch"
git commit -qm 'unpushed work' --allow-empty
expect_failure bash scripts/release.sh 1.2.3
rm "$temp/bin/git"

bash scripts/release-changelog.sh 1.2.3 CHANGELOG.md > artifacts/release/CHANGELOG.md
printf '\n- A noteworthy feature.\n' > artifacts/release/release-notes.md
cmp CHANGELOG.md "$temp/original.md"
expect_failure bash scripts/release-changelog.sh 1.0.0 CHANGELOG.md
for content in '# Missing section' '## [1.0.0]' "$(printf '## [Unreleased]\n## [Unreleased]')"; do
  printf '%s\n' "$content" > "$temp/invalid.md"
  expect_failure bash scripts/release-changelog.sh 2.0.0 "$temp/invalid.md"
done
new_checkout() {
  git clone -q "$temp/origin.git" "$temp/$1"
  cd "$temp/$1"
  git checkout -q --detach "$source_sha"
  mkdir -p artifacts/release
  cp "$temp/repo/artifacts/release/"* artifacts/release/
}
new_checkout missing
expect_failure env GITHUB_ACTIONS=false bash scripts/finalize-release.sh 1.2.3 main "$source_sha" artifacts/release
rm artifacts/release/recon-1.2.3-windows-x64.tar.gz
expect_failure bash scripts/finalize-release.sh 1.2.3 main "$source_sha" artifacts/release
[[ -z $(git --git-dir="$temp/origin.git" tag) ]]
git -C "$temp/repo" push -q origin main
new_checkout advanced
expect_failure bash scripts/finalize-release.sh 1.2.3 main "$source_sha" artifacts/release
git --git-dir="$temp/origin.git" update-ref refs/heads/main "$source_sha"
new_checkout race
advanced_sha=$(git -C "$temp/repo" rev-parse main)
cat > .git/hooks/pre-push <<HOOK
#!/bin/sh
git --git-dir="$temp/origin.git" update-ref refs/heads/main "$advanced_sha"
HOOK
chmod +x .git/hooks/pre-push
expect_failure bash scripts/finalize-release.sh 1.2.3 main "$source_sha" artifacts/release
[[ $(git --git-dir="$temp/origin.git" rev-parse main) == "$advanced_sha" ]]
[[ -z $(git --git-dir="$temp/origin.git" tag) ]]
git --git-dir="$temp/origin.git" update-ref refs/heads/main "$source_sha"
printf '#!/bin/sh\nexit 1\n' > "$temp/origin.git/hooks/pre-receive"
chmod +x "$temp/origin.git/hooks/pre-receive"
new_checkout rejected
expect_failure bash scripts/finalize-release.sh 1.2.3 main "$source_sha" artifacts/release
[[ $(git --git-dir="$temp/origin.git" rev-parse main) == "$source_sha" ]]
[[ -z $(git --git-dir="$temp/origin.git" tag) && ! -e "$FAKE_GH_STATE/release.json" ]]
rm "$temp/origin.git/hooks/pre-receive"
new_checkout upload_failure
touch "$FAKE_GH_STATE/fail-upload"
expect_failure bash scripts/finalize-release.sh 1.2.3 main "$source_sha" artifacts/release
release_sha=$(git --git-dir="$temp/origin.git" rev-parse 'v1.2.3^{}')
[[ $(git --git-dir="$temp/origin.git" rev-parse main) == "$release_sha" ]]
[[ $(git diff --name-only "$source_sha" "$release_sha") == CHANGELOG.md ]]
[[ $(jq -r .isDraft "$FAKE_GH_STATE/release.json") == true ]]
grep -Fq "## [1.2.3] - $(date -u +%Y-%m-%d)" CHANGELOG.md
grep -Fq '## [1.0.0] - 2026-01-01' CHANGELOG.md
[[ $(grep -Fc '## [Unreleased]' CHANGELOG.md) == 1 ]]
awk '/^## \[1.0.0\]/ { section = 1 } section { print }' CHANGELOG.md > "$temp/released.md"
awk '/^## \[1.0.0\]/ { section = 1 } section { print }' "$temp/original.md" | cmp - "$temp/released.md"
new_checkout wrong_run
expect_failure env GITHUB_RUN_ID=456 bash scripts/finalize-release.sh 1.2.3 main "$source_sha" artifacts/release
new_checkout tampered
echo tampered >> artifacts/release/CHANGELOG.md
expect_failure bash scripts/finalize-release.sh 1.2.3 main "$source_sha" artifacts/release
new_checkout retry
rm "$FAKE_GH_STATE/fail-upload"
bash scripts/finalize-release.sh 1.2.3 main "$source_sha" artifacts/release
[[ $(jq -r .isDraft "$FAKE_GH_STATE/release.json") == false ]]
new_checkout published
before=$(grep -c '^release upload' "$FAKE_GH_STATE/calls")
bash scripts/finalize-release.sh 1.2.3 main "$source_sha" artifacts/release
[[ $(grep -c '^release upload' "$FAKE_GH_STATE/calls") == "$before" ]]
grep -Fq -- '--prerelease=false' "$FAKE_GH_STATE/calls"

# A new prerelease uses the same gates but never marks itself latest stable.
cd "$temp/repo"
git fetch -q origin main
git checkout -q --detach FETCH_HEAD
source_sha=$(git rev-parse HEAD)
git clone -q "$temp/origin.git" "$temp/prerelease"
cd "$temp/prerelease"
mkdir -p artifacts/release
bash scripts/release-changelog.sh 1.3.0-rc.1 CHANGELOG.md > artifacts/release/CHANGELOG.md
echo 'Preview release.' > artifacts/release/release-notes.md
for platform in macos-x64 macos-arm64 windows-x64 linux-x64; do
  cp "$temp/repo/artifacts/release/recon-1.2.3-$platform.tar.gz" "artifacts/release/recon-1.3.0-rc.1-$platform.tar.gz"
done
rm "$FAKE_GH_STATE/release.json"
GITHUB_RUN_ID=456 bash scripts/finalize-release.sh 1.3.0-rc.1 main "$source_sha" artifacts/release
grep -Fq -- '--prerelease --latest=false' "$FAKE_GH_STATE/calls"
echo 'Release script tests passed.'
