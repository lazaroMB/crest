#!/usr/bin/env bash
# Builds the crest release tarball from an already compiled binary.
#
#   make release && scripts/package.sh
#   cargo build --release --locked --target <t> -p crest
#   scripts/package.sh --target <t>
#
# Produces <out>/crest-<version>-<target>.tar.gz and its checksum
# <file>.sha256 in the `sha256sum -c` format. The tarball holds a directory
# with install.sh (scripts/install.sh), README.txt with the runtime
# requirements, LICENSE* and THIRD-PARTY.md, and:
#
# - Linux: the binary, crest-launcher and share/ with the .desktop entry
#   and the hicolor icons;
# - macOS: crest.app (Contents/MacOS/crest is the binary, which loads the
#   login shell's environment itself; Resources/crest.icns built with
#   iconutil from the hicolor PNGs when iconutil exists). With --sign,
#   scripts/sign-macos.sh signs, notarizes and staples it before the tar.
#
# Compatible with Bash 3.2 (macOS).

set -euo pipefail

BIN=crest
target=""
version=""
out="dist"
sign=0

usage() {
    cat <<'EOF'
Usage: scripts/package.sh [--target <triple>] [--version <v>] [--out <dir>]

  --target   Rust target triple. With it, packages target/<triple>/release/crest;
             without it, target/release/crest and the host triple (rustc -vV).
  --version  Version in the file name (default: the crest package version).
  --out      Output directory (default: dist).
  --sign     macOS: sign and notarize crest.app (scripts/sign-macos.sh; needs
             its APPLE_* variables).
EOF
}

die() {
    echo "package.sh: $*" >&2
    exit 1
}

while [ $# -gt 0 ]; do
    case "$1" in
        --target) [ $# -ge 2 ] || die "--target needs a value"; target="$2"; shift 2 ;;
        --version) [ $# -ge 2 ] || die "--version needs a value"; version="$2"; shift 2 ;;
        --out) [ $# -ge 2 ] || die "--out needs a value"; out="$2"; shift 2 ;;
        --sign) sign=1; shift ;;
        -h | --help) usage; exit 0 ;;
        *) usage >&2; die "unknown argument: $1" ;;
    esac
done

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

# CARGO_TARGET_DIR honors a target/ outside the repo.
target_dir="${CARGO_TARGET_DIR:-target}"
if [ -n "$target" ]; then
    bin_path="$target_dir/$target/release/$BIN"
else
    bin_path="$target_dir/release/$BIN"
    target="$(rustc -vV | sed -n 's/^host: //p')"
    [ -n "$target" ] || die "could not read the host triple from rustc -vV"
fi
[ -x "$bin_path" ] || die "$bin_path does not exist; build it first (make release)"

if [ -z "$version" ]; then
    # `cargo pkgid` prints ...#crest@0.1.0 (or ...#0.1.0 on old cargo versions).
    pkgid="$(cargo pkgid -p "$BIN")"
    version="${pkgid##*[#@]}"
    [ -n "$version" ] || die "could not read the version from $pkgid"
fi

name="$BIN-$version-$target"
mkdir -p "$out"
out="$(cd "$out" && pwd)"
staging="$(mktemp -d "${TMPDIR:-/tmp}/crest-package.XXXXXX")"
trap 'rm -rf "$staging"' EXIT
mkdir "$staging/$name"

cp scripts/install.sh "$staging/$name/install.sh"
chmod 755 "$staging/$name/install.sh"
for license in LICENSE* THIRD-PARTY.md; do
    if [ -f "$license" ]; then
        cp "$license" "$staging/$name/"
    fi
