#!/usr/bin/env bash
# Tests for scripts/install.sh and the app layout of scripts/package.sh.
# Usage: bash tests/install/run.sh   (also with Bash 3.2)
#
# A fake crest (a shell script) is packaged for Linux and for macOS and
# installed into a temporary prefix, so both layouts are checked on any
# host: the uname that install.sh sees is faked.

set -u

HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
INSTALL="$ROOT/scripts/install.sh"

FAILS=0
PASSES=0

check() { # description, then the command that must succeed
    local desc=$1
    shift
    if "$@"; then
        PASSES=$((PASSES + 1))
    else
        FAILS=$((FAILS + 1))
        echo "FAIL: $desc" >&2
    fi
}

eq() { [ "$1" = "$2" ] || { echo "  expected '$2', got '$1'" >&2; return 1; }; }
contains() { case "$1" in *"$2"*) return 0 ;; esac; echo "  '$1' lacks '$2'" >&2; return 1; }
not() { ! "$@"; }

TMP=$(mktemp -d "${TMPDIR:-/tmp}/crest-install-test.XXXXXX")
trap 'rm -rf "$TMP"' EXIT

# The fake crest prints its arguments and a variable only the fake login
# shell sets.
export CARGO_TARGET_DIR="$TMP/target"
for target in x86_64-unknown-linux-gnu aarch64-apple-darwin; do
    mkdir -p "$CARGO_TARGET_DIR/$target/release"
    printf '#!/bin/sh\necho "args:$*"\necho "marker:${CREST_TEST_MARKER:-none}"\n' \
        >"$CARGO_TARGET_DIR/$target/release/crest"
    chmod 755 "$CARGO_TARGET_DIR/$target/release/crest"
done

# A login shell named zsh that exports the marker, as ~/.zprofile would.
mkdir -p "$TMP/shell"
cat >"$TMP/shell/zsh" <<'EOF'
#!/bin/sh
[ "$1" = -l ] || exit 90
shift
[ "$1" = -c ] || exit 91
shift
CREST_TEST_MARKER=login
export CREST_TEST_MARKER
exec /bin/sh -c "$@"
EOF
chmod 755 "$TMP/shell/zsh"

package() { # target -> prints the tarball path
    bash "$ROOT/scripts/package.sh" --target "$1" --version 0.0.0 --out "$TMP/dist" 2>/dev/null | tail -n 1
}

# --- Linux -----------------------------------------------------------------
linux_tar=$(package x86_64-unknown-linux-gnu)
check "linux tarball" test -f "$linux_tar"
listing=$(tar -tzf "$linux_tar")
for f in install.sh crest crest-launcher README.txt share/applications/crest.desktop \
    share/icons/hicolor/scalable/apps/crest.svg; do
    check "linux tarball has $f" contains "$listing" "crest-0.0.0-x86_64-unknown-linux-gnu/$f"
done

P="$TMP/linux/.local"
linux() { HOME="$TMP/linux" CREST_INSTALL_UNAME_S=Linux CREST_INSTALL_UNAME_M=x86_64 sh "$INSTALL" "$@"; }
check "linux install" linux --tarball "$linux_tar" --prefix "$P" >/dev/null 2>&1
check "binary" test -x "$P/bin/crest"
check "launcher" test -x "$P/lib/crest/crest-launcher"
desktop="$P/share/applications/crest.desktop"
check "desktop entry" test -f "$desktop"
check "absolute Exec" eq "$(sed -n 's/^Exec=//p' "$desktop")" "\"$P/lib/crest/crest-launcher\""
check "absolute Icon" eq "$(sed -n 's/^Icon=//p' "$desktop")" "$P/share/icons/hicolor/scalable/apps/crest.svg"
check "icons" test -f "$P/share/icons/hicolor/256x256/apps/crest.png"
if command -v desktop-file-validate >/dev/null 2>&1; then
    check "desktop-file-validate" desktop-file-validate "$desktop"
fi
out=$(SHELL="$TMP/shell/zsh" "$P/lib/crest/crest-launcher" ~/src "two words")
check "launcher passes the arguments" contains "$out" "args:$HOME/src two words"
check "launcher uses the login shell" contains "$out" "marker:login"
out=$(SHELL=/usr/bin/fish "$P/lib/crest/crest-launcher" x)
check "non-POSIX shells fall back to sh" contains "$out" "args:x"

