#!/usr/bin/env bash
# install-deps.sh: installs or checks crest's external dependencies.
#
# Requirements and dependency table: README.md, section "Requirements".
#
# Must work with Bash 3.2 (the macOS one): no associative arrays, `mapfile`,
# `${var,,}`, `|&` or `&>>`. Lists are space-separated strings (no package or
# command name contains spaces), so no arrays are used and `set -u` is safe.

set -u

PROG=${0##*/}

OPT_CHECK=0
OPT_YES=0
OPT_DEV=0
OPT_NO_RUST=0

# --- output -----------------------------------------------------------------

if [ -t 1 ] && [ -z "${NO_COLOR:-}" ] && [ "${TERM:-dumb}" != dumb ]; then
    C_RED=$(printf '\033[31m')
    C_GREEN=$(printf '\033[32m')
    C_YELLOW=$(printf '\033[33m')
    C_BOLD=$(printf '\033[1m')
    C_RESET=$(printf '\033[0m')
else
    C_RED='' C_GREEN='' C_YELLOW='' C_BOLD='' C_RESET=''
fi

info() { printf '%s==>%s %s\n' "$C_BOLD" "$C_RESET" "$*"; }
warn() { printf '%swarning:%s %s\n' "$C_YELLOW" "$C_RESET" "$*" >&2; }
err() { printf '%serror:%s %s\n' "$C_RED" "$C_RESET" "$*" >&2; }
die() {
    err "$*"
    exit 1
}

usage() {
    cat <<EOF
Usage: $PROG [options]

Installs crest's external dependencies for the system and its package
manager (apt, dnf, yum, pacman, zypper, apk, xbps or Homebrew).

Options:
  --check     Only check, without installing or asking for root. Exits 1 if anything is missing.
  --yes, -y   Do not ask before installing (non-interactive mode).
  --dev       Include development tools (cargo-insta, cargo-nextest).
  --no-rust   Do not check or install Rust.
  -h, --help  Show this help.

Exit codes: 0 everything present, 1 something missing, 2 incorrect usage.
EOF
}

parse_args() {
    while [ $# -gt 0 ]; do
        case $1 in
            --check) OPT_CHECK=1 ;;
            --yes | -y) OPT_YES=1 ;;
            --dev) OPT_DEV=1 ;;
            --no-rust) OPT_NO_RUST=1 ;;
            -h | --help)
                usage
                exit 0
                ;;
            *)
                err "unknown option: $1"
                usage >&2
                exit 2
                ;;
        esac
        shift
    done
}

# --- system detection -------------------------------------------------------

PLATFORM=unknown # linux | macos | unknown
ARCH=unknown     # x86_64 | aarch64 | whatever uname -m says
DISTRO_ID=''
DISTRO_LIKE=''
DISTRO_NAME=''
PKG_MGR=none # apt | dnf | yum | pacman | zypper | apk | xbps | brew | none
SUDO=''

have() { command -v "$1" >/dev/null 2>&1; }

# Reads an os-release field without sourcing the file.
os_release_field() {
    sed -n "s/^$1=//p" "${OS_RELEASE_FILE:-/etc/os-release}" 2>/dev/null |
        head -n 1 | sed "s/^[\"']//; s/[\"']\$//"
}

# Prints the package manager matching an ID/ID_LIKE word.
pkg_mgr_for_distro() {
    case $1 in
        debian | ubuntu | linuxmint | pop | raspbian) echo apt ;;
        fedora | rhel | centos | rocky | almalinux | amzn | ol) echo dnf ;;
        arch | manjaro | endeavouros | artix | cachyos) echo pacman ;;
        opensuse* | suse | sles | sled) echo zypper ;;
        alpine | postmarketos) echo apk ;;
        void) echo xbps ;;
    esac
}

# Main binary of each package manager.
pkg_mgr_cmd() {
    case $1 in
        apt) echo apt-get ;;
        xbps) echo xbps-install ;;
        *) echo "$1" ;;
    esac
}

# Finds Homebrew even when it is not in the PATH (fresh install).
# CREST_BREW_CANDIDATES overrides the well-known locations (the tests use it
# so a Homebrew installed on the CI runner does not leak in).
find_brew() {
    if have brew; then
        return 0
    fi
    local b
    for b in ${CREST_BREW_CANDIDATES-/opt/homebrew/bin/brew /usr/local/bin/brew /home/linuxbrew/.linuxbrew/bin/brew}; do
        if [ -x "$b" ]; then
            PATH="${b%/brew}:$PATH"
            export PATH
            return 0
        fi
    done
    return 1
}