done
# What install.sh copies into the config folder when it is missing there.
mkdir -p "$staging/$name/config/themes"
cp assets/init.lua "$staging/$name/config/init.lua"
cp examples/themes/*.lua "$staging/$name/config/themes/"

# crest.icns from the committed PNGs: each @2x is the next size up, and
# 512@2x is the 512 one scaled with sips.
make_icns() { # <output.icns>
    local set_dir="$staging/crest.iconset" icons=assets/icons/hicolor size double
    mkdir -p "$set_dir"
    for size in 16 32 128 256 512; do
        cp "$icons/${size}x${size}/apps/crest.png" "$set_dir/icon_${size}x${size}.png"
        double=$((size * 2))
        if [ -f "$icons/${double}x${double}/apps/crest.png" ]; then
            cp "$icons/${double}x${double}/apps/crest.png" "$set_dir/icon_${size}x${size}@2x.png"
        else
            sips -z "$double" "$double" "$icons/${size}x${size}/apps/crest.png" \
                --out "$set_dir/icon_${size}x${size}@2x.png" >/dev/null
        fi
    done
    iconutil -c icns "$set_dir" -o "$1"
}

case "$target" in
    *-apple-darwin)
        app="$staging/$name/crest.app/Contents"
        mkdir -p "$app/MacOS" "$app/Resources"
        cp "$bin_path" "$app/MacOS/crest"
        chmod 755 "$app/MacOS/crest"
        sed "s/@VERSION@/$version/g" assets/macos/Info.plist.in >"$app/Info.plist"
        printf 'APPL????' >"$app/PkgInfo"
        if command -v iconutil >/dev/null 2>&1 && command -v sips >/dev/null 2>&1; then
            make_icns "$app/Resources/crest.icns"
        else
            echo "package.sh: iconutil not found; crest.app has no icon" >&2
        fi
        if [ "$sign" = 1 ]; then
            bash scripts/sign-macos.sh "$staging/$name/crest.app"
        fi
        ;;
    *)
        [ "$sign" = 0 ] || die "--sign is only for macOS targets"

        cp "$bin_path" "$staging/$name/$BIN"
        cp assets/crest-launcher.sh "$staging/$name/crest-launcher"
        chmod 755 "$staging/$name/crest-launcher"
        mkdir -p "$staging/$name/share/applications" "$staging/$name/share/icons"
        cp assets/crest.desktop "$staging/$name/share/applications/"
        cp -R assets/icons/hicolor "$staging/$name/share/icons/"
        ;;
esac

case "$target" in
    *-apple-darwin)
        platform_notes="macOS
-----
- ./install.sh copies crest.app to ~/Applications (Launchpad, Spotlight,
  Dock) and links ~/.local/bin/crest to it.
- Release builds are signed with a Developer ID and notarized by Apple.
- rg and fzf: brew install ripgrep fzf"
        ;;
    *)
        platform_notes="Linux
-----
- Vulkan: the loader (libvulkan1 / vulkan-loader) and the GPU driver
  (mesa-vulkan-drivers or the vendor's).
- GPUI libraries: libxkbcommon, libxkbcommon-x11, wayland, libxcb,
  libx11-xcb, fontconfig and freetype (distro packages).
- glibc the same as or newer than the one on the build system; the CI
  tarballs are built on Ubuntu 22.04 (glibc 2.35)."
        ;;
esac

cat >"$staging/$name/README.txt" <<EOF
crest $version ($target)

Installation: run ./install.sh (no sudo). It puts \`crest\` in ~/.local/bin
and the app in the app menu (Linux) or ~/Applications (macOS);
./install.sh --uninstall removes it. --prefix <dir> changes ~/.local.
It also copies config/ (a commented init.lua and the example color themes)
into ~/.config/crest, skipping every file that is already there.

Started from the app menu, the Dock or Launchpad, crest gets the PATH of
your login shell (~/.zprofile, ~/.profile, ~/.bash_profile), not the one of
~/.zshrc or ~/.bashrc. Keep ANTHROPIC_API_KEY in the system keyring, or
export it in one of those files.

Runtime requirements
--------------------
- ripgrep (rg) 13.0 or newer.
- fzf 0.36 or newer.
- git.
SQLite and TLS (rustls) are built into the binary: no system libsqlite3 or
OpenSSL is needed.

$platform_notes

From the repository, scripts/install-deps.sh installs or checks (--check)
these dependencies.
EOF

tarball="$name.tar.gz"
tar -C "$staging" -czf "$out/$tarball" "$name"

if command -v sha256sum >/dev/null 2>&1; then
    (cd "$out" && sha256sum "$tarball" >"$tarball.sha256")
else
    (cd "$out" && shasum -a 256 "$tarball" >"$tarball.sha256")
fi

echo "$out/$tarball"
