#!/bin/sh
# Installs (or updates) Timewarp from the latest GitHub release:
#   curl -fsSL https://raw.githubusercontent.com/0xInception/timewarp/main/scripts/install.sh | sh
#
# Files downloaded with curl aren't quarantined, so macOS doesn't block the
# app. The download is checked against the SHA-256 in the release manifest.
set -eu
repo=0xInception/timewarp
base="https://github.com/$repo/releases/latest/download"
die() { echo "error: $*" >&2; exit 1; }

[ "$(uname -s)" = Darwin ] || die "Timewarp is for macOS."
major=$(sw_vers -productVersion | cut -d. -f1)
[ "$major" -ge 15 ] || die "Timewarp needs macOS 15 or later."
arch=$(uname -m | sed 's/arm64/aarch64/')

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
curl -fsSL "$base/latest.json" -o "$tmp/latest.json" || die "couldn't reach GitHub."
version=$(plutil -extract version raw -o - "$tmp/latest.json")
name="Timewarp-$version-macos-$arch.tar.gz"
sha=""
i=0
while n=$(plutil -extract "assets.$i.name" raw -o - "$tmp/latest.json" 2>/dev/null); do
    [ "$n" = "$name" ] && sha=$(plutil -extract "assets.$i.sha256" raw -o - "$tmp/latest.json")
    i=$((i + 1))
done
[ -n "$sha" ] || die "Timewarp $version has no download for this Mac ($arch)."

echo "Downloading Timewarp $version..."
curl -fL --progress-bar "$base/$name" -o "$tmp/$name"
[ "$(shasum -a 256 "$tmp/$name" | cut -d' ' -f1)" = "$sha" ] || die "the download is damaged. Nothing was installed."
tar -xzf "$tmp/$name" -C "$tmp"

dest=/Applications
[ -w "$dest" ] || dest="$HOME/Applications"
mkdir -p "$dest"
# Quit a running copy the safe way (it finishes saving any recording).
if pgrep -xq Timewarp; then
    echo "Quitting Timewarp..."
    osascript -e 'quit app "Timewarp"' >/dev/null 2>&1 || true
    while pgrep -xq Timewarp; do sleep 1; done
fi
rm -rf "$dest/Timewarp.app"
mv "$tmp/Timewarp.app" "$dest/"
xattr -dr com.apple.quarantine "$dest/Timewarp.app" 2>/dev/null || true
echo "Installed $dest/Timewarp.app"
open "$dest/Timewarp.app"
