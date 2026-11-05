#!/bin/sh
# Builds Timewarp and packages it the way the in-app updater expects:
#   dist/timewarp-<version>-macos-<arch>.tar.gz  (+ .sha256)
# Attach both files to a GitHub release tagged v<version>.
set -eu
cd "$(dirname "$0")/.."
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
arch=$(uname -m | sed 's/arm64/aarch64/')
name="timewarp-$version-macos-$arch.tar.gz"

cargo build --release -p timewarp-desktop
mkdir -p dist
tar -czf "dist/$name" -C target/release Timewarp
(cd dist && shasum -a 256 "$name" > "$name.sha256")
echo "dist/$name"
echo "dist/$name.sha256"
