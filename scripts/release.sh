#!/usr/bin/env bash
#
# Prepare a release in one command:
#
#   scripts/release.sh 0.9.0            # bump, CHANGELOG.md, all CI checks, commit, tag
#   scripts/release.sh 0.9.0 --quick    # same, but skip the checks (CI still runs them)
#
# It never pushes. At the end it prints the one command that publishes the
# release, so pushing stays a deliberate step.

set -euo pipefail

PROJECT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$PROJECT_DIR"

fail() {
  echo "release: $*" >&2
  exit 1
}

version="${1:-}"
version="${version#v}"
quick=false
[[ "${2:-}" == "--quick" ]] && quick=true

[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] ||
  fail "give the new version, for example: scripts/release.sh 0.9.0"
tag="v$version"

for command_name in git git-cliff cargo npm; do
  command -v "$command_name" >/dev/null 2>&1 || {
    [[ "$command_name" == "git-cliff" ]] && fail "git-cliff is missing. Install it with: brew install git-cliff"
    fail "$command_name is missing"
  }
done

[[ "$(git branch --show-current)" == "main" ]] || fail "switch to main first"
[[ -z "$(git status --porcelain)" ]] || fail "commit or stash your changes first"
git rev-parse -q --verify "refs/tags/$tag" >/dev/null && fail "$tag already exists"

current="$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1)"
[[ "$(printf '%s\n%s\n' "$current" "$version" | sort -V | tail -1)" == "$version" &&
  "$current" != "$version" ]] || fail "$version is not newer than $current"

echo "Releasing $current → $version"

# 1. The version, in every file that carries it.
# awk, not sed: macOS and Linux sed disagree on "first match only" and -i.
awk -v v="$version" '
  !done && /^version = / { print "version = \"" v "\""; done = 1; next }
  { print }
' Cargo.toml >Cargo.toml.new && mv Cargo.toml.new Cargo.toml
# The magpie-fi entry in Cargo.lock is the version line right after its name.
awk -v v="$version" '
  found && /^version = / { print "version = \"" v "\""; found = 0; next }
  { print }
  /^name = "magpie-fi"$/ { found = 1 }
' Cargo.lock >Cargo.lock.new && mv Cargo.lock.new Cargo.lock
sed -i.bak "s/MAGPIE_TAG=$current/MAGPIE_TAG=$version/" .env.example
rm -f .env.example.bak
(cd web && npm version "$version" --no-git-tag-version --allow-same-version >/dev/null)
cargo metadata --locked --format-version 1 >/dev/null ||
  fail "Cargo.lock did not take the new version"

# 2. The changelog, from the commit subjects since the last release.
notes="$(git cliff --unreleased --tag "$tag" --strip all 2>/dev/null)"
[[ -n "$(printf '%s' "$notes" | grep '^- ')" ]] || fail "no changes since the last release"
git cliff --tag "$tag" -o CHANGELOG.md 2>/dev/null
# Trailing blank lines would make every release touch the end of the file.
printf '%s\n' "$(cat CHANGELOG.md)" >CHANGELOG.md

echo
echo "$notes"
echo

# 3. The same checks CI runs, so a broken release never gets a tag.
if [[ "$quick" == false ]]; then
  echo "Running the checks (add --quick to skip)…"
  cargo fmt --all -- --check
  cargo clippy --all-targets --locked
  cargo test --locked
  (cd web && npm ci && npm run format:check && npm run lint && npm run build)
fi

# 4. Commit and tag locally.
git add Cargo.toml Cargo.lock web/package.json web/package-lock.json .env.example CHANGELOG.md
git commit -q -m "chore: release $version"
git tag -a "$tag" -m "magpie-fi $version"

echo
echo "Ready: commit and tag $tag are made here, nothing is pushed yet."
echo "Publish it (builds the image and the GitHub release) with:"
echo
echo "  git push --atomic origin main $tag"
echo
echo "Undo it instead with:  git tag -d $tag && git reset --hard HEAD~1"