# Running the script of an extracted tarball installs that tarball.
mkdir -p "$TMP/extract"
tar -xzf "$linux_tar" -C "$TMP/extract"
rm -rf "$P"
HOME="$TMP/linux" CREST_INSTALL_UNAME_S=Linux CREST_INSTALL_UNAME_M=x86_64 \
    sh "$TMP/extract/crest-0.0.0-x86_64-unknown-linux-gnu/install.sh" --prefix "$P" >/dev/null 2>&1
check "extracted install" test -x "$P/bin/crest"

check "linux uninstall" linux --uninstall --prefix "$P" >/dev/null 2>&1
check "no binary left" not test -e "$P/bin/crest"
check "no launcher left" not test -e "$P/lib/crest"
check "no desktop entry left" not test -e "$desktop"
check "no icons left" eq "$(find "$P/share/icons" -name 'crest.*' | wc -l | tr -d ' ')" 0

# --- macOS -----------------------------------------------------------------
mac_tar=$(package aarch64-apple-darwin)
listing=$(tar -tzf "$mac_tar")
for f in install.sh crest.app/Contents/Info.plist crest.app/Contents/MacOS/crest \
    crest.app/Contents/MacOS/crest-bin; do
    check "mac tarball has $f" contains "$listing" "crest-0.0.0-aarch64-apple-darwin/$f"
done
check "mac tarball has no loose binary" not grep -qx "crest-0.0.0-aarch64-apple-darwin/crest" <<<"$listing"

M="$TMP/mac/.local"
APPS="$TMP/mac/Applications"
mac() {
    HOME="$TMP/mac" CREST_INSTALL_UNAME_S=Darwin CREST_INSTALL_UNAME_M=arm64 \
        CREST_INSTALL_APPS_DIR="$APPS" sh "$INSTALL" "$@"
}
check "mac install" mac --tarball "$mac_tar" --prefix "$M" >/dev/null 2>&1
app="$APPS/crest.app/Contents"
check "app bundle" test -x "$app/MacOS/crest"
plist="$app/Info.plist"
check "plist version" contains "$(cat "$plist")" "<string>0.0.0</string>"
check "plist executable" contains "$(cat "$plist")" "<key>CFBundleExecutable</key>"
if command -v plutil >/dev/null 2>&1; then
    check "plutil -lint" plutil -lint "$plist" >/dev/null
fi
if command -v iconutil >/dev/null 2>&1; then
    check "icns" test -s "$app/Resources/crest.icns"
fi
check "command links into the app" eq "$(readlink "$M/bin/crest")" "$app/MacOS/crest-bin"
out=$(SHELL="$TMP/shell/zsh" "$app/MacOS/crest" --pick)
check "app launcher passes the arguments" contains "$out" "args:--pick"
check "app launcher uses the login shell" contains "$out" "marker:login"
check "mac uninstall" mac --uninstall --prefix "$M" >/dev/null 2>&1
check "no app left" not test -e "$APPS/crest.app"
check "no link left" not test -L "$M/bin/crest"

# --- Platforms -------------------------------------------------------------
check "linux arm64 is refused" not env CREST_INSTALL_UNAME_S=Linux CREST_INSTALL_UNAME_M=aarch64 \
    sh "$INSTALL" --tarball /nonexistent >/dev/null 2>&1
err=$(CREST_INSTALL_UNAME_S=FreeBSD CREST_INSTALL_UNAME_M=amd64 sh "$INSTALL" 2>&1)
check "unsupported platforms say so" contains "$err" "Linux x86_64 and macOS only"
check "a missing tarball is an error" not env CREST_INSTALL_UNAME_S=Linux CREST_INSTALL_UNAME_M=x86_64 \
    HOME="$TMP/linux" sh "$INSTALL" --tarball "$TMP/none.tar.gz" --prefix "$P" >/dev/null 2>&1

echo "install: $PASSES passed, $FAILS failed"
[ "$FAILS" -eq 0 ]
