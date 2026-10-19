#!/usr/bin/env bash
# Renders assets/logo.svg (the TwoCrest mark) into the app icons:
#
#   assets/icons/hicolor/<n>x<n>/apps/crest.png   (Linux icon theme layout)
#   assets/icons/crest.icns                        (macOS, when iconutil exists)
#
# Needs rsvg-convert (librsvg). The PNGs are committed, so this only runs
# when the logo changes.
#
# Compatible with Bash 3.2 (macOS).

set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
svg="$root/assets/logo.svg"
out="$root/assets/icons"

command -v rsvg-convert >/dev/null 2>&1 || {
    echo "icons.sh: rsvg-convert is needed (librsvg2-bin / librsvg)" >&2
    exit 1
}

for size in 16 24 32 48 64 128 256 512; do
    dir="$out/hicolor/${size}x${size}/apps"
    mkdir -p "$dir"
    rsvg-convert -w "$size" -h "$size" "$svg" -o "$dir/crest.png"
done
mkdir -p "$out/hicolor/scalable/apps"
cp "$svg" "$out/hicolor/scalable/apps/crest.svg"

if command -v iconutil >/dev/null 2>&1; then
    set_dir="$(mktemp -d "${TMPDIR:-/tmp}/crest-icons.XXXXXX")/crest.iconset"
    mkdir -p "$set_dir"
    for size in 16 32 128 256 512; do
        rsvg-convert -w "$size" -h "$size" "$svg" -o "$set_dir/icon_${size}x${size}.png"
        double=$((size * 2))
        rsvg-convert -w "$double" -h "$double" "$svg" -o "$set_dir/icon_${size}x${size}@2x.png"
    done
    iconutil -c icns "$set_dir" -o "$out/crest.icns"
fi

echo "icons written to $out"
