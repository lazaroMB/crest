#!/usr/bin/env bash
# Takes the README's screenshots (assets/screenshots/*.png) from the UI
# examples, which run on scripted fakes (no API key).
#
#   scripts/screenshots.sh [scene ...]      # default: every scene
#
# Each example opens a window, the script waits until the scene is on
# screen (the example prints a `ready` line with its viewport size and
# scale) and captures the window: on Linux with ImageMagick's `import` on
# the X display (on Wayland, through XWayland: WAYLAND_DISPLAY is unset for
# it); on macOS with `screencapture -l <window id>`, the id found by
# scripts/macos/window-id.swift from the example's pid. The windows show up
# on your screen for a few seconds.
#
# Needs: cargo, ImageMagick (`magick` or `convert`); on Linux `import`,
# `xprop` and an X display (DISPLAY, and XAUTHORITY; under GNOME's Wayland
# session the XWayland cookie in /run/user/$UID is found by itself); on
# macOS `swiftc` (the Xcode command line tools) and, the first time, the
# Screen Recording permission for your terminal (a blank capture means it
# was just granted: run again). Install JetBrains Mono first, or the text
# measures differently from the other screenshots.

set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
out="$root/assets/screenshots"
logs="$(mktemp -d "${TMPDIR:-/tmp}/crest-shots.XXXXXX")"
# Widest image written: twice the README's 900 px, so retina captures stay
# crisp and 1x captures pass through untouched.
width=1800
mkdir -p "$out"

os="$(uname -s)"
magick="convert"
command -v magick >/dev/null 2>&1 && magick="magick"
need() {
    command -v "$1" >/dev/null 2>&1 || {
        echo "screenshots.sh: $1 is needed ($2)" >&2
        exit 1
    }
}
need "$magick" "ImageMagick"
case "$os" in
    Darwin)
        need screencapture "macOS"
        need swiftc "the Xcode command line tools"
        swiftc -O -o "$logs/window-id" "$root/scripts/macos/window-id.swift"
        ;;
    *)
        export DISPLAY="${DISPLAY:-:0}"
        if [ -z "${XAUTHORITY:-}" ]; then
            for cookie in /run/user/"$(id -u)"/.mutter-Xwaylandauth.*; do
                [ -f "$cookie" ] && export XAUTHORITY="$cookie"
            done
        fi
        need import "ImageMagick"
        need xprop "x11-utils"
        ;;
esac

# scene name -> "example [args...]"
declare -A scenes=(
    [hero]="showcase --scene hero"
    [sidebar]="showcase --scene sidebar"
    [tour]="showcase --scene tour"
    [menu]="showcase --scene menu"
    [info]="showcase --scene info"
    [picker]="showcase --scene picker"
    [permissions]="showcase --scene permissions"
    [research]="showcase --scene research"
    [approval]="showcase --scene approval"
    [help]="showcase --scene help"
    [model3d]="showcase --scene model"
    [modal]="showcase --scene modal"
    [blocks]="showcase --scene blocks"
    [routing]="showcase --scene routing"
    [technical]="showcase --scene technical"
    [board]="sysboard"
)
# Crop of the capture, in logical pixels of the window (ImageMagick
# geometry, scaled by the window's scale factor), for scenes whose point is
# one part of the window.
declare -A crop=(
    [board]="1240x760+20+20"
    [blocks]="1280x870+0+0"
    [routing]="1280x690+0+0"
    [technical]="1280x900+0+0"
    [menu]="1280x520+0+440"
    [info]="640x420+640+540"
)
order=(hero sidebar tour menu info picker permissions research approval help model3d modal blocks routing technical board)
[ "$#" -gt 0 ] && order=("$@")

cd "$root"
cargo build -q -p ui --features examples --example showcase --example sysboard

# The id of the window of the example (pid $1): the newest window of class
# "crest" on X11, the process's first normal window on macOS.
example_window() {
    local pid="$1" id
    case "$os" in
        Darwin) "$logs/window-id" "$pid" 2>/dev/null ;;
        *)
            for id in $(xprop -root _NET_CLIENT_LIST 2>/dev/null | grep -o '0x[0-9a-f]*' | sed '1!G;h;$!d'); do
                if xprop -id "$id" WM_CLASS 2>/dev/null | grep -q '"crest"'; then
                    echo "$id"
                    return 0
                fi
            done
            return 1
            ;;
    esac
}

# Scales an ImageMagick geometry "WxH+X+Y" by $2 (an integer scale factor).
scaled() {
    local geometry="$1" s="$2"
    echo "$geometry" | awk -v s="$s" -F'[x+]' '{ printf "%dx%d+%d+%d", $1*s, $2*s, $3*s, $4*s }'
}

for scene in "${order[@]}"; do
    spec="${scenes[$scene]:-}"
    [ -n "$spec" ] || { echo "unknown scene: $scene" >&2; exit 1; }
    read -r example args <<<"$spec"
    log="$logs/$scene.log"
    if [ "$os" = Darwin ]; then
        # shellcheck disable=SC2086 # args are words
        "target/debug/examples/$example" $args >"$log" 2>&1 &
    else
        # shellcheck disable=SC2086 # args are words
        env -u WAYLAND_DISPLAY "target/debug/examples/$example" $args >"$log" 2>&1 &
    fi
    pid=$!
    # `<example>: ready <w>x<h> scale <s> ...`, once the scene is on screen.
    ready=""
    for _ in $(seq 1 240); do
        ready="$(grep -m1 "^$example: ready" "$log" 2>/dev/null || true)"
        [ -n "$ready" ] && break
        sleep 0.25
    done
    if [ -z "$ready" ]; then
        echo "$scene: the example never got ready (see $log)" >&2
        kill "$pid" 2>/dev/null || true
        continue
    fi
    sleep 3
    read -r _ _ dims _ scale _ <<<"$ready"
    w="${dims%x*}"
    h="${dims#*x}"
    s="${scale%.*}"
    [ -n "$s" ] || s=1
    id=""
    for _ in $(seq 1 20); do
        id="$(example_window "$pid" || true)"
        [ -n "$id" ] && break
        sleep 0.25
    done
    if [ -n "$id" ]; then
        case "$os" in
            Darwin) screencapture -x -o -l "$id" "$logs/$scene.png" ;;
            *) import -window "$id" "$logs/$scene.png" ;;
        esac
        # The capture holds the whole window: keep the viewport (the bottom
        # w×h, which drops the title bar), then the scene's own crop.
        geometry="${crop[$scene]:-}"
        "$magick" "$logs/$scene.png" \
            -gravity South -crop "$((w * s))x$((h * s))+0+0" +repage -gravity NorthWest \
            ${geometry:+-crop "$(scaled "$geometry" "$s")" +repage} \
            -strip -resize "${width}x>" "$out/$scene.png"
        echo "$scene -> assets/screenshots/$scene.png"
    else
        echo "$scene: no window found for the example (see $log)" >&2
    fi
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
done
