#!/usr/bin/env bash
# Tests for scripts/check-arch.sh against toy workspaces in fixtures/.
#
# The fixtures have no real external dependencies: the blacklisted I/O crates
# (tokio, opentelemetry-sdk) are empty crates in fixtures/_vendor, referenced
# by path from outside each test workspace, so cargo metadata sees them as
# external and nothing is downloaded. The blacklist matches by package name,
# so the name is enough.
#
# Usage: tests/arch/run.sh   (exits 1 if any case does not give the expected result)
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
script="$repo/scripts/check-arch.sh"
fixtures="$here/fixtures"

passed=0
failed=0

# check <name> <expected code> <expected text in the output> <args...>
check() {
    local label="$1" want="$2" needle="$3" out code
    shift 3
    set +e
    out="$("$@" 2>&1)"
    code=$?
    set -e
    if [ "$code" -ne "$want" ]; then
        echo "FAIL $label: exit code $code, expected $want"
        printf '%s\n' "$out" | sed 's/^/    /'
        failed=$((failed + 1))
    elif [ -n "$needle" ] && ! printf '%s\n' "$out" | grep -qF -- "$needle"; then
        echo "FAIL $label: the output does not contain '$needle'"
        printf '%s\n' "$out" | sed 's/^/    /'
        failed=$((failed + 1))
    else
        echo "ok   $label"
        passed=$((passed + 1))
    fi
}

# fixture <name> <expected code> <expected text>
fixture() {
    check "$1" "$2" "$3" "$script" --manifest-path "$fixtures/$1/Cargo.toml"
}

fixture valid 0 "check-arch: ok"
fixture adapter-to-adapter 1 "crate adapter-fs (layer adapter) depends on adapter-shell (layer adapter) [normal]: forbidden"
fixture adapter-to-adapter-dev 1 "depends on adapter-shell (layer adapter) [dev]: forbidden"
fixture domain-to-application 1 "crate domain (layer domain) depends on application (layer app)"
fixture app-to-adapter 1 "crate application (layer app) depends on adapter-fs (layer adapter)"
fixture ui-to-adapter 1 "crate ui (layer ui) depends on adapter-fs (layer adapter)"
fixture app-io-dep 1 "crate application (layer app) depends on the I/O crate tokio"
fixture domain-io-glob 1 "crate domain (layer domain) depends on the I/O crate opentelemetry-sdk"
fixture app-misnamed 1 "crate app (layer app) must be named 'application'"
fixture unknown-path 1 "crate misc (crates/misc) does not belong to any known layer"

check "env CHECK_ARCH_MANIFEST" 1 "must be named 'application'" \
    env CHECK_ARCH_MANIFEST="$fixtures/app-misnamed/Cargo.toml" "$script"
check "unknown argument" 2 "usage:" "$script" --bogus
check "real workspace" 0 "check-arch: ok" "$script" --manifest-path "$repo/Cargo.toml"

echo "$passed ok, $failed failed"
[ "$failed" -eq 0 ]
