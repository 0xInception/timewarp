#!/bin/sh
# Rebuilds apps/desktop/assets/AppIcon.icns from icon.svg (needs rsvg-convert).
set -eu
cd "$(dirname "$0")/../apps/desktop/assets"
set=$(mktemp -d)/AppIcon.iconset
mkdir -p "$set"
for s in 16 32 128 256 512; do
    rsvg-convert -w $s -h $s icon.svg -o "$set/icon_${s}x${s}.png"
    rsvg-convert -w $((s * 2)) -h $((s * 2)) icon.svg -o "$set/icon_${s}x${s}@2x.png"
done
iconutil -c icns "$set" -o AppIcon.icns
echo "apps/desktop/assets/AppIcon.icns"