detect_pkg_mgr() {
    if [ "$PLATFORM" = macos ]; then
        if find_brew; then PKG_MGR=brew; fi
        return 0
    fi
    local word mgr
    # os-release's hint first; if its manager is absent, by presence.
    for word in $DISTRO_ID $DISTRO_LIKE; do
        mgr=$(pkg_mgr_for_distro "$word")
        if [ "$mgr" = dnf ] && ! have dnf && have yum; then mgr=yum; fi
        if [ -n "$mgr" ] && have "$(pkg_mgr_cmd "$mgr")"; then
            PKG_MGR=$mgr
            return 0
        fi
    done
    for mgr in apt dnf yum pacman zypper apk xbps; do
        if have "$(pkg_mgr_cmd "$mgr")"; then
            PKG_MGR=$mgr
            return 0
        fi
    done
}

detect_system() {
    case $(uname -s) in
        Linux) PLATFORM=linux ;;
        Darwin) PLATFORM=macos ;;
        *) PLATFORM=unknown ;;
    esac
    ARCH=$(uname -m)
    case $ARCH in
        x86_64 | amd64) ARCH=x86_64 ;;
        aarch64 | arm64) ARCH=aarch64 ;;
    esac
    if [ "$PLATFORM" = linux ]; then
        DISTRO_ID=$(os_release_field ID)
        DISTRO_LIKE=$(os_release_field ID_LIKE)
        DISTRO_NAME=$(os_release_field PRETTY_NAME)
        [ -n "$DISTRO_NAME" ] || DISTRO_NAME="Linux ${DISTRO_ID:-unknown}"
    elif [ "$PLATFORM" = macos ]; then
        DISTRO_NAME=$(sw_vers -productVersion 2>/dev/null)
        DISTRO_NAME="macOS${DISTRO_NAME:+ $DISTRO_NAME}"
    else
        DISTRO_NAME=$(uname -s)
    fi
    detect_pkg_mgr
}

# Sets SUDO for the system package managers. Only used when installing.
setup_sudo() {
    if [ "$PKG_MGR" = brew ] || [ "$(id -u)" = 0 ]; then
        SUDO=''
    elif have sudo; then
        # Without a terminal sudo cannot ask for a password: fail instead of hanging.
        if [ -t 0 ]; then SUDO='sudo'; else SUDO='sudo -n'; fi
    elif have doas; then
        if [ -t 0 ]; then SUDO='doas'; else SUDO='doas -n'; fi
    else
        die "installing packages needs root and there is no sudo or doas"
    fi
}

print_system() {
    info "System: $DISTRO_NAME ($PLATFORM, $ARCH), package manager: $PKG_MGR"
}

# --- list and version helpers -----------------------------------------------

# Prints list $1 with word $2 added if it was not there.
list_add() {
    case " $1 " in
        *" $2 "*) printf '%s\n' "$1" ;;
        *) printf '%s\n' "${1:+$1 }$2" ;;
    esac
}

list_count() {
    # shellcheck disable=SC2086 # word counting on purpose
    set -- $1
    echo $#
}

# Extracts the first numeric version (1.2, 1.2.3...) from standard input.
extract_version() {
    sed -n 's/^[^0-9]*\([0-9][0-9]*\(\.[0-9][0-9]*\)*\).*$/\1/p' | head -n 1
}

# version_ge A B: succeeds if version A >= B. Compares numerically field by
# field; missing fields count as 0 and suffixes (-rc1) are ignored.
version_ge() {
    local a=$1 b=$2 x y
    while [ -n "$a" ] || [ -n "$b" ]; do
        x=${a%%.*}
        y=${b%%.*}
        case $a in *.*) a=${a#*.} ;; *) a='' ;; esac
        case $b in *.*) b=${b#*.} ;; *) b='' ;; esac
        x=${x%%[!0-9]*}
        y=${y%%[!0-9]*}
        x=${x:-0}
        y=${y:-0}
        if [ "$x" -gt "$y" ]; then return 0; fi
        if [ "$x" -lt "$y" ]; then return 1; fi
    done
    return 0
}

# Version reported by `cmd [args...] --version`, without reading stdin.
tool_version() {
    "$@" --version </dev/null 2>/dev/null | extract_version
}

# --- checks -----------------------------------------------------------------

RG_MIN=13.0
FZF_MIN=0.36
LINUX_LIBS="xkbcommon xkbcommon-x11 wayland-client xcb x11-xcb fontconfig freetype2 vulkan"
LOCAL_BIN=${CREST_LOCAL_BIN:-$HOME/.local/bin}
CARGO_BIN=${CARGO_HOME:-$HOME/.cargo}/bin
ORIG_PATH=$PATH
MISSING=''        # ids of what is missing (see pkg_names)
SHADOWED_DIRS=''  # directories added to the PATH that the user's PATH does not put first
SHADOWED_CMDS=''
VULKAN_DRIVER=0   # 1 if a Vulkan ICD was found

