#!/usr/bin/env bash
# Prueba scripts/install-deps.sh en contenedores (docker o podman).
#
#   tests/install-deps/run-containers.sh                 # --check en las imágenes por defecto
#   tests/install-deps/run-containers.sh --install IMG   # instala con --yes --no-rust y vuelve a verificar
#
# --check en una imagen limpia debe salir con 1, listar lo que falta y no
# quedarse esperando entrada. Tras --install, --check --no-rust debe salir con 0.

set -u

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
ENGINE=${CONTAINER_ENGINE:-docker}
TIMEOUT=${CHECK_TIMEOUT:-120}
DEFAULT_IMAGES="ubuntu:24.04 ubuntu:22.04 fedora:latest archlinux:latest"
FAILS=0

run() { # imagen, comando dentro del contenedor
    "$ENGINE" run --rm -i -v "$ROOT:/src:ro" "$1" sh -c "$2" </dev/null
}

check_image() {
    local img=$1 out rc
    echo "== $img: --check"
    out=$(timeout "$TIMEOUT" "$ENGINE" run --rm -v "$ROOT:/src:ro" "$img" \
        bash /src/scripts/install-deps.sh --check </dev/null 2>&1)
    rc=$?
    printf '%s\n' "$out" | sed 's/^/   /'
    case $rc in
        1) case $out in
               *"faltan "*) echo "   ok: sale con 1 y lista lo que falta" ;;
               *) echo "   FALLA: sale con 1 pero no lista lo que falta"; FAILS=$((FAILS + 1)) ;;
           esac ;;
        124) echo "   FALLA: se colgó (más de ${TIMEOUT}s)"; FAILS=$((FAILS + 1)) ;;
        *) echo "   FALLA: código $rc, se esperaba 1"; FAILS=$((FAILS + 1)) ;;
    esac
}

install_image() {
    local img=$1 rc
    echo "== $img: --yes --no-rust y luego --check --no-rust"
    # shellcheck disable=SC2016 # se expande dentro del contenedor
    run "$img" 'bash /src/scripts/install-deps.sh --yes --no-rust >/tmp/install.log 2>&1 || { tail -n 40 /tmp/install.log; exit 3; }
        PATH="$HOME/.local/bin:$PATH" bash /src/scripts/install-deps.sh --check --no-rust'
    rc=$?
    if [ "$rc" = 0 ]; then
        echo "   ok: todo presente tras instalar"
    else
        echo "   FALLA: código $rc"
        FAILS=$((FAILS + 1))
    fi
}

if [ "${1:-}" = --install ]; then
    shift
    [ $# -gt 0 ] || set -- ubuntu:24.04
    for img in "$@"; do install_image "$img"; done
else
    # shellcheck disable=SC2086 # lista de imágenes separada por espacios
    [ $# -gt 0 ] || set -- $DEFAULT_IMAGES
    for img in "$@"; do check_image "$img"; done
fi

echo "== $FAILS fallos"
[ "$FAILS" = 0 ]
