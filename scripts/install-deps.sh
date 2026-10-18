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

main() {
    parse_args "$@"
}

# Con CREST_INSTALL_DEPS_LIB=1 el script solo define funciones (para tests).
if [ "${CREST_INSTALL_DEPS_LIB:-0}" != 1 ]; then
    main "$@"
fi
