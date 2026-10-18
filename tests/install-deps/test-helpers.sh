#!/usr/bin/env bash
# Tests for the pure functions of scripts/install-deps.sh.
# Usage: bash tests/install-deps/test-helpers.sh   (also with Bash 3.2)

set -u

HERE=$(cd "$(dirname "$0")" && pwd)
# shellcheck disable=SC2034 # read by the sourced script
CREST_INSTALL_DEPS_LIB=1
# shellcheck source=scripts/install-deps.sh
. "$HERE/../../scripts/install-deps.sh"

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
not() { ! "$@"; }

# version_ge
check "equal" version_ge 13.0 13.0
check "greater major" version_ge 14.1.0 13.0
check "missing field counts as 0" version_ge 13 13.0
check "numeric, not lexicographic" version_ge 0.100 0.36
check "less" not version_ge 0.29 0.36
check "less in the last field" not version_ge 0.35.9 0.36
check "suffix ignored" version_ge 1.2.3-rc1 1.2.3
check "greater with more fields" version_ge 0.36.1 0.36

# extract_version
check "ripgrep" eq "$(echo 'ripgrep 14.1.0 (rev e50df40a19)' | extract_version)" 14.1.0
check "fzf debian" eq "$(echo '0.29 (debian)' | extract_version)" 0.29
check "jq" eq "$(echo 'jq-1.7.1' | extract_version)" 1.7.1
check "gcc" eq "$(echo 'cc (Ubuntu 13.2.0-23ubuntu4) 13.2.0' | extract_version)" 13.2.0
check "first line" eq "$(printf 'cmake version 3.28.3\n\nCMake suite 9.9\n' | extract_version)" 3.28.3
check "no version" eq "$(echo 'nothing' | extract_version)" ""

# list_add / list_count
check "empty list" eq "$(list_add '' a)" a
check "adds" eq "$(list_add 'a b' c)" "a b c"
check "no duplicates" eq "$(list_add 'a b c' b)" "a b c"
check "prefixes are not confused" eq "$(list_add 'xcb' x)" "xcb x"
check "count" eq "$(list_count 'a b c')" 3
check "empty count" eq "$(list_count '')" 0

# os_release_field
TMP_OS=$(mktemp "${TMPDIR:-/tmp}/os-release.XXXXXX")
printf 'NAME="Ubuntu"\nID=ubuntu\nID_LIKE=debian\nPRETTY_NAME="Ubuntu 24.04 LTS"\nVERSION_ID='"'"'24.04'"'"'\n' >"$TMP_OS"
check "unquoted ID" eq "$(OS_RELEASE_FILE=$TMP_OS os_release_field ID)" ubuntu
check "double quotes" eq "$(OS_RELEASE_FILE=$TMP_OS os_release_field PRETTY_NAME)" "Ubuntu 24.04 LTS"
check "single quotes" eq "$(OS_RELEASE_FILE=$TMP_OS os_release_field VERSION_ID)" 24.04
check "absent field" eq "$(OS_RELEASE_FILE=$TMP_OS os_release_field VARIANT)" ""
rm -f "$TMP_OS"

# pkg_mgr_for_distro
check "ubuntu" eq "$(pkg_mgr_for_distro ubuntu)" apt
check "rocky" eq "$(pkg_mgr_for_distro rocky)" dnf
check "opensuse" eq "$(pkg_mgr_for_distro opensuse-tumbleweed)" zypper
check "void" eq "$(pkg_mgr_for_distro void)" xbps
check "unknown" eq "$(pkg_mgr_for_distro plan9)" ""

# verify_sha256 and download targets
TMP_F=$(mktemp "${TMPDIR:-/tmp}/sha.XXXXXX")
printf 'abc' >"$TMP_F"
check "correct sha256" verify_sha256 "$TMP_F" ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad
check "wrong sha256" not verify_sha256 "$TMP_F" 0000 2>/dev/null
rm -f "$TMP_F"
PLATFORM=macos ARCH=aarch64
check "rg macOS arm" eq "$(rg_target)" aarch64-apple-darwin
check "fzf macOS arm" eq "$(fzf_target)" darwin_arm64
PLATFORM=linux ARCH=x86_64
check "rg linux x86_64" eq "$(rg_target)" x86_64-unknown-linux-musl
check "fzf linux x86_64" eq "$(fzf_target)" linux_amd64
PLATFORM=linux ARCH=riscv64
check "rg unsupported architecture" not rg_target

# packages_for: no duplicates, and empty for what the manager does not provide
PKG_MGR=apk
check "apk deduplicates xkbcommon" eq "$(packages_for 'xkbcommon xkbcommon-x11 cc')" "libxkbcommon-dev build-base"
PKG_MGR=apt
check "apt" eq "$(packages_for 'rg curl x11-xcb')" "ripgrep curl ca-certificates libx11-xcb-dev"
check "rust is not a package" eq "$(packages_for 'rust clippy')" ""
PKG_MGR=brew
check "brew without cc" eq "$(packages_for 'cc clang pkg-config')" "pkgconf"

# install_packages: in one go and, on failure, one by one reporting the failures
INSTALLED=''
pm_refresh() { :; }
pm_install() {
    case " $* " in *" does-not-exist "*) return 1 ;; esac
    INSTALLED="$INSTALLED $*"
}
info() { :; }
warn() { :; }
err() { :; }
PKG_MGR=apt
install_packages "a b c"
check "bulk" eq "$INSTALLED" " a b c"
INSTALLED=''
check "one by one fails" not install_packages "a does-not-exist c"
check "one by one installs the rest" eq "$INSTALLED" " a c"

echo "$PASSES ok, $FAILS failures"
[ "$FAILS" = 0 ]