# The binaries this script installs in ~/.local/bin or ~/.cargo/bin must be
# found even if those directories are not in the user's PATH yet.
PATH="$LOCAL_BIN:$CARGO_BIN:$PATH"
export PATH

report_ok() { printf '  %s[ok]%s      %-18s %s\n' "$C_GREEN" "$C_RESET" "$1" "$2"; }
report_warn() { printf '  %s[warn]%s    %-18s %s\n' "$C_YELLOW" "$C_RESET" "$1" "$2"; }
# report_missing ID LABEL DETAIL
report_missing() {
    printf '  %s[missing]%s %-18s %s\n' "$C_RED" "$C_RESET" "$2" "$3"
    MISSING=$(list_add "$MISSING" "$1")
}

# Warns if CMD was found in a directory this script added and the user's PATH
# does not resolve it to that same binary.
check_shadowed() {
    local cmd=$1 path=$2 user_path
    case $path in
        "$LOCAL_BIN"/* | "$CARGO_BIN"/*) ;;
        *) return 0 ;;
    esac
    user_path=$(PATH=$ORIG_PATH && command -v "$cmd" 2>/dev/null)
    if [ "$user_path" != "$path" ]; then
        SHADOWED_DIRS=$(list_add "$SHADOWED_DIRS" "${path%/*}")
        SHADOWED_CMDS=$(list_add "$SHADOWED_CMDS" "$cmd")
    fi
}

# A single warning per directory the user's PATH does not resolve first.
report_shadowed() {
    local dir
    for dir in $SHADOWED_DIRS; do
        report_warn PATH "put $dir at the front of your PATH (used by: $SHADOWED_CMDS)"
    done
}

# check_tool ID CMD LABEL [MIN_VERSION]
check_tool() {
    local id=$1 cmd=$2 label=$3 min=${4:-} path ver
    if ! path=$(command -v "$cmd" 2>/dev/null); then
        report_missing "$id" "$label" "'$cmd' not found"
        return 1
    fi
    ver=$(tool_version "$cmd")
    if [ -n "$min" ] && { [ -z "$ver" ] || ! version_ge "$ver" "$min"; }; then
        report_missing "$id" "$label" "version ${ver:-unknown} at $path; >= $min is needed"
        return 1
    fi
    report_ok "$label" "${ver:+$ver }($path)"
    check_shadowed "$cmd" "$path"
}

check_c_compiler() {
    if have cc; then
        check_tool cc cc "C compiler"
    elif have gcc; then
        check_tool cc gcc "C compiler"
    else
        report_missing cc "C compiler" "neither 'cc' nor 'gcc' found"
    fi
}

check_libs() {
    local lib ver
    if ! have pkg-config; then
        for lib in $LINUX_LIBS; do
            report_missing "$lib" "lib $lib" "cannot be checked without pkg-config"
        done
        return 0
    fi
    for lib in $LINUX_LIBS; do
        if ver=$(pkg-config --modversion "$lib" 2>/dev/null); then
            report_ok "lib $lib" "$ver"
        else
            report_missing "$lib" "lib $lib" "pkg-config cannot find $lib.pc"
        fi
    done
}

# The driver depends on the GPU, which CI usually lacks: only warn.
check_vulkan_driver() {
    local dir f n=0 first=''
    for dir in /usr/share/vulkan/icd.d /etc/vulkan/icd.d /usr/local/share/vulkan/icd.d; do
        for f in "$dir"/*.json; do
            [ -e "$f" ] || continue
            n=$((n + 1))
            [ -n "$first" ] || first=${f##*/}
        done
    done
    if [ "$n" -gt 0 ]; then
        VULKAN_DRIVER=1
        report_ok "Vulkan driver" "$n ICD ($first$([ "$n" -gt 1 ] && printf ', ...'))"
    else
        VULKAN_DRIVER=0
        report_warn "Vulkan driver" "no ICD in .../vulkan/icd.d; install your GPU driver (mesa or nvidia)"
    fi
}

check_rust() {
    local ver
    check_tool rust cargo cargo
    check_tool rust rustc rustc
    check_tool rustfmt rustfmt rustfmt
    if have cargo && ver=$(tool_version cargo clippy) && [ -n "$ver" ]; then
        report_ok clippy "$ver"
    else
        report_missing clippy clippy "'cargo clippy' not available"
    fi
    if have rustup; then
        report_ok rustup "$(tool_version rustup)"
    elif have cargo; then
        report_warn rustup "Rust does not come from rustup: rust-toolchain.toml will not apply"
    fi
}

