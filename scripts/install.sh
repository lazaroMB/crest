#!/bin/sh
# Installs crest as an app: in the app menu on Linux, in ~/Applications on
# macOS (Launchpad, Spotlight, Dock), and as the `crest` command. No sudo.
#
#   curl -fsSL https://raw.githubusercontent.com/lazaroMB/crest/main/scripts/install.sh | sh
#   curl -fsSL .../install.sh | sh -s -- --version v0.8.0
#   ./install.sh                 # from an extracted release tarball
#   ./install.sh --uninstall
#
# Options:
#   --version <vX.Y.Z>  release to install (default: the latest)
#   --tarball <file>    install a local crest-<version>-<target>.tar.gz
#   --prefix <dir>      where bin/, lib/ and share/ go (default: ~/.local)
#   --uninstall         remove what this script installs
#
# Linux: PREFIX/bin/crest, PREFIX/lib/crest/crest-launcher, the .desktop
# entry in PREFIX/share/applications (with absolute paths, so it works
# even when PREFIX/bin is not in the desktop session's PATH) and the
# hicolor icons. macOS: ~/Applications/crest.app and PREFIX/bin/crest
# linked to its binary.
#
# POSIX sh (it runs through `curl | sh`); tested with dash and Bash 3.2.

set -eu

REPO="lazaroMB/crest"
APP_NAME="crest"

version=""
tarball=""
prefix="${HOME}/.local"
uninstall=0

say() { printf '%s\n' "$*"; }
warn() { printf 'warning: %s\n' "$*" >&2; }
die() {
    printf 'install.sh: %s\n' "$*" >&2
    exit 1
}

usage() {
    sed -n '2,/^# POSIX/p' "$0" 2>/dev/null | sed 's/^# \{0,1\}//' || true
}

while [ $# -gt 0 ]; do
    case "$1" in
        --version) [ $# -ge 2 ] || die "--version needs a value"; version="$2"; shift 2 ;;
        --tarball) [ $# -ge 2 ] || die "--tarball needs a value"; tarball="$2"; shift 2 ;;
        --prefix) [ $# -ge 2 ] || die "--prefix needs a value"; prefix="$2"; shift 2 ;;
        --uninstall) uninstall=1; shift ;;
        -h | --help) usage; exit 0 ;;
        *) die "unknown argument: $1 (see --help)" ;;
    esac
done

# The Rust target triple of this machine, as the release tarballs name it.
# CREST_INSTALL_UNAME_S / _M fake uname in the tests.
detect_target() {
    os="${CREST_INSTALL_UNAME_S:-$(uname -s)}"
    arch="${CREST_INSTALL_UNAME_M:-$(uname -m)}"
    case "$os/$arch" in
        Linux/x86_64 | Linux/amd64) echo x86_64-unknown-linux-gnu ;;
        Linux/aarch64 | Linux/arm64) echo aarch64-unknown-linux-gnu ;;
        Darwin/arm64 | Darwin/aarch64) echo aarch64-apple-darwin ;;
        Darwin/x86_64) echo x86_64-apple-darwin ;;
        *) return 1 ;;
    esac
}

apps_dir() { echo "${CREST_INSTALL_APPS_DIR:-$HOME/Applications}"; }

refresh_linux_caches() {
    if command -v update-desktop-database >/dev/null 2>&1; then
        update-desktop-database -q "$prefix/share/applications" 2>/dev/null || true
    fi
    if command -v gtk-update-icon-cache >/dev/null 2>&1; then
        gtk-update-icon-cache -q -t "$prefix/share/icons/hicolor" 2>/dev/null || true
    fi
}

lsregister() {
    tool=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
    if [ -x "$tool" ]; then
        "$tool" "$@" >/dev/null 2>&1 || true
    fi
}

do_uninstall() {
    case "$1" in
        *-apple-darwin)
            app="$(apps_dir)/$APP_NAME.app"
            if [ -d "$app" ]; then
                lsregister -u "$app"
                rm -rf "$app"
            fi
            ;;
        *)
            rm -f "$prefix/share/applications/crest.desktop"
            rm -rf "$prefix/lib/crest"
            find "$prefix/share/icons/hicolor" -path '*/apps/crest.*' -exec rm -f {} + 2>/dev/null || true
            refresh_linux_caches
            ;;
    esac
    rm -f "$prefix/bin/crest"
    say "crest uninstalled (your settings and sessions in ~/.config/crest and the data directory stay)"
}

fetch() { # url, output file
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL --proto '=https' --tlsv1.2 -o "$2" "$1"
    elif command -v wget >/dev/null 2>&1; then
        wget -q -O "$2" "$1"
    else
        die "curl or wget is needed to download crest"
    fi
}

latest_version() {
    url="https://github.com/$REPO/releases/latest"
    if command -v curl >/dev/null 2>&1; then
        effective=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "$url")
    else
        effective=$(wget -q -S --spider "$url" 2>&1 | sed -n 's/^ *[Ll]ocation: *//p' | tail -n 1)
    fi
    tag=${effective##*/tag/}
    tag=$(printf '%s' "$tag" | tr -d '\r')
    case "$tag" in
        v[0-9]*) echo "$tag" ;;
        *) die "could not find the latest release of $REPO" ;;
    esac
}

sha256_check() { # dir with <file> and <file>.sha256, file
    if command -v sha256sum >/dev/null 2>&1; then
        (cd "$1" && sha256sum -c "$2.sha256" >/dev/null)
    elif command -v shasum >/dev/null 2>&1; then
        (cd "$1" && shasum -a 256 -c "$2.sha256" >/dev/null)
    else
        warn "no sha256sum or shasum: the download is not verified"
    fi
}

