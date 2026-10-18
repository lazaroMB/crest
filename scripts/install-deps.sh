#!/usr/bin/env bash
# install-deps.sh: instala o verifica las dependencias externas de crest.
#
# Requisitos y tabla de dependencias: docs/PLAN.md, "Build y dependencias
# externas".
#
# Debe funcionar con Bash 3.2 (el de macOS): nada de arrays asociativos,
# `mapfile`, `${var,,}`, `|&` ni `&>>`. Las listas son cadenas separadas por
# espacios (ningún nombre de paquete o comando lleva espacios), así que no se
# usan arrays y `set -u` es seguro.

set -u

PROG=${0##*/}

OPT_CHECK=0
OPT_YES=0
OPT_DEV=0
OPT_NO_RUST=0

# --- salida -----------------------------------------------------------------

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
warn() { printf '%saviso:%s %s\n' "$C_YELLOW" "$C_RESET" "$*" >&2; }
err() { printf '%serror:%s %s\n' "$C_RED" "$C_RESET" "$*" >&2; }
die() {
    err "$*"
    exit 1
}

usage() {
    cat <<EOF
Uso: $PROG [opciones]

Instala las dependencias externas de crest según el sistema y su gestor de
paquetes (apt, dnf, yum, pacman, zypper, apk, xbps o Homebrew).

Opciones:
  --check     Solo verifica, sin instalar ni pedir root. Sale con 1 si falta algo.
  --yes, -y   No pregunta antes de instalar (modo no interactivo).
  --dev       Incluye herramientas de desarrollo (cargo-insta, cargo-nextest).
  --no-rust   No verifica ni instala Rust.
  -h, --help  Muestra esta ayuda.

Códigos de salida: 0 todo presente, 1 falta algo, 2 uso incorrecto.
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
                err "opción desconocida: $1"
                usage >&2
                exit 2
                ;;
        esac
        shift
    done
}

# --- detección del sistema --------------------------------------------------

PLATFORM=unknown # linux | macos | unknown
ARCH=unknown     # x86_64 | aarch64 | lo que diga uname -m
DISTRO_ID=''
DISTRO_LIKE=''
DISTRO_NAME=''
PKG_MGR=none # apt | dnf | yum | pacman | zypper | apk | xbps | brew | none
SUDO=''

have() { command -v "$1" >/dev/null 2>&1; }

# Lee un campo de os-release sin hacer `source` del archivo.
os_release_field() {
    sed -n "s/^$1=//p" "${OS_RELEASE_FILE:-/etc/os-release}" 2>/dev/null |
        head -n 1 | sed "s/^[\"']//; s/[\"']\$//"
}

# Imprime el gestor que corresponde a una palabra de ID/ID_LIKE.
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

# Binario principal de cada gestor.
pkg_mgr_cmd() {
    case $1 in
        apt) echo apt-get ;;
        xbps) echo xbps-install ;;
        *) echo "$1" ;;
    esac
}

# Busca Homebrew aunque no esté en PATH (instalación recién hecha).
find_brew() {
    if have brew; then
        return 0
    fi
    local b
    for b in /opt/homebrew/bin/brew /usr/local/bin/brew /home/linuxbrew/.linuxbrew/bin/brew; do
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
    # Primero la pista de os-release; si su gestor no está, por presencia.
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
        [ -n "$DISTRO_NAME" ] || DISTRO_NAME="Linux ${DISTRO_ID:-desconocido}"
    elif [ "$PLATFORM" = macos ]; then
        DISTRO_NAME="macOS $(sw_vers -productVersion 2>/dev/null)"
    else
        DISTRO_NAME=$(uname -s)
    fi
    detect_pkg_mgr
}

# Define SUDO para los gestores del sistema. Solo se usa al instalar.
setup_sudo() {
    if [ "$PKG_MGR" = brew ] || [ "$(id -u)" = 0 ]; then
        SUDO=''
    elif have sudo; then
        # Sin terminal, sudo no puede pedir contraseña: que falle en vez de colgarse.
        if [ -t 0 ]; then SUDO='sudo'; else SUDO='sudo -n'; fi
    elif have doas; then
        if [ -t 0 ]; then SUDO='doas'; else SUDO='doas -n'; fi
    else
        die "hace falta root para instalar paquetes y no hay sudo ni doas"
    fi
}

print_system() {
    info "Sistema: $DISTRO_NAME ($PLATFORM, $ARCH), gestor de paquetes: $PKG_MGR"
}

main() {
    parse_args "$@"
    detect_system
    print_system
}

# Con CREST_INSTALL_DEPS_LIB=1 el script solo define funciones (para tests).
if [ "${CREST_INSTALL_DEPS_LIB:-0}" != 1 ]; then
    main "$@"
fi