# check_cargo_sub ID SUBCOMMAND
check_cargo_sub() {
    local ver
    if have cargo && ver=$(tool_version cargo "$2") && [ -n "$ver" ]; then
        report_ok "$1" "$ver"
    else
        report_missing "$1" "$1" "'cargo $2' not available"
    fi
}

# --- macOS ------------------------------------------------------------------

clt_installed() {
    local dir
    dir=$(xcode-select -p 2>/dev/null) && [ -n "$dir" ] && [ -d "$dir" ]
}

check_macos() {
    local ver
    if clt_installed; then
        report_ok "Command Line Tools" "$(xcode-select -p 2>/dev/null)"
        # xcrun opens no dialogs; it fails if only the CLT are present (no Xcode).
        ver=$(xcrun -sdk macosx metal --version </dev/null 2>/dev/null | extract_version)
        if [ -n "$ver" ]; then
            report_ok "Metal compiler" "$ver"
        else
            report_missing metal "Metal compiler" "'xcrun -sdk macosx metal' not available (the full Xcode is needed)"
        fi
    else
        report_missing clt "Command Line Tools" "'xcode-select -p' cannot find the tools"
        report_missing metal "Metal compiler" "requires Xcode"
    fi
    if find_brew; then
        report_ok Homebrew "$(tool_version brew) ($(command -v brew))"
    else
        report_missing brew Homebrew "'brew' not found"
    fi
}

# Explains how to install what this script does not install by itself on
# macOS (IDs clt, metal and brew from list $1).
print_macos_help() {
    case " $1 " in
        *" clt "*)
            cat <<'EOF'
Xcode Command Line Tools (C compiler, clang, git):
    xcode-select --install
EOF
            ;;
    esac
    case " $1 " in
        *" metal "*)
            cat <<'EOF'
Metal compiler (GPUI shaders): install the full Xcode from the App Store, then
    sudo xcode-select -s /Applications/Xcode.app/Contents/Developer
    sudo xcodebuild -runFirstLaunch
    xcodebuild -downloadComponent MetalToolchain   # Xcode 26 or later
EOF
            ;;
    esac
    case " $1 " in
        *" brew "*)
            cat <<'EOF'
Homebrew (https://brew.sh):
    /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
EOF
            ;;
    esac
}

run_checks() {
    MISSING=''
    SHADOWED_DIRS=''
    SHADOWED_CMDS=''
    info "Checking dependencies"
    if [ "$PLATFORM" = macos ]; then
        check_macos
    fi
    if [ "$PLATFORM" = macos ] && ! clt_installed; then
        # Without the Command Line Tools, cc/clang/git in /usr/bin are shims
        # that open an install dialog: do not run them.
        report_missing cc "C compiler" "requires the Command Line Tools"
        report_missing clang clang "requires the Command Line Tools"
        report_missing git git "requires the Command Line Tools"
    else
        check_c_compiler
        check_tool clang clang clang
        check_tool git git git
    fi
    check_tool pkg-config pkg-config pkg-config
    check_tool cmake cmake cmake
    check_tool jq jq jq
    check_tool rg rg ripgrep "$RG_MIN"
    check_tool fzf fzf fzf "$FZF_MIN"
    if [ "$PLATFORM" = linux ]; then
        check_libs
        check_vulkan_driver
    fi
    if [ "$OPT_NO_RUST" = 0 ]; then
        check_rust
    fi
    if [ "$OPT_DEV" = 1 ]; then
        check_cargo_sub cargo-insta insta
        check_cargo_sub cargo-nextest nextest
    fi
    report_shadowed
}

# --- packages per manager ---------------------------------------------------

