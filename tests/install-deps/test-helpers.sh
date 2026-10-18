#!/usr/bin/env bash
# Tests de las funciones puras de scripts/install-deps.sh.
# Uso: bash tests/install-deps/test-helpers.sh   (también con Bash 3.2)

set -u

HERE=$(cd "$(dirname "$0")" && pwd)
# shellcheck disable=SC2034 # la lee el script incluido
CREST_INSTALL_DEPS_LIB=1
# shellcheck source=scripts/install-deps.sh
. "$HERE/../../scripts/install-deps.sh"

FAILS=0
PASSES=0

check() { # descripción, luego el comando que debe tener éxito
    local desc=$1
    shift
    if "$@"; then
        PASSES=$((PASSES + 1))
    else
        FAILS=$((FAILS + 1))
        echo "FALLA: $desc" >&2
    fi
}

eq() { [ "$1" = "$2" ] || { echo "  esperado '$2', obtenido '$1'" >&2; return 1; }; }
not() { ! "$@"; }

# version_ge
check "igual" version_ge 13.0 13.0
check "mayor mayor" version_ge 14.1.0 13.0
check "campo que falta cuenta como 0" version_ge 13 13.0
check "numérico, no lexicográfico" version_ge 0.100 0.36
check "menor" not version_ge 0.29 0.36
check "menor en el último campo" not version_ge 0.35.9 0.36
check "sufijo ignorado" version_ge 1.2.3-rc1 1.2.3
check "mayor con más campos" version_ge 0.36.1 0.36

# extract_version
check "ripgrep" eq "$(echo 'ripgrep 14.1.0 (rev e50df40a19)' | extract_version)" 14.1.0
check "fzf debian" eq "$(echo '0.29 (debian)' | extract_version)" 0.29
check "jq" eq "$(echo 'jq-1.7.1' | extract_version)" 1.7.1
check "gcc" eq "$(echo 'cc (Ubuntu 13.2.0-23ubuntu4) 13.2.0' | extract_version)" 13.2.0
check "primera línea" eq "$(printf 'cmake version 3.28.3\n\nCMake suite 9.9\n' | extract_version)" 3.28.3
check "sin versión" eq "$(echo 'nada' | extract_version)" ""

# list_add / list_count
check "lista vacía" eq "$(list_add '' a)" a
check "añade" eq "$(list_add 'a b' c)" "a b c"
check "no duplica" eq "$(list_add 'a b c' b)" "a b c"
check "no confunde prefijos" eq "$(list_add 'xcb' x)" "xcb x"
check "cuenta" eq "$(list_count 'a b c')" 3
check "cuenta vacía" eq "$(list_count '')" 0

# os_release_field
TMP_OS=$(mktemp "${TMPDIR:-/tmp}/os-release.XXXXXX")
printf 'NAME="Ubuntu"\nID=ubuntu\nID_LIKE=debian\nPRETTY_NAME="Ubuntu 24.04 LTS"\nVERSION_ID='"'"'24.04'"'"'\n' >"$TMP_OS"
check "ID sin comillas" eq "$(OS_RELEASE_FILE=$TMP_OS os_release_field ID)" ubuntu
check "con comillas dobles" eq "$(OS_RELEASE_FILE=$TMP_OS os_release_field PRETTY_NAME)" "Ubuntu 24.04 LTS"
check "con comillas simples" eq "$(OS_RELEASE_FILE=$TMP_OS os_release_field VERSION_ID)" 24.04
check "campo ausente" eq "$(OS_RELEASE_FILE=$TMP_OS os_release_field VARIANT)" ""
rm -f "$TMP_OS"

# pkg_mgr_for_distro
check "ubuntu" eq "$(pkg_mgr_for_distro ubuntu)" apt
check "rocky" eq "$(pkg_mgr_for_distro rocky)" dnf
check "opensuse" eq "$(pkg_mgr_for_distro opensuse-tumbleweed)" zypper
check "void" eq "$(pkg_mgr_for_distro void)" xbps
check "desconocida" eq "$(pkg_mgr_for_distro plan9)" ""

# verify_sha256 y destinos de descarga
TMP_F=$(mktemp "${TMPDIR:-/tmp}/sha.XXXXXX")
printf 'abc' >"$TMP_F"
check "sha256 correcta" verify_sha256 "$TMP_F" ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad
check "sha256 incorrecta" not verify_sha256 "$TMP_F" 0000 2>/dev/null
rm -f "$TMP_F"
PLATFORM=macos ARCH=aarch64
check "rg macOS arm" eq "$(rg_target)" aarch64-apple-darwin
check "fzf macOS arm" eq "$(fzf_target)" darwin_arm64
PLATFORM=linux ARCH=x86_64
check "rg linux x86_64" eq "$(rg_target)" x86_64-unknown-linux-musl
check "fzf linux x86_64" eq "$(fzf_target)" linux_amd64
PLATFORM=linux ARCH=riscv64
check "rg arquitectura no soportada" not rg_target

# packages_for: sin duplicados y vacío para lo que el gestor no provee
PKG_MGR=apk
check "apk deduplica xkbcommon" eq "$(packages_for 'xkbcommon xkbcommon-x11 cc')" "libxkbcommon-dev build-base"
PKG_MGR=apt
check "apt" eq "$(packages_for 'rg curl x11-xcb')" "ripgrep curl ca-certificates libx11-xcb-dev"
check "rust no es un paquete" eq "$(packages_for 'rust clippy')" ""
PKG_MGR=brew
check "brew sin cc" eq "$(packages_for 'cc clang pkg-config')" "pkgconf"

# install_packages: en bloque y, si falla, uno por uno reportando los que fallan
INSTALLED=''
pm_refresh() { :; }
pm_install() {
    case " $* " in *" no-existe "*) return 1 ;; esac
    INSTALLED="$INSTALLED $*"
}
info() { :; }
warn() { :; }
err() { :; }
PKG_MGR=apt
install_packages "a b c"
check "bloque" eq "$INSTALLED" " a b c"
INSTALLED=''
check "uno por uno falla" not install_packages "a no-existe c"
check "uno por uno instala el resto" eq "$INSTALLED" " a c"

echo "$PASSES ok, $FAILS fallos"
[ "$FAILS" = 0 ]
