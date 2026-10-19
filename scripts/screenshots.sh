#!/usr/bin/env bash
# Takes the README's screenshots (assets/screenshots/*.png) from the UI
# examples, which run on scripted fakes (no API key).
#
#   scripts/screenshots.sh [scene ...]      # default: every scene
#
# Each example opens a window on the current X display (on Wayland, through
# XWayland: WAYLAND_DISPLAY is unset for it), the script waits until the
# scene is on screen and captures the window with ImageMagick's `import`.
# The windows show up on your screen for a few seconds.
#
# Needs: cargo, ImageMagick (`import`, `convert`), `xprop`, an X display
# (DISPLAY, and XAUTHORITY; under GNOME's Wayland session the XWayland
# cookie in /run/user/$UID is found by itself).

set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
out="$root/assets/screenshots"
logs="$(mktemp -d "${TMPDIR:-/tmp}/crest-shots.XXXXXX")"
width=1600
mkdir -p "$out"

export DISPLAY="${DISPLAY:-:0}"
if [ -z "${XAUTHORITY:-}" ]; then
    for cookie in /run/user/"$(id -u)"/.mutter-Xwaylandauth.*; do
        [ -f "$cookie" ] && export XAUTHORITY="$cookie"
    done
fi
magick="convert"
command -v magick >/dev/null 2>&1 && magick="magick"
for tool in import "$magick" xprop; do
    command -v "$tool" >/dev/null 2>&1 || {
        echo "screenshots.sh: $tool is needed (ImageMagick / x11-utils)" >&2
        exit 1
    }
done

# scene name -> "example [args...]"
declare -A scenes=(
    [hero]="showcase --scene hero"
    [research]="showcase --scene research"
    [approval]="showcase --scene approval"
    [help]="showcase --scene help"
    [model3d]="showcase --scene model"
    [modal]="showcase --scene modal"
    [blocks]="showcase --scene blocks"
    [routing]="showcase --scene routing"
    [board]="sysboard"
)
# Crop of the capture, for examples whose window has placeholders around
# what is shown (ImageMagick geometry).
declare -A crop=(
    [board]="1240x760+20+20"
    [blocks]="1280x575+0+0"
    [routing]="1280x690+0+0"
)
# What each example prints once it is worth capturing (empty: a delay).
declare -A ready=(
    [showcase]="showcase: ready"
)
order=(hero research approval help model3d modal blocks routing board)
[ "$#" -gt 0 ] && order=("$@")

cd "$root"
cargo build -q -p ui --example showcase --example sysboard

# The id of the newest window of class "crest".
crest_window() {
    local id
    for id in $(xprop -root _NET_CLIENT_LIST 2>/dev/null | grep -o '0x[0-9a-f]*' | tac); do
        if xprop -id "$id" WM_CLASS 2>/dev/null | grep -q '"crest"'; then
            echo "$id"
            return 0
        fi
    done
    return 1
}

for scene in "${order[@]}"; do
    spec="${scenes[$scene]:-}"
    [ -n "$spec" ] || { echo "unknown scene: $scene" >&2; exit 1; }
    read -r example args <<<"$spec"
    log="$logs/$scene.log"
    # shellcheck disable=SC2086 # args are words
    env -u WAYLAND_DISPLAY "target/debug/examples/$example" $args >"$log" 2>&1 &
    pid=$!
    marker="${ready[$example]:-}"
    if [ -n "$marker" ]; then
        for _ in $(seq 1 120); do
            grep -q "$marker" "$log" && break
            sleep 0.25
        done
        sleep 3
    else
        sleep 8
    fi
    if id="$(crest_window)"; then
        import -window "$id" "$logs/$scene.png"
        geometry="${crop[$scene]:-}"
        "$magick" "$logs/$scene.png" ${geometry:+-crop "$geometry" +repage} \
            -strip -resize "${width}x>" "$out/$scene.png"
        echo "$scene -> assets/screenshots/$scene.png"
    else
        echo "$scene: no crest window found (see $log)" >&2
    fi
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
done