# pkg_names MANAGER ID: packages that provide ID (empty if the manager does
# not provide it, e.g. Rust, which goes through rustup). The IDs are those of
# MISSING plus curl, tar and gzip, needed to download rg, fzf and rustup.
pkg_names() {
    case $1 in
        apt) case $2 in
            cc) echo build-essential ;;
            pkg-config | cmake | clang | git | jq | fzf | tar | gzip) echo "$2" ;;
            rg) echo ripgrep ;;
            curl) echo "curl ca-certificates" ;;
            xkbcommon) echo libxkbcommon-dev ;;
            xkbcommon-x11) echo libxkbcommon-x11-dev ;;
            wayland-client) echo libwayland-dev ;;
            xcb) echo libxcb1-dev ;;
            x11-xcb) echo libx11-xcb-dev ;;
            fontconfig) echo libfontconfig-dev ;;
            freetype2) echo libfreetype-dev ;;
            vulkan) echo libvulkan-dev ;;
            vulkan-driver) echo mesa-vulkan-drivers ;;
        esac ;;
        dnf | yum) case $2 in
            cc) echo "gcc gcc-c++ make" ;;
            pkg-config) if [ "$1" = dnf ]; then echo pkgconf-pkg-config; else echo pkgconfig; fi ;;
            cmake | clang | git | jq | fzf | curl | tar | gzip) echo "$2" ;;
            rg) echo ripgrep ;;
            xkbcommon) echo libxkbcommon-devel ;;
            xkbcommon-x11) echo libxkbcommon-x11-devel ;;
            wayland-client) echo wayland-devel ;;
            xcb) echo libxcb-devel ;;
            x11-xcb) echo libX11-devel ;;
            fontconfig) echo fontconfig-devel ;;
            freetype2) echo freetype-devel ;;
            vulkan) echo vulkan-loader-devel ;;
            vulkan-driver) echo mesa-vulkan-drivers ;;
        esac ;;
        pacman) case $2 in
            cc) echo base-devel ;;
            pkg-config) echo pkgconf ;;
            cmake | clang | git | jq | fzf | curl | tar | gzip | fontconfig | freetype2) echo "$2" ;;
            rg) echo ripgrep ;;
            xkbcommon) echo libxkbcommon ;;
            xkbcommon-x11) echo libxkbcommon-x11 ;;
            wayland-client) echo wayland ;;
            xcb) echo libxcb ;;
            x11-xcb) echo libx11 ;;
            vulkan) echo vulkan-icd-loader ;;
            # The driver depends on the GPU (vulkan-radeon, vulkan-intel, nvidia-utils).
        esac ;;
        zypper) case $2 in
            cc) echo "gcc gcc-c++ make" ;;
            pkg-config | cmake | clang | git | jq | fzf | curl | tar | gzip) echo "$2" ;;
            rg) echo ripgrep ;;
            xkbcommon) echo libxkbcommon-devel ;;
            xkbcommon-x11) echo libxkbcommon-x11-devel ;;
            wayland-client) echo wayland-devel ;;
            xcb) echo libxcb-devel ;;
            x11-xcb) echo libX11-devel ;;
            fontconfig) echo fontconfig-devel ;;
            freetype2) echo freetype2-devel ;;
            vulkan) echo vulkan-devel ;;
        esac ;;
        apk) case $2 in
            cc) echo build-base ;;
            pkg-config) echo pkgconf ;;
            cmake | clang | git | jq | fzf | curl | tar | gzip) echo "$2" ;;
            rg) echo ripgrep ;;
            xkbcommon | xkbcommon-x11) echo libxkbcommon-dev ;;
            wayland-client) echo wayland-dev ;;
            xcb) echo libxcb-dev ;;
            x11-xcb) echo libx11-dev ;;
            fontconfig) echo fontconfig-dev ;;
            freetype2) echo freetype-dev ;;
            vulkan) echo vulkan-loader-dev ;;
        esac ;;
        xbps) case $2 in
            cc) echo base-devel ;;
            pkg-config | cmake | clang | git | jq | fzf | curl | tar | gzip) echo "$2" ;;
            rg) echo ripgrep ;;
            xkbcommon | xkbcommon-x11) echo libxkbcommon-devel ;;
            wayland-client) echo wayland-devel ;;
            xcb) echo libxcb-devel ;;
            x11-xcb) echo libX11-devel ;;
            fontconfig) echo fontconfig-devel ;;
            freetype2) echo freetype-devel ;;
            vulkan) echo vulkan-loader-devel ;;
        esac ;;
        brew) case $2 in
            # cc and clang come with the Command Line Tools.
            cmake | git | jq | fzf | curl) echo "$2" ;;
            pkg-config) echo pkgconf ;;
            rg) echo ripgrep ;;
        esac ;;
    esac
}

# Packages (deduplicated) of the current manager for the ID list $1.
packages_for() {
    local id p pkgs=''
    for id in $1; do
        for p in $(pkg_names "$PKG_MGR" "$id"); do
            pkgs=$(list_add "$pkgs" "$p")
        done
    done
    printf '%s\n' "$pkgs"
}

as_root() {
    if [ -n "$SUDO" ]; then
        # shellcheck disable=SC2086 # SUDO may be "sudo -n"
        $SUDO "$@"
    else
        "$@"
    fi
}

