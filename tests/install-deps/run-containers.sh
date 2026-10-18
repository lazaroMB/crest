#!/usr/bin/env bash
# Tests scripts/install-deps.sh in containers (docker or podman).
#
#   tests/install-deps/run-containers.sh                 # --check on the default images
#   tests/install-deps/run-containers.sh --install IMG   # installs with --yes --no-rust and checks again
#
# --check on a clean image must exit 1, list what is missing and not wait for
# input. After --install, --check --no-rust must exit 0.

set -u

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
ENGINE=${CONTAINER_ENGINE:-docker}
TIMEOUT=${CHECK_TIMEOUT:-120}
DEFAULT_IMAGES="ubuntu:24.04 ubuntu:22.04 fedora:latest archlinux:latest"
FAILS=0

run() { # image, command inside the container
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
               *"missing "*) echo "   ok: exits 1 and lists what is missing" ;;
               *) echo "   FAIL: exits 1 but does not list what is missing"; FAILS=$((FAILS + 1)) ;;
           esac ;;
        124) echo "   FAIL: hung (more than ${TIMEOUT}s)"; FAILS=$((FAILS + 1)) ;;
        *) echo "   FAIL: exit code $rc, expected 1"; FAILS=$((FAILS + 1)) ;;
    esac
}

install_image() {
    local img=$1 rc
    echo "== $img: --yes --no-rust, then --check --no-rust"
    # shellcheck disable=SC2016 # expanded inside the container
    run "$img" 'bash /src/scripts/install-deps.sh --yes --no-rust >/tmp/install.log 2>&1 || { tail -n 40 /tmp/install.log; exit 3; }
        PATH="$HOME/.local/bin:$PATH" bash /src/scripts/install-deps.sh --check --no-rust'
    rc=$?
    if [ "$rc" = 0 ]; then
        echo "   ok: everything present after installing"
    else
        echo "   FAIL: exit code $rc"
        FAILS=$((FAILS + 1))
    fi
}

if [ "${1:-}" = --install ]; then
    shift
    [ $# -gt 0 ] || set -- ubuntu:24.04
    for img in "$@"; do install_image "$img"; done
else
    # shellcheck disable=SC2086 # space-separated image list
    [ $# -gt 0 ] || set -- $DEFAULT_IMAGES
    for img in "$@"; do check_image "$img"; done
fi

echo "== $FAILS failures"
[ "$FAILS" = 0 ]
