#!/usr/bin/env bash
# Tests de scripts/check-arch.sh contra workspaces de juguete en fixtures/.
#
# Los fixtures no tienen dependencias externas reales: los crates de I/O de la
# lista negra (tokio, opentelemetry-sdk) son crates vacíos en fixtures/_vendor,
# referenciados por path desde fuera de cada workspace de prueba, así que
# cargo metadata los ve como externos y no se descarga nada. La lista negra
# compara por nombre de paquete, de modo que basta con el nombre.
#
# Uso: tests/arch/run.sh   (sale con 1 si algún caso no da lo esperado)
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
script="$repo/scripts/check-arch.sh"
fixtures="$here/fixtures"

passed=0
failed=0

# check <nombre> <código esperado> <texto esperado en la salida> <args...>
check() {
    local label="$1" want="$2" needle="$3" out code
    shift 3
    set +e
    out="$("$@" 2>&1)"
    code=$?
    set -e
    if [ "$code" -ne "$want" ]; then
        echo "FAIL $label: código $code, se esperaba $want"
        printf '%s\n' "$out" | sed 's/^/    /'
        failed=$((failed + 1))
    elif [ -n "$needle" ] && ! printf '%s\n' "$out" | grep -qF -- "$needle"; then
        echo "FAIL $label: la salida no contiene '$needle'"
        printf '%s\n' "$out" | sed 's/^/    /'
        failed=$((failed + 1))
    else
        echo "ok   $label"
        passed=$((passed + 1))
    fi
}

# fixture <nombre> <código esperado> <texto esperado>
fixture() {
    check "$1" "$2" "$3" "$script" --manifest-path "$fixtures/$1/Cargo.toml"
}

fixture valid 0 "check-arch: ok"
fixture adapter-to-adapter 1 "crate adapter-fs (capa adapter) depende de adapter-shell (capa adapter) [normal]: prohibido"
fixture adapter-to-adapter-dev 1 "depende de adapter-shell (capa adapter) [dev]: prohibido"
fixture domain-to-application 1 "crate domain (capa domain) depende de application (capa app)"
fixture app-to-adapter 1 "crate application (capa app) depende de adapter-fs (capa adapter)"
fixture ui-to-adapter 1 "crate ui (capa ui) depende de adapter-fs (capa adapter)"
fixture app-io-dep 1 "crate application (capa app) depende del crate de I/O tokio"
fixture domain-io-glob 1 "crate domain (capa domain) depende del crate de I/O opentelemetry-sdk"
fixture app-misnamed 1 "crate app (capa app) debe llamarse 'application'"
fixture unknown-path 1 "crate misc (crates/misc) no pertenece a ninguna capa conocida"

check "env CHECK_ARCH_MANIFEST" 1 "debe llamarse 'application'" \
    env CHECK_ARCH_MANIFEST="$fixtures/app-misnamed/Cargo.toml" "$script"
check "argumento desconocido" 2 "uso:" "$script" --bogus
check "workspace real" 0 "check-arch: ok" "$script" --manifest-path "$repo/Cargo.toml"

echo "$passed ok, $failed fallidos"
[ "$failed" -eq 0 ]