# Refreshes the manager's indexes before installing, when needed.
pm_refresh() {
    case $PKG_MGR in
        apt) as_root apt-get update ;;
        apk) as_root apk update ;;
        xbps) as_root xbps-install -S ;;
        pacman)
            # Without synced databases (fresh image) -Sy would leave only a
            # partial upgrade; Arch requires -Syu.
            if [ -z "$(ls /var/lib/pacman/sync 2>/dev/null)" ]; then
                as_root pacman -Syu --noconfirm
            fi
            ;;
    esac
}

pm_install() {
    case $PKG_MGR in
        apt) as_root env DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends "$@" ;;
        dnf | yum) as_root "$PKG_MGR" install -y "$@" ;;
        pacman) as_root pacman -S --needed --noconfirm "$@" ;;
        zypper) as_root zypper --non-interactive install --no-recommends "$@" ;;
        apk) as_root apk add "$@" ;;
        xbps) as_root xbps-install -y "$@" ;;
        brew) brew install "$@" ;;
        *) return 1 ;;
    esac
}

# Installs list $1 in one go; if that fails (e.g. a name that does not exist
# in this distro version), retries one by one and reports the failures.
install_packages() {
    local pkgs=$1 p failed=''
    [ -n "$pkgs" ] || return 0
    info "Installing with $PKG_MGR: $pkgs"
    pm_refresh || warn "could not refresh the $PKG_MGR indexes"
    # shellcheck disable=SC2086 # space-separated package list
    if pm_install $pkgs; then
        return 0
    fi
    warn "bulk install failed; retrying package by package"
    for p in $pkgs; do
        pm_install "$p" || failed=$(list_add "$failed" "$p")
    done
    if [ -n "$failed" ]; then
        err "could not install: $failed"
        return 1
    fi
}

# confirm QUESTION: succeeds if the user accepts (or with --yes).
confirm() {
    local ans
    [ "$OPT_YES" = 1 ] && return 0
    if [ ! -t 0 ]; then
        err "no terminal to confirm; use --yes to install without asking"
        return 1
    fi
    printf '%s [y/N] ' "$1"
    read -r ans || return 1
    case $ans in
        # Spanish answers (s, si, sí) are kept as aliases.
        y | Y | yes | Yes | YES | s | S | si | sí | Si | Sí) return 0 ;;
        *) return 1 ;;
    esac
}

# Summarizes what is missing. Succeeds if nothing is.
print_summary() {
    local pkgs
    if [ -z "$MISSING" ]; then
        info "${C_GREEN}All dependencies are present.${C_RESET}"
        return 0
    fi
    err "missing $(list_count "$MISSING") dependencies: $MISSING"
    pkgs=$(packages_for "$MISSING")
    if [ -n "$pkgs" ]; then
        printf '%s packages that provide them: %s\n' "$PKG_MGR" "$pkgs"
    fi
    if [ "$PLATFORM" = macos ]; then
        print_macos_help "$MISSING"
    fi
    printf 'To install them: %s%s%s\n' "$0" \
        "$([ "$OPT_DEV" = 1 ] && printf ' --dev')" \
        "$([ "$OPT_NO_RUST" = 1 ] && printf ' --no-rust')"
    return 1
}

# --- official GitHub binaries (rg, fzf) -------------------------------------

RG_VERSION=${CREST_RG_VERSION:-15.2.0}
FZF_VERSION=${CREST_FZF_VERSION:-0.74.5}
WORK_DIR=''

cleanup() {
    if [ -n "$WORK_DIR" ]; then rm -rf "$WORK_DIR"; fi
}

# Creates WORK_DIR only once. Do not call it in a subshell: the trap would
# delete it when the subshell exits.
ensure_work_dir() {
    if [ -z "$WORK_DIR" ]; then
        WORK_DIR=$(mktemp -d "${TMPDIR:-/tmp}/crest-deps.XXXXXX") || {
            err "could not create a temporary directory"
            return 1
        }
        trap cleanup EXIT
    fi
}

# download URL DEST
download() {
    if have curl; then
        curl -fsSL --proto '=https' --tlsv1.2 --retry 3 -o "$2" "$1"
    elif have wget; then
        wget -q -O "$2" "$1"
    else
        err "curl or wget is needed to download $1"
        return 1
    fi
}

sha256_of() {
    if have sha256sum; then
        sha256sum "$1" | cut -d ' ' -f 1
    elif have shasum; then
        shasum -a 256 "$1" | cut -d ' ' -f 1
    else
        return 1
    fi
}

