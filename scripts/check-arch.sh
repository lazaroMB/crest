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
# Reglas de dependencias internas (crates con path dentro del workspace):
#   domain -> ninguna; app -> domain; adapter, ui -> app, domain; bin -> todas.
# domain y app tampoco pueden usar crates de I/O (ver IO_CRATES).
#
# Sale con 0 si todo cumple, 1 si hay alguna violación, 2 si hay un error de uso.
# Requiere cargo y jq. Compatible con bash 3.2.
set -euo pipefail
set -f # sin globbing de rutas: IO_CRATES contiene patrones como opentelemetry*

# Crates de I/O prohibidos en [dependencies] y [build-dependencies] de domain y
# app (se compara el nombre del paquete; admite globs). En [dev-dependencies] sí
# se permiten: los tests pueden usar un runtime o un fs real sin que el código
# de producción quede acoplado a ellos. futures, async-trait, tracing (fachada)
# y similar son puros y están permitidos.
IO_CRATES="tokio reqwest reqwest-eventsource hyper gpui gpui-component mlua
rusqlite sqlx rmcp notify keyring opentelemetry* tracing-opentelemetry"

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
    elif [ "$layer" = app ] && [ "$name" != application ]; then
        fail "crate $name (capa app) debe llamarse 'application'"
    fi
done <<EOF
$packages
EOF

# ¿Puede la capa $1 depender de la capa $2?
internal_allowed() {
    case "$1:$2" in
        app:domain) return 0 ;;
        adapter:app | adapter:domain) return 0 ;;
        ui:app | ui:domain) return 0 ;;
        bin:*) return 0 ;;
        *) return 1 ;;
    esac
}

# Una línea por dependencia con path: crate<TAB>dir<TAB>dep<TAB>dir de dep<TAB>tipo.
# Las reglas internas se aplican a todos los tipos (normal, dev y build).
path_deps="$(printf '%s' "$metadata" | jq -r '
    .packages[] | .name as $n | (.manifest_path | sub("/Cargo.toml$"; "")) as $d
    | .dependencies[] | select(.path != null)
    | [$n, $d, .name, .path, (.kind // "normal")] | @tsv')"

while IFS="$(printf '\t')" read -r name dir dep dep_dir kind; do
    [ -n "$name" ] || continue
    case "$dep_dir" in
        "$root" | "$root"/*) ;;
        *) continue ;; # path fuera del workspace: no es interna
    esac
    from="$(layer_of "$dir")"
    to="$(layer_of "$dep_dir")"
    [ "$from" != unknown ] || continue # ya reportado arriba
    if [ "$to" = unknown ]; then
        fail "crate $name (capa $from) depende de $dep (${dep_dir#"$root"/}), que no pertenece a ninguna capa conocida"
    elif ! internal_allowed "$from" "$to"; then
        fail "crate $name (capa $from) depende de $dep (capa $to) [$kind]: prohibido"
    fi
done <<EOF
$path_deps
EOF

is_io_crate() {
    local pattern
    for pattern in $IO_CRATES; do
        # shellcheck disable=SC2254 # el patrón es un glob a propósito
        case "$1" in
            $pattern) return 0 ;;
        esac
    done
    return 1
}

# Una línea por dependencia no-dev: crate<TAB>dir<TAB>dep<TAB>tipo.
non_dev_deps="$(printf '%s' "$metadata" | jq -r '
    .packages[] | .name as $n | (.manifest_path | sub("/Cargo.toml$"; "")) as $d
    | .dependencies[] | select(.kind != "dev")
    | [$n, $d, .name, (.kind // "normal")] | @tsv')"

while IFS="$(printf '\t')" read -r name dir dep kind; do
    [ -n "$name" ] || continue
    from="$(layer_of "$dir")"
    case "$from" in
        domain | app) ;;
        *) continue ;;
    esac
    if is_io_crate "$dep"; then
        fail "crate $name (capa $from) depende del crate de I/O $dep [$kind]: prohibido"
    fi
done <<EOF
$non_dev_deps
EOF

if [ "$errors" -gt 0 ]; then
    echo "check-arch: $errors violación(es) de arquitectura" >&2
    exit 1
fi
echo "check-arch: ok"
