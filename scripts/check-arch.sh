#!/usr/bin/env bash
# check-arch.sh: verifica las reglas de dependencia de la arquitectura hexagonal.
#
# Capas (por la ruta del manifest relativa a la raíz del workspace):
#   domain   crates/domain
#   app      crates/app            (el paquete debe llamarse "application")
#   adapter  crates/adapters/<x>
#   ui       crates/ui
#   bin      crates/bin
#
# Uso: scripts/check-arch.sh [--manifest-path <Cargo.toml>]
#      (o la variable de entorno CHECK_ARCH_MANIFEST)
# Sale con 0 si todo cumple, 1 si hay alguna violación, 2 si hay un error de uso.
# Requiere cargo y jq. Compatible con bash 3.2.
set -euo pipefail

manifest="${CHECK_ARCH_MANIFEST:-}"

usage() {
    echo "uso: $0 [--manifest-path <Cargo.toml>]" >&2
}

while [ $# -gt 0 ]; do
    case "$1" in
        --manifest-path)
            if [ $# -lt 2 ]; then
                usage
                exit 2
            fi
            manifest="$2"
            shift 2
            ;;
        --manifest-path=*)
            manifest="${1#--manifest-path=}"
            shift
            ;;
        -h | --help)
            usage
            exit 0
            ;;
        *)
            usage
            exit 2
            ;;
    esac
done

for tool in cargo jq; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "check-arch: falta '$tool' en el PATH" >&2
        exit 2
    fi
done

metadata_args=(metadata --format-version 1 --no-deps --offline)
if [ -n "$manifest" ]; then
    metadata_args+=(--manifest-path "$manifest")
fi

metadata="$(cargo "${metadata_args[@]}")"
root="$(printf '%s' "$metadata" | jq -r '.workspace_root')"

errors=0

fail() {
    echo "check-arch: $*" >&2
    errors=$((errors + 1))
}

# Imprime la capa de un directorio (absoluto) o "unknown" si no es clasificable.
layer_of() {
    local rel="${1#"$root"/}"
    case "$rel" in
        crates/domain) echo domain ;;
        crates/app) echo app ;;
        crates/adapters/*/*) echo unknown ;;
        crates/adapters/*) echo adapter ;;
        crates/ui) echo ui ;;
        crates/bin) echo bin ;;
        *) echo unknown ;;
    esac
}

# Una línea por paquete del workspace: nombre<TAB>directorio.
packages="$(printf '%s' "$metadata" | jq -r '
    .packages[] | [.name, (.manifest_path | sub("/Cargo.toml$"; ""))] | @tsv')"

while IFS="$(printf '\t')" read -r name dir; do
    [ -n "$name" ] || continue
    layer="$(layer_of "$dir")"
    if [ "$layer" = unknown ]; then
        fail "crate $name (${dir#"$root"/}) no pertenece a ninguna capa conocida"
    fi
done <<EOF
$packages
EOF

if [ "$errors" -gt 0 ]; then
    echo "check-arch: $errors violación(es) de arquitectura" >&2
    exit 1
fi
echo "check-arch: ok"