install_linux() { # extracted dir
    src="$1"
    # Checked before copying anything, so a bad tarball leaves no half install.
    [ -x "$src/crest" ] || die "the tarball has no crest binary for Linux"
    for f in crest-launcher share/applications/crest.desktop share/icons/hicolor; do
        [ -e "$src/$f" ] || die "the tarball has no $f: releases before v0.8.0 cannot be installed with this script"
    done
    mkdir -p "$prefix/bin" "$prefix/lib/crest" "$prefix/share/applications" "$prefix/share/icons"
    # Through a temporary name, so a running crest is not overwritten in place.
    cp "$src/crest" "$prefix/bin/.crest.new"
    chmod 755 "$prefix/bin/.crest.new"
    mv -f "$prefix/bin/.crest.new" "$prefix/bin/crest"
    cp "$src/crest-launcher" "$prefix/lib/crest/crest-launcher"
    chmod 755 "$prefix/lib/crest/crest-launcher"
    cp -R "$src/share/icons/hicolor" "$prefix/share/icons/"
    launcher="$prefix/lib/crest/crest-launcher"
    icon="$prefix/share/icons/hicolor/scalable/apps/crest.svg"
    [ -f "$icon" ] || icon=crest
    sed -e "s|^Exec=.*|Exec=\"$launcher\"|" -e "s|^Icon=.*|Icon=$icon|" \
        -e "s|^TryExec=.*||" \
        "$src/share/applications/crest.desktop" >"$prefix/share/applications/crest.desktop"
    chmod 644 "$prefix/share/applications/crest.desktop"
    refresh_linux_caches
    say "installed crest to $prefix/bin/crest and the app menu ($prefix/share/applications/crest.desktop)"
}

install_macos() { # extracted dir
    src="$1"
    [ -d "$src/crest.app" ] || die "the tarball has no crest.app"
    apps="$(apps_dir)"
    mkdir -p "$apps" "$prefix/bin"
    app="$apps/$APP_NAME.app"
    rm -rf "$app"
    cp -R "$src/crest.app" "$app"
    # Releases from v0.14.0 on are signed and notarized, and a download by
    # curl is not quarantined anyway; this is for older, unsigned ones.
    if command -v xattr >/dev/null 2>&1; then
        xattr -dr com.apple.quarantine "$app" 2>/dev/null || true
    fi
    lsregister -f "$app"
    # Before v0.14.0 the app's executable was a launcher script next to
    # crest-bin; now it is the binary itself.
    bin="$app/Contents/MacOS/crest"
    [ -x "$app/Contents/MacOS/crest-bin" ] && bin="$app/Contents/MacOS/crest-bin"
    ln -sf "$bin" "$prefix/bin/crest"
    say "installed crest.app to $apps (Launchpad, Spotlight) and the command $prefix/bin/crest"
}

hints() { # target
    case ":$PATH:" in
        *":$prefix/bin:"*) ;;
        *) warn "$prefix/bin is not in your PATH; add it to use \`crest\` in a terminal" ;;
    esac
    missing=""
    for tool in rg fzf git; do
        command -v "$tool" >/dev/null 2>&1 || missing="$missing $tool"
    done
    if [ -n "$missing" ]; then
        case "$1" in
            *-apple-darwin) warn "crest needs:$missing (brew install ripgrep fzf git)" ;;
            *) warn "crest needs:$missing (install ripgrep, fzf and git with your package manager)" ;;
        esac
    fi
    say ""
    say "Next: open crest (from the app menu or with \`crest\`) and paste your Anthropic"
    say "API key when it asks (or run /login). It is stored in the system keyring."
    say "Opened from the app menu, crest reads the environment of your login shell"
    say "(~/.zprofile, ~/.profile), not ~/.zshrc or ~/.bashrc."
}

main() {
    target=$(detect_target) || die "no crest build for $(uname -s) $(uname -m) (Linux x86_64 or aarch64 and macOS only)"
    if [ "$uninstall" = 1 ]; then
        do_uninstall "$target"
        return
    fi

    work=$(mktemp -d "${TMPDIR:-/tmp}/crest-install.XXXXXX")
    trap 'rm -rf "$work"' EXIT INT TERM

    # An extracted tarball: this script sits next to the files.
    here=$(cd "$(dirname "$0")" 2>/dev/null && pwd || echo "")
    if [ -z "$tarball" ] && [ -z "$version" ] && [ -n "$here" ] &&
        { [ -x "$here/crest" ] || [ -d "$here/crest.app" ]; } && [ -f "$here/README.txt" ]; then
        src="$here"
    else
        if [ -z "$tarball" ]; then
            [ -n "$version" ] || version=$(latest_version)
            case "$version" in v*) ;; *) version="v$version" ;; esac
            file="crest-${version#v}-$target.tar.gz"
            base="https://github.com/$REPO/releases/download/$version"
            say "downloading $file"
            fetch "$base/$file" "$work/$file" || die "could not download $base/$file"
            fetch "$base/$file.sha256" "$work/$file.sha256" || die "could not download $file.sha256"
            sha256_check "$work" "$file" || die "the checksum of $file does not match"
            tarball="$work/$file"
        fi
        [ -f "$tarball" ] || die "$tarball does not exist"
        mkdir "$work/x"
        tar -xzf "$tarball" -C "$work/x"
        src=$(find "$work/x" -mindepth 1 -maxdepth 1 -type d | head -n 1)
        [ -n "$src" ] || die "$tarball is empty"
    fi

    case "$target" in
        *-apple-darwin) install_macos "$src" ;;
        *) install_linux "$src" ;;
    esac
    hints "$target"
}

main
