#!/bin/sh
# Builds Timewarp.app for this Mac's CPU and packages it for the updater:
#   dist/Timewarp-<version>-macos-<arch>.tar.gz
#
# Signing (all optional, set by the release workflow from GitHub secrets):
#   MACOS_SIGNING_IDENTITY   e.g. "Developer ID Application: Name (TEAMID)".
#                            Without it the app is signed ad hoc.
#   APPLE_ID, APPLE_TEAM_ID, APPLE_APP_PASSWORD
#                            Notarize too (needs a Developer ID identity).
set -eu
cd "$(dirname "$0")/.."
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
arch=$(uname -m | sed 's/arm64/aarch64/')

cargo build --release --locked -p timewarp-desktop

app=dist/Timewarp.app
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp target/release/Timewarp "$app/Contents/MacOS/Timewarp"
cp apps/desktop/assets/AppIcon.icns "$app/Contents/Resources/AppIcon.icns"
sed "s/@VERSION@/$version/g" apps/desktop/Info.plist > "$app/Contents/Info.plist"
printf 'APPL????' > "$app/Contents/PkgInfo"

if [ -n "${MACOS_SIGNING_IDENTITY:-}" ]; then
    codesign --force --options runtime --timestamp --sign "$MACOS_SIGNING_IDENTITY" "$app"
else
    # Ad hoc: fine to run, but macOS asks for Screen Recording again after
    # each update because the signature changes. A Developer ID fixes that.
    codesign --force --sign - "$app"
fi
codesign --verify --strict "$app"

if [ -n "${APPLE_ID:-}" ] && [ -n "${MACOS_SIGNING_IDENTITY:-}" ]; then
    ditto -c -k --keepParent "$app" dist/notarize.zip
    xcrun notarytool submit dist/notarize.zip --apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" \
        --password "$APPLE_APP_PASSWORD" --wait
    xcrun stapler staple "$app"
    rm dist/notarize.zip
fi

name="Timewarp-$version-macos-$arch.tar.gz"
# No AppleDouble (._*) files in the archive.
COPYFILE_DISABLE=1 tar -czf "dist/$name" -C dist Timewarp.app
echo "dist/$name"
