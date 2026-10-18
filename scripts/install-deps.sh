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

# --- utilidades de listas y versiones ----------------------------------------

# Imprime la lista $1 con la palabra $2 añadida si no estaba.
list_add() {
    case " $1 " in
        *" $2 "*) printf '%s\n' "$1" ;;
        *) printf '%s\n' "${1:+$1 }$2" ;;
    esac
}

list_count() {
    # shellcheck disable=SC2086 # se cuentan las palabras a propósito
    set -- $1
    echo $#
}

# Extrae la primera versión numérica (1.2, 1.2.3...) de la entrada estándar.
extract_version() {
    sed -n 's/^[^0-9]*\([0-9][0-9]*\(\.[0-9][0-9]*\)*\).*$/\1/p' | head -n 1
}

# version_ge A B: éxito si la versión A >= B. Compara campo a campo, en número;
# los campos que faltan cuentan como 0 y los sufijos (-rc1) se ignoran.
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

# Versión que informa `cmd [args...] --version`, sin leer de stdin.
tool_version() {
    "$@" --version </dev/null 2>/dev/null | extract_version
}

# --- verificación -----------------------------------------------------------

RG_MIN=13.0
FZF_MIN=0.36
LINUX_LIBS="xkbcommon xkbcommon-x11 wayland-client xcb x11-xcb fontconfig freetype2 vulkan"
LOCAL_BIN=${CREST_LOCAL_BIN:-$HOME/.local/bin}
CARGO_BIN=${CARGO_HOME:-$HOME/.cargo}/bin
ORIG_PATH=$PATH
MISSING=''        # ids de lo que falta (ver pkg_names)
VULKAN_DRIVER=0   # 1 si se encontró un ICD de Vulkan

# Los binarios que este script instala en ~/.local/bin o ~/.cargo/bin deben
# encontrarse aunque esos directorios aún no estén en el PATH del usuario.
PATH="$LOCAL_BIN:$CARGO_BIN:$PATH"
export PATH

report_ok() { printf '  %s[ok]%s    %-18s %s\n' "$C_GREEN" "$C_RESET" "$1" "$2"; }
report_warn() { printf '  %s[aviso]%s %-18s %s\n' "$C_YELLOW" "$C_RESET" "$1" "$2"; }
# report_missing ID ETIQUETA DETALLE
report_missing() {
    printf '  %s[falta]%s %-18s %s\n' "$C_RED" "$C_RESET" "$2" "$3"
    MISSING=$(list_add "$MISSING" "$1")
}

# Avisa si CMD se encontró en un directorio que añadió este script y el PATH
# del usuario no lo resuelve a ese mismo binario.
check_shadowed() {
    local cmd=$1 path=$2 user_path
    case $path in
        "$LOCAL_BIN"/* | "$CARGO_BIN"/*) ;;
        *) return 0 ;;
    esac
    user_path=$(PATH=$ORIG_PATH && command -v "$cmd" 2>/dev/null)
    if [ "$user_path" != "$path" ]; then
        report_warn "$cmd" "pon ${path%/*} al principio de tu PATH (hoy resuelve a ${user_path:-nada})"
    fi
}

# check_tool ID CMD ETIQUETA [VERSION_MINIMA]
check_tool() {
    local id=$1 cmd=$2 label=$3 min=${4:-} path ver
    if ! path=$(command -v "$cmd" 2>/dev/null); then
        report_missing "$id" "$label" "no se encuentra '$cmd'"
        return 1
    fi
    ver=$(tool_version "$cmd")
    if [ -n "$min" ] && { [ -z "$ver" ] || ! version_ge "$ver" "$min"; }; then
        report_missing "$id" "$label" "versión ${ver:-desconocida} en $path; se necesita >= $min"
        return 1
    fi
    report_ok "$label" "${ver:+$ver }($path)"
    check_shadowed "$cmd" "$path"
}

check_c_compiler() {
    if have cc; then
        check_tool cc cc "compilador C"
    elif have gcc; then
        check_tool cc gcc "compilador C"
    else
        report_missing cc "compilador C" "no se encuentra 'cc' ni 'gcc'"
    fi
}

check_libs() {
    local lib ver
    if ! have pkg-config; then
        for lib in $LINUX_LIBS; do
            report_missing "$lib" "lib $lib" "no se puede verificar sin pkg-config"
        done
        return 0
    fi
    for lib in $LINUX_LIBS; do
        if ver=$(pkg-config --modversion "$lib" 2>/dev/null); then
            report_ok "lib $lib" "$ver"
        else
            report_missing "$lib" "lib $lib" "pkg-config no encuentra $lib.pc"
        fi
    done
}

# El driver depende de la GPU y en CI suele no haberla: solo avisa.
check_vulkan_driver() {
    local dir f found=''
    for dir in /usr/share/vulkan/icd.d /etc/vulkan/icd.d /usr/local/share/vulkan/icd.d; do
        for f in "$dir"/*.json; do
            [ -e "$f" ] && found="${found:+$found, }${f##*/}"
        done
    done
    if [ -n "$found" ]; then
        VULKAN_DRIVER=1
        report_ok "driver Vulkan" "$found"
    else
        VULKAN_DRIVER=0
        report_warn "driver Vulkan" "no hay ICD en .../vulkan/icd.d; instala el driver de tu GPU (mesa o nvidia)"
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
        report_missing clippy clippy "'cargo clippy' no disponible"
    fi
    if have rustup; then
        report_ok rustup "$(tool_version rustup)"
    elif have cargo; then
        report_warn rustup "Rust no viene de rustup: rust-toolchain.toml no se aplicará"
    fi
}

