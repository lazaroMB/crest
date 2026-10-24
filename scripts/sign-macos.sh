#!/usr/bin/env bash
# Signs crest.app with a Developer ID, notarizes it with Apple and staples
# the ticket, with rcodesign (apple-codesign): no keychain and no Xcode, so
# it also runs on Linux.
#
#   APPLE_CERT_P12_BASE64=... APPLE_CERT_PASSWORD=... APPLE_API_KEY_JSON=... \
#       scripts/sign-macos.sh path/to/crest.app
#
# - APPLE_CERT_P12_BASE64: the "Developer ID Application" certificate and
#   its key, exported from Keychain Access as .p12, in base64.
# - APPLE_CERT_PASSWORD: the password of that .p12.
# - APPLE_API_KEY_JSON: an App Store Connect API key (Developer role) as
#   `rcodesign encode-app-store-connect-api-key -o key.json <issuer> <key id>
#   AuthKey_<key id>.p8` writes it.
#
# Called by scripts/package.sh --sign. The hardened runtime is on
# (--for-notarization) and crest needs no entitlement: it is not
# sandboxed, Lua has no JIT and the keychain works without one.
#
# Compatible with Bash 3.2 (macOS).

set -euo pipefail

RCODESIGN_VERSION=0.29.0
# From the release's SHA256SUMS.
RCODESIGN_SHA_MACOS=d98372d5524226ccf9dc0eda03d4e4f5826182dabb2fc3f2bd303ed9113a748d
RCODESIGN_SHA_LINUX=dbe85cedd8ee4217b64e9a0e4c2aef92ab8bcaaa41f20bde99781ff02e600002

die() {
    echo "sign-macos.sh: $*" >&2
    exit 1
}

[ $# -eq 1 ] || die "usage: scripts/sign-macos.sh <crest.app>"
app="$1"
[ -d "$app/Contents/MacOS" ] || die "$app is not an app bundle"
for var in APPLE_CERT_P12_BASE64 APPLE_CERT_PASSWORD APPLE_API_KEY_JSON; do
    [ -n "${!var:-}" ] || die "$var is not set"
done

work="$(mktemp -d "${TMPDIR:-/tmp}/crest-sign.XXXXXX")"
trap 'rm -rf "$work"' EXIT

case "$(uname -s)/$(uname -m)" in
    Darwin/*) asset=macos-universal sha=$RCODESIGN_SHA_MACOS ;;
    Linux/x86_64) asset=x86_64-unknown-linux-musl sha=$RCODESIGN_SHA_LINUX ;;
    *) die "no rcodesign build pinned for $(uname -s) $(uname -m)" ;;
esac
name="apple-codesign-$RCODESIGN_VERSION-$asset"
curl -fsSL --proto '=https' --tlsv1.2 -o "$work/$name.tar.gz" \
    "https://github.com/indygreg/apple-platform-rs/releases/download/apple-codesign%2F$RCODESIGN_VERSION/$name.tar.gz"
echo "$sha  $work/$name.tar.gz" | shasum -a 256 -c - >/dev/null || die "rcodesign checksum mismatch"
tar -xzf "$work/$name.tar.gz" -C "$work"
rcodesign="$work/$name/rcodesign"

# Secrets go to files only readable by us, never on the command line.
umask 077
printf '%s' "$APPLE_CERT_P12_BASE64" | base64 --decode >"$work/cert.p12"
printf '%s' "$APPLE_CERT_PASSWORD" >"$work/cert.pass"
printf '%s' "$APPLE_API_KEY_JSON" >"$work/api-key.json"

"$rcodesign" sign --for-notarization \
    --p12-file "$work/cert.p12" --p12-password-file "$work/cert.pass" \
    "$app"
"$rcodesign" notary-submit --api-key-file "$work/api-key.json" \
    --wait --max-wait-seconds 1800 --staple "$app"

echo "sign-macos.sh: $app signed, notarized and stapled"