# verify_sha256 FILE EXPECTED_SUM
verify_sha256() {
    local got
    if ! got=$(sha256_of "$1") || [ -z "$got" ]; then
        err "no sha256sum or shasum to verify ${1##*/}"
        return 1
    fi
    if [ "$got" != "$2" ]; then
        err "wrong SHA-256 sum for ${1##*/}: $got (expected $2)"
        return 1
    fi
}

# Target triple of the ripgrep releases for this machine.
rg_target() {
    case $PLATFORM:$ARCH in
        linux:x86_64) echo x86_64-unknown-linux-musl ;;
        linux:aarch64) echo aarch64-unknown-linux-musl ;;
        macos:x86_64) echo x86_64-apple-darwin ;;
        macos:aarch64) echo aarch64-apple-darwin ;;
        *) return 1 ;;
    esac
}

# Platform suffix of the fzf releases for this machine.
fzf_target() {
    case $PLATFORM:$ARCH in
        linux:x86_64) echo linux_amd64 ;;
        linux:aarch64) echo linux_arm64 ;;
        macos:x86_64) echo darwin_amd64 ;;
        macos:aarch64) echo darwin_arm64 ;;
        *) return 1 ;;
    esac
}

# install_bin SOURCE NAME: copies an executable to LOCAL_BIN.
install_bin() {
    mkdir -p "$LOCAL_BIN" || return 1
    cp "$1" "$LOCAL_BIN/$2.tmp" && chmod 755 "$LOCAL_BIN/$2.tmp" &&
        mv -f "$LOCAL_BIN/$2.tmp" "$LOCAL_BIN/$2"
}

install_rg_from_github() {
    local target name base dir sum
    target=$(rg_target) || {
        err "no ripgrep binary for $PLATFORM/$ARCH"
        return 1
    }
    name=ripgrep-$RG_VERSION-$target
    base=https://github.com/BurntSushi/ripgrep/releases/download/$RG_VERSION/$name.tar.gz
    ensure_work_dir || return 1
    dir=$WORK_DIR
    info "Downloading ripgrep $RG_VERSION ($target) to $LOCAL_BIN"
    download "$base" "$dir/$name.tar.gz" && download "$base.sha256" "$dir/$name.sha256" || return 1
    sum=$(cut -d ' ' -f 1 "$dir/$name.sha256")
    verify_sha256 "$dir/$name.tar.gz" "$sum" || return 1
    tar -xzf "$dir/$name.tar.gz" -C "$dir" && install_bin "$dir/$name/rg" rg
}

install_fzf_from_github() {
    local target name base dir sum
    target=$(fzf_target) || {
        err "no fzf binary for $PLATFORM/$ARCH"
        return 1
    }
    name=fzf-$FZF_VERSION-$target.tar.gz
    base=https://github.com/junegunn/fzf/releases/download/v$FZF_VERSION
    ensure_work_dir || return 1
    dir=$WORK_DIR
    info "Downloading fzf $FZF_VERSION ($target) to $LOCAL_BIN"
    download "$base/$name" "$dir/$name" &&
        download "$base/fzf_${FZF_VERSION}_checksums.txt" "$dir/fzf-checksums.txt" || return 1
    sum=$(grep " $name\$" "$dir/fzf-checksums.txt" | cut -d ' ' -f 1)
    if [ -z "$sum" ]; then
        err "$name is not listed in fzf_${FZF_VERSION}_checksums.txt"
        return 1
    fi
    verify_sha256 "$dir/$name" "$sum" || return 1
    mkdir -p "$dir/fzf" && tar -xzf "$dir/$name" -C "$dir/fzf" && install_bin "$dir/fzf/fzf" fzf
}

# tool_ok CMD MIN: succeeds if CMD exists and its version is >= MIN.
tool_ok() {
    local ver
    have "$1" || return 1
    ver=$(tool_version "$1")
    [ -n "$ver" ] && version_ge "$ver" "$2"
}

# If rg or fzf are missing or too old after the packages (Ubuntu 22.04's fzf
# is 0.29), installs the official binary in ~/.local/bin.
ensure_search_tools() {
    tool_ok rg "$RG_MIN" || install_rg_from_github || err "could not install ripgrep from GitHub"
    tool_ok fzf "$FZF_MIN" || install_fzf_from_github || err "could not install fzf from GitHub"
    hash -r
}

# --- Rust and development tools ---------------------------------------------

clippy_ok() { have cargo && cargo clippy --version </dev/null >/dev/null 2>&1; }

install_rustup() {
    ensure_work_dir || return 1
    info "Installing Rust with rustup"
    download https://sh.rustup.rs "$WORK_DIR/rustup-init.sh" || return 1
    # -y: the user already confirmed the plan (or passed --yes).
    sh "$WORK_DIR/rustup-init.sh" -y --profile default </dev/null
}

