#!/usr/bin/env bash
# Arma el tarball de release de crest a partir de un binario ya compilado.
#
#   make release && scripts/package.sh
#   cargo build --release --locked --target <t> -p crest
#   scripts/package.sh --target <t>
#
# Produce <out>/crest-<version>-<target>.tar.gz (un directorio con el
# binario, README.txt con los requisitos de runtime y LICENSE* si existen)
# y su checksum <archivo>.sha256 en el formato de `sha256sum -c`.
#
# Compatible con Bash 3.2 (macOS).

set -euo pipefail

BIN=crest
target=""
version=""
out="dist"

usage() {
    cat <<'EOF'
Uso: scripts/package.sh [--target <triple>] [--version <v>] [--out <dir>]

  --target   Triple de Rust. Con él se empaqueta target/<triple>/release/crest;
             sin él, target/release/crest y el triple del host (rustc -vV).
  --version  Versión del nombre del archivo (por defecto, la del paquete crest).
  --out      Directorio de salida (por defecto, dist).
EOF
}

die() {
    echo "package.sh: $*" >&2
    exit 1
}

while [ $# -gt 0 ]; do
    case "$1" in
        --target) [ $# -ge 2 ] || die "falta el valor de --target"; target="$2"; shift 2 ;;
        --version) [ $# -ge 2 ] || die "falta el valor de --version"; version="$2"; shift 2 ;;
        --out) [ $# -ge 2 ] || die "falta el valor de --out"; out="$2"; shift 2 ;;
        -h | --help) usage; exit 0 ;;
        *) usage >&2; die "argumento desconocido: $1" ;;
    esac
done

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

# CARGO_TARGET_DIR respeta un target/ fuera del repo.
target_dir="${CARGO_TARGET_DIR:-target}"
if [ -n "$target" ]; then
    bin_path="$target_dir/$target/release/$BIN"
else
    bin_path="$target_dir/release/$BIN"
    target="$(rustc -vV | sed -n 's/^host: //p')"
    [ -n "$target" ] || die "no se pudo leer el triple del host con rustc -vV"
fi
[ -x "$bin_path" ] || die "no existe $bin_path; compílalo antes (make release)"

if [ -z "$version" ]; then
    # `cargo pkgid` imprime ...#crest@0.1.0 (o ...#0.1.0 en cargos viejos).
    pkgid="$(cargo pkgid -p "$BIN")"
    version="${pkgid##*[#@]}"
    [ -n "$version" ] || die "no se pudo leer la versión de $pkgid"
fi

name="$BIN-$version-$target"
mkdir -p "$out"
out="$(cd "$out" && pwd)"
staging="$(mktemp -d "${TMPDIR:-/tmp}/crest-package.XXXXXX")"
trap 'rm -rf "$staging"' EXIT
mkdir "$staging/$name"

cp "$bin_path" "$staging/$name/$BIN"
for license in LICENSE*; do
    if [ -f "$license" ]; then
        cp "$license" "$staging/$name/"
    fi
done

case "$target" in
    *-apple-darwin)
        platform_notes="macOS
-----
- El binario no está firmado ni notarizado. Si Gatekeeper lo bloquea:
    xattr -d com.apple.quarantine ./crest
- rg y fzf: brew install ripgrep fzf"
        ;;
    *)
        platform_notes="Linux
-----
- Vulkan: el loader (libvulkan1 / vulkan-loader) y el driver de la GPU
  (mesa-vulkan-drivers o el del fabricante).
- Bibliotecas de GPUI: libxkbcommon, libxkbcommon-x11, wayland, libxcb,
  libx11-xcb, fontconfig y freetype (paquetes de la distro).
- glibc igual o más nueva que la del sistema donde se compiló; los
  tarballs del CI se compilan en Ubuntu 22.04 (glibc 2.35)."
        ;;
esac

cat >"$staging/$name/README.txt" <<EOF
crest $version ($target)

Instalación: copia \`crest\` a un directorio del PATH, por ejemplo ~/.local/bin.

Requisitos de runtime
---------------------
- ripgrep (rg) 13.0 o más nuevo.
- fzf 0.36 o más nuevo.
- git.
SQLite y TLS (rustls) van dentro del binario: no hace falta libsqlite3 ni
OpenSSL del sistema.

$platform_notes

Desde el repositorio, scripts/install-deps.sh instala o verifica (--check)
estas dependencias.
EOF

tarball="$name.tar.gz"
tar -C "$staging" -czf "$out/$tarball" "$name"

if command -v sha256sum >/dev/null 2>&1; then
    (cd "$out" && sha256sum "$tarball" >"$tarball.sha256")
else
    (cd "$out" && shasum -a 256 "$tarball" >"$tarball.sha256")
fi

echo "$out/$tarball"
