#!/usr/bin/env bash
# Simulates macOS with fake commands (uname, xcode-select, xcrun, brew...) to
# test the macOS paths of install-deps.sh --check on Linux, ideally with
# Bash 3.2:  docker run --rm -v "$PWD:/src:ro" bash:3.2 bash /src/tests/install-deps/test-macos-sim.sh

set -u

HERE=$(cd "$(dirname "$0")" && pwd)
SCRIPT=$HERE/../../scripts/install-deps.sh
TMP=$(mktemp -d "${TMPDIR:-/tmp}/macos-sim.XXXXXX")
trap 'rm -rf "$TMP"' EXIT
FAILS=0

# stub NAME OUTPUT: fake command that prints OUTPUT (and records that it ran).
stub() {
    # shellcheck disable=SC2016 # $0 and $* belong to the stub
    printf '#!/bin/sh\necho "$0 $*" >>"%s/ran"\necho "%s"\n' "$TMP" "$2" >"$BIN/$1"
    chmod +x "$BIN/$1"
}

base_stubs() {
    BIN=$1
    mkdir -p "$BIN"
    cat >"$BIN/uname" <<'STUB'
#!/bin/sh
case $1 in -s) echo Darwin ;; -m) echo arm64 ;; *) echo Darwin ;; esac
STUB
    chmod +x "$BIN/uname"
    stub sw_vers 15.1
    for t in cc clang git; do stub "$t" "$t (shim) 0.0"; done
    stub pkg-config 2.3.0
    stub cmake "cmake version 3.31.0"
    stub jq jq-1.7.1
    stub rg "ripgrep 14.1.1"
    stub fzf "0.56.3 (brew)"
}

# run_case NAME EXPECTED_CODE PATTERNS...: runs --check --no-rust.
run_case() {
    local name=$1 want=$2 out rc pat
    shift 2
    out=$(HOME=$TMP/home PATH="$BIN:/usr/bin:/bin" CREST_BREW_CANDIDATES="" "$BASH" "$SCRIPT" --check --no-rust 2>&1)
    rc=$?
    if [ "$rc" != "$want" ]; then
        echo "FAIL [$name]: exit code $rc, expected $want" >&2
        echo "$out" >&2
        FAILS=$((FAILS + 1))
    fi
    for pat in "$@"; do
        case $out in
            *"$pat"*) ;;
            *)
                echo "FAIL [$name]: '$pat' missing from the output" >&2
                echo "$out" >&2
                FAILS=$((FAILS + 1))
                ;;
        esac
    done
}

# 1. No Command Line Tools and no Homebrew: must not run cc/clang/git (they
#    would open a dialog) and must explain how to install CLT, Xcode and Homebrew.
base_stubs "$TMP/bin1"
printf '#!/bin/sh\nexit 2\n' >"$BIN/xcode-select"
chmod +x "$BIN/xcode-select"
: >"$TMP/ran"
run_case "no CLT" 1 "xcode-select --install" "App Store" "brew.sh" "clt metal brew"
if grep -q -e '/cc ' -e '/clang ' -e '/git ' "$TMP/ran"; then
    echo "FAIL [no CLT]: ran a CLT shim" >&2
    FAILS=$((FAILS + 1))
fi

# 2. With CLT, Xcode (metal) and Homebrew: everything present.
base_stubs "$TMP/bin2"
mkdir -p "$TMP/clt"
printf '#!/bin/sh\necho "%s"\n' "$TMP/clt" >"$BIN/xcode-select"
chmod +x "$BIN/xcode-select"
stub xcrun "Apple metal version 32023.404 (metalfe-32023.404)"
stub brew "Homebrew 4.4.0"
stub cc "Apple clang version 16.0.0 (clang-1600.0.26.4)"
stub clang "Apple clang version 16.0.0 (clang-1600.0.26.4)"
stub git "git version 2.39.5 (Apple Git-154)"
run_case "complete" 0 "Metal compiler" "All dependencies"

# 3. Only CLT (no Xcode): the Metal compiler is missing and that is explained.
printf '#!/bin/sh\necho "xcrun: error: unable to find utility metal" >&2\nexit 72\n' >"$BIN/xcrun"
run_case "no Xcode" 1 "missing 1 dependencies: metal" "xcodebuild -runFirstLaunch"

echo "simulated macOS: $FAILS failures"
[ "$FAILS" = 0 ]
