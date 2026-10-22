#!/usr/bin/env bash
# check-arch.sh: checks the dependency rules of the hexagonal architecture.
#
# Layers (by the manifest path relative to the workspace root):
#   domain   crates/domain
#   app      crates/app            (the package must be named "application")
#   adapter  crates/adapters/<x>
#   ui       crates/ui
#   bin      crates/bin
#   platform crates/platform (OS glue: processes, directories, console)
#
# Usage: scripts/check-arch.sh [--manifest-path <Cargo.toml>]
#      (or the CHECK_ARCH_MANIFEST environment variable)
# Internal dependency rules (path crates inside the workspace):
#   domain -> none; app -> domain; adapter, ui -> app, domain, platform;
#   platform -> none; bin -> all.
# domain and app cannot use I/O crates either (see IO_CRATES).
#
# Exits 0 if everything complies, 1 on any violation, 2 on a usage error.
# Requires cargo and jq. Compatible with bash 3.2.
set -euo pipefail
set -f # no pathname globbing: IO_CRATES holds patterns such as opentelemetry*

# I/O crates forbidden in the [dependencies] and [build-dependencies] of domain
# and app (matched by package name; globs allowed). They are allowed in
# [dev-dependencies]: tests may use a runtime or a real fs without coupling the
# production code to them. futures, async-trait, tracing (the facade) and the
# like are pure and allowed.
IO_CRATES="tokio reqwest reqwest-eventsource hyper gpui gpui-component mlua
rusqlite sqlx rmcp notify keyring opentelemetry* tracing-opentelemetry"

manifest="${CHECK_ARCH_MANIFEST:-}"

usage() {
    echo "usage: $0 [--manifest-path <Cargo.toml>]" >&2
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
        echo "check-arch: '$tool' is not in the PATH" >&2
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

# Prints the layer of an (absolute) directory, or "unknown" if it fits none.
layer_of() {
    local rel="${1#"$root"/}"
    case "$rel" in
        crates/domain) echo domain ;;
        crates/app) echo app ;;
        crates/adapters/*/*) echo unknown ;;
        crates/adapters/*) echo adapter ;;
        crates/ui) echo ui ;;
        crates/bin) echo bin ;;
        crates/platform) echo platform ;;
        *) echo unknown ;;
    esac
}

# One line per workspace package: name<TAB>directory.
packages="$(printf '%s' "$metadata" | jq -r '
    .packages[] | [.name, (.manifest_path | sub("/Cargo.toml$"; ""))] | @tsv')"

while IFS="$(printf '\t')" read -r name dir; do
    [ -n "$name" ] || continue
    layer="$(layer_of "$dir")"
    if [ "$layer" = unknown ]; then
        fail "crate $name (${dir#"$root"/}) does not belong to any known layer"
    elif [ "$layer" = app ] && [ "$name" != application ]; then
        fail "crate $name (layer app) must be named 'application'"
    fi
done <<EOF
$packages
EOF

# May layer $1 depend on layer $2?
internal_allowed() {
    case "$1:$2" in
        app:domain) return 0 ;;
        adapter:app | adapter:domain | adapter:platform) return 0 ;;
        ui:app | ui:domain | ui:platform) return 0 ;;
        bin:*) return 0 ;;
        *) return 1 ;;
    esac
}

# One line per path dependency: crate<TAB>dir<TAB>dep<TAB>dep dir<TAB>kind.
# The internal rules apply to every kind (normal, dev and build).
path_deps="$(printf '%s' "$metadata" | jq -r '
    .packages[] | .name as $n | (.manifest_path | sub("/Cargo.toml$"; "")) as $d
    | .dependencies[] | select(.path != null)
    | [$n, $d, .name, .path, (.kind // "normal")] | @tsv')"

while IFS="$(printf '\t')" read -r name dir dep dep_dir kind; do
    [ -n "$name" ] || continue
    case "$dep_dir" in
        "$root" | "$root"/*) ;;
        *) continue ;; # path outside the workspace: not internal
    esac
    from="$(layer_of "$dir")"
    to="$(layer_of "$dep_dir")"
    [ "$from" != unknown ] || continue # already reported above
    if [ "$to" = unknown ]; then
        fail "crate $name (layer $from) depends on $dep (${dep_dir#"$root"/}), which does not belong to any known layer"
    elif ! internal_allowed "$from" "$to"; then
        fail "crate $name (layer $from) depends on $dep (layer $to) [$kind]: forbidden"
    fi
done <<EOF
$path_deps
EOF

is_io_crate() {
    local pattern
    for pattern in $IO_CRATES; do
        # shellcheck disable=SC2254 # the pattern is a glob on purpose
        case "$1" in
            $pattern) return 0 ;;
        esac
    done
    return 1
}

# One line per non-dev dependency: crate<TAB>dir<TAB>dep<TAB>kind.
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
        fail "crate $name (layer $from) depends on the I/O crate $dep [$kind]: forbidden"
    fi
done <<EOF
$non_dev_deps
EOF

if [ "$errors" -gt 0 ]; then
    echo "check-arch: $errors architecture violation(s)" >&2
    exit 1
fi
echo "check-arch: ok"
