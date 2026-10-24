#!/usr/bin/env bash
# Publishes the crest cask to the lazaroMB/homebrew-tap tap, so that
# `brew install --cask lazaromb/tap/crest` installs the release.
#
#   PACKAGES_TOKEN=... scripts/publish-homebrew.sh <version> <dist dir>
#
# <dist dir> holds the release's crest-<version>-<arch>-apple-darwin.tar.gz.sha256
# files; the cask (packaging/homebrew/crest.rb.in with the version and the
# checksums) is committed as Casks/crest.rb and pushed with PACKAGES_TOKEN,
# a token that can push to the tap. With --dry-run it only prints the cask.
#
# Run by the `homebrew` job of release.yml after the GitHub Release.

set -euo pipefail

TAP="lazaroMB/homebrew-tap"

die() {
    echo "publish-homebrew.sh: $*" >&2
    exit 1
}

dry_run=0
if [ "${1:-}" = --dry-run ]; then
    dry_run=1
    shift
fi
[ $# -eq 2 ] || die "usage: scripts/publish-homebrew.sh [--dry-run] <version> <dist dir>"
version="${1#v}"
dist="$2"
root="$(cd "$(dirname "$0")/.." && pwd)"

sha_of() { # arch
    local file="$dist/crest-$version-$1-apple-darwin.tar.gz.sha256"
    [ -f "$file" ] || die "$file does not exist"
    cut -d ' ' -f 1 "$file"
}

cask="$(sed -e "s/@VERSION@/$version/" \
    -e "s/@SHA256_ARM@/$(sha_of aarch64)/" \
    -e "s/@SHA256_INTEL@/$(sha_of x86_64)/" \
    "$root/packaging/homebrew/crest.rb.in")"

if [ "$dry_run" = 1 ]; then
    printf '%s\n' "$cask"
    exit 0
fi
[ -n "${PACKAGES_TOKEN:-}" ] || die "PACKAGES_TOKEN is not set"

work="$(mktemp -d "${TMPDIR:-/tmp}/crest-tap.XXXXXX")"
trap 'rm -rf "$work"' EXIT
git clone --quiet --depth 1 "https://x-access-token:$PACKAGES_TOKEN@github.com/$TAP.git" "$work/tap"
mkdir -p "$work/tap/Casks"
printf '%s\n' "$cask" >"$work/tap/Casks/crest.rb"
cd "$work/tap"
git add Casks/crest.rb
if git diff --cached --quiet; then
    echo "publish-homebrew.sh: the tap already has crest $version"
    exit 0
fi
git -c user.name="github-actions[bot]" \
    -c user.email="41898282+github-actions[bot]@users.noreply.github.com" \
    commit --quiet -m "crest $version"
git push --quiet origin HEAD
echo "publish-homebrew.sh: pushed crest $version to $TAP"