ensure_rust() {
    [ "$OPT_NO_RUST" = 1 ] && return 0
    if ! have cargo; then
        if have rustup; then
            info "Installing the stable toolchain with rustup"
            rustup default stable </dev/null || err "could not install the stable toolchain"
        else
            install_rustup || err "could not install Rust with rustup"
        fi
        hash -r
    fi
    have cargo || return 1
    if have rustfmt && clippy_ok; then
        return 0
    fi
    if have rustup; then
        info "Adding rustfmt and clippy with rustup"
        rustup component add rustfmt clippy </dev/null || err "could not add rustfmt and clippy"
    else
        warn "Rust does not come from rustup: install rustfmt and clippy with your package manager or use rustup"
    fi
}

ensure_dev_tools() {
    [ "$OPT_DEV" = 1 ] || return 0
    if ! have cargo; then
        err "cargo-insta and cargo-nextest cannot be installed without cargo"
        return 1
    fi
    if ! cargo insta --version </dev/null >/dev/null 2>&1; then
        info "Installing cargo-insta"
        cargo install --locked cargo-insta </dev/null || err "could not install cargo-insta"
    fi
    if ! cargo nextest --version </dev/null >/dev/null 2>&1; then
        info "Installing cargo-nextest"
        cargo install --locked cargo-nextest </dev/null || err "could not install cargo-nextest"
    fi
}

# Tools the installer itself needs to download.
bootstrap_ids() {
    local ids='' c
    for c in curl tar gzip; do
        have "$c" || ids=$(list_add "$ids" "$c")
    done
    printf '%s\n' "$ids"
}

run_install() {
    local ids pkgs id
    ids=$MISSING
    [ "$PLATFORM" = linux ] && [ "$VULKAN_DRIVER" = 0 ] && ids=$(list_add "$ids" vulkan-driver)
    for id in $(bootstrap_ids); do ids=$(list_add "$ids" "$id"); done
    pkgs=$(packages_for "$ids")
    if [ -z "$MISSING" ] && [ -z "$pkgs" ]; then
        return 0
    fi
    if [ "$PLATFORM" = macos ]; then
        if ! clt_installed; then
            print_macos_help clt
            die "install the Command Line Tools and run this script again"
        fi
        # Xcode and Homebrew are not installed automatically: explain how.
        print_macos_help "$MISSING"
    elif [ "$PKG_MGR" = none ] && [ -n "$ids" ]; then
        warn "no supported package manager; install by hand: $ids"
    fi
    info "Installation plan"
    [ -n "$pkgs" ] && printf '  packages (%s): %s\n' "$PKG_MGR" "$pkgs"
    printf '  rg and fzf: if missing or too old after the packages, official GitHub binary in %s\n' "$LOCAL_BIN"
    case " $MISSING " in
        *" rust "*) [ "$OPT_NO_RUST" = 0 ] && printf '  Rust: rustup (stable, with rustfmt and clippy)\n' ;;
        *" rustfmt "* | *" clippy "*) printf '  Rust: rustfmt and clippy\n' ;;
    esac
    case " $MISSING " in
        *" cargo-insta "* | *" cargo-nextest "*) printf '  cargo install --locked: cargo-insta, cargo-nextest\n' ;;
    esac
    confirm "Continue?" || die "installation cancelled"
    [ -n "$pkgs" ] && setup_sudo
    install_packages "$pkgs" || true
    hash -r
    ensure_search_tools
    ensure_rust
    ensure_dev_tools
}

main() {
    parse_args "$@"
    detect_system
    print_system
    if [ "$PLATFORM" = unknown ]; then
        warn "unsupported system: $(uname -s); only checking"
        OPT_CHECK=1
    fi
    # When checking, rustup must not download the rust-toolchain.toml toolchain.
    if [ "$OPT_CHECK" = 1 ]; then
        RUSTUP_AUTO_INSTALL=0
        export RUSTUP_AUTO_INSTALL
    fi
    # From the repo root, so rustup applies its rust-toolchain.toml.
    cd "$(dirname "$0")/.." || die "cannot enter the repo root"
    run_checks
    if [ "$OPT_CHECK" = 1 ]; then
        print_summary
        return
    fi
    run_install
    echo
    run_checks
    print_summary
}

# With CREST_INSTALL_DEPS_LIB=1 the script only defines functions (for tests).
if [ "${CREST_INSTALL_DEPS_LIB:-0}" != 1 ]; then
    main "$@"
fi