# check_cargo_sub ID SUBCOMANDO
check_cargo_sub() {
    local ver
    if have cargo && ver=$(tool_version cargo "$2") && [ -n "$ver" ]; then
        report_ok "$1" "$ver"
    else
        report_missing "$1" "$1" "'cargo $2' no disponible"
    fi
}

run_checks() {
    MISSING=''
    info "Verificando dependencias"
    check_c_compiler
    check_tool pkg-config pkg-config pkg-config
    check_tool cmake cmake cmake
    check_tool clang clang clang
    check_tool git git git
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
}

# --- paquetes por gestor ----------------------------------------------------

# pkg_names GESTOR ID: paquetes que proveen ID (vacío si el gestor no lo
# provee; p. ej. Rust, que va por rustup). Los IDs son los de MISSING más
# curl, tar y gzip, que hacen falta para descargar rg, fzf y rustup.
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
            # El driver depende de la GPU (vulkan-radeon, vulkan-intel, nvidia-utils).
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
            # cc y clang vienen con las Command Line Tools.
            cmake | git | jq | fzf | curl) echo "$2" ;;
            pkg-config) echo pkgconf ;;
            rg) echo ripgrep ;;
        esac ;;
    esac
}

# Paquetes (sin repetir) del gestor actual para la lista de IDs $1.
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
        # shellcheck disable=SC2086 # SUDO puede ser "sudo -n"
        $SUDO "$@"
    else
        "$@"
    fi
}

# Actualiza los índices del gestor si hace falta antes de instalar.
pm_refresh() {
    case $PKG_MGR in
        apt) as_root apt-get update ;;
        apk) as_root apk update ;;
        xbps) as_root xbps-install -S ;;
        pacman)
            # Sin bases sincronizadas (imagen recién creada) -Sy solo dejaría
            # una actualización parcial; Arch exige -Syu.
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

# Instala la lista $1 en bloque; si falla (p. ej. un nombre que no existe en
# esta versión de la distro), reintenta uno por uno y reporta los que fallan.
install_packages() {
    local pkgs=$1 p failed=''
    [ -n "$pkgs" ] || return 0
    info "Instalando con $PKG_MGR: $pkgs"
    pm_refresh || warn "no se pudieron actualizar los índices de $PKG_MGR"
    # shellcheck disable=SC2086 # lista de paquetes separada por espacios
    if pm_install $pkgs; then
        return 0
    fi
    warn "la instalación en bloque falló; reintentando paquete por paquete"
    for p in $pkgs; do
        pm_install "$p" || failed=$(list_add "$failed" "$p")
    done
    if [ -n "$failed" ]; then
        err "no se pudieron instalar: $failed"
        return 1
    fi
}

# confirm PREGUNTA: éxito si el usuario acepta (o con --yes).
confirm() {
    local ans
    [ "$OPT_YES" = 1 ] && return 0
    if [ ! -t 0 ]; then
        err "no hay terminal para confirmar; usa --yes para instalar sin preguntar"
        return 1
    fi
    printf '%s [s/N] ' "$1"
    read -r ans || return 1
    case $ans in
        s | S | si | sí | Si | Sí | y | Y | yes) return 0 ;;
        *) return 1 ;;
    esac
}

# Resume lo que falta. Éxito si no falta nada.
print_summary() {
    local pkgs
    if [ -z "$MISSING" ]; then
        info "${C_GREEN}Todas las dependencias están presentes.${C_RESET}"
        return 0
    fi
    err "faltan $(list_count "$MISSING") dependencias: $MISSING"
    pkgs=$(packages_for "$MISSING")
    if [ -n "$pkgs" ]; then
        printf 'Paquetes de %s que las proveen: %s\n' "$PKG_MGR" "$pkgs"
    fi
    printf 'Para instalarlas: %s%s%s\n' "$0" \
        "$([ "$OPT_DEV" = 1 ] && printf ' --dev')" \
        "$([ "$OPT_NO_RUST" = 1 ] && printf ' --no-rust')"
    return 1
}

# Herramientas que el propio instalador necesita para descargar.
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
    if [ "$PKG_MGR" = none ] && [ -n "$ids" ]; then
        warn "no se encontró un gestor de paquetes soportado; instala a mano: $ids"
    fi
    info "Plan de instalación"
    [ -n "$pkgs" ] && printf '  paquetes (%s): %s\n' "$PKG_MGR" "$pkgs"
    confirm "¿Continuar?" || die "instalación cancelada"
    [ -n "$pkgs" ] && setup_sudo
    install_packages "$pkgs" || true
}

main() {
    parse_args "$@"
    detect_system
    print_system
    if [ "$PLATFORM" = unknown ]; then
        warn "sistema no soportado: $(uname -s); solo se verificará"
        OPT_CHECK=1
    fi
    # Al verificar, rustup no debe descargar el toolchain de rust-toolchain.toml.
    if [ "$OPT_CHECK" = 1 ]; then
        RUSTUP_AUTO_INSTALL=0
        export RUSTUP_AUTO_INSTALL
    fi
    # Desde la raíz del repo, para que rustup aplique su rust-toolchain.toml.
    cd "$(dirname "$0")/.." || die "no se puede entrar a la raíz del repo"
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

# Con CREST_INSTALL_DEPS_LIB=1 el script solo define funciones (para tests).
if [ "${CREST_INSTALL_DEPS_LIB:-0}" != 1 ]; then
    main "$@"
fi
