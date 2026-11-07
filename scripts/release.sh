#!/bin/sh
# Cut a release:  scripts/release.sh 0.2.0
#
# Sets the version, dates its CHANGELOG.md section, commits, tags v<version>
# and pushes. GitHub Actions then builds, signs and publishes it (see
# .github/workflows/release.yml), and running copies of Timewarp offer it.
set -eu
cd "$(dirname "$0")/.."
version=${1:?usage: scripts/release.sh <version>, e.g. 0.2.0}
tag="v$version"
die() { echo "error: $*" >&2; exit 1; }

echo "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' || die "versions look like 1.2.3"
[ "$(git branch --show-current)" = main ] || die "release from main"
[ -z "$(git status --porcelain)" ] || die "commit or stash your changes first"
[ -s apps/desktop/update-key.pub ] || die "no update key yet: cargo run -p timewarp-release -- keygen"
git fetch --quiet origin
[ "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)" ] || die "main isn't in sync with origin/main"
git rev-parse -q --verify "refs/tags/$tag" >/dev/null && die "$tag already exists"
git ls-remote --exit-code --tags origin "$tag" >/dev/null 2>&1 && die "$tag already exists on GitHub"
grep -q "^## $version\b" CHANGELOG.md || die "add a '## $version' section to CHANGELOG.md first"

# Version and changelog date.
sed -i '' "/^\[workspace.package\]/,/^\[/ s/^version = \".*\"/version = \"$version\"/" Cargo.toml
sed -i '' "s/^## $version (unreleased)/## $version ($(date +%Y-%m-%d))/" CHANGELOG.md
cargo update --workspace --quiet
cargo run --quiet -p timewarp-release -- notes "$version" >/dev/null

git add Cargo.toml Cargo.lock CHANGELOG.md
git diff --cached --quiet || git commit --quiet -m "Timewarp $version"
git tag -a "$tag" -m "Timewarp $version"
git push --atomic origin main "$tag"
echo "Pushed $tag. Follow the build: gh run watch"
