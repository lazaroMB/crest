#!/bin/sh
# Starts crest from a desktop launcher (the Linux app menu, the macOS Dock,
# Launchpad or Spotlight) with the environment of the user's login shell.
#
# Apps started that way do not inherit the terminal's PATH: on macOS it is
# /usr/bin:/bin:/usr/sbin:/sbin, without Homebrew, so crest would not find
# rg, fzf or git. The login shell reads ~/.zprofile, ~/.profile or
# ~/.bash_profile (where `brew shellenv` and ~/.cargo/bin usually live).
# It is not interactive (-i): an ~/.zshrc that prompts or execs tmux would
# keep the app from starting.
#
# The binary is crest-bin next to this script (inside crest.app), or
# ../../bin/crest (PREFIX/lib/crest/crest-launcher -> PREFIX/bin/crest).

here=$(cd "$(dirname "$0")" && pwd)
if [ -x "$here/crest-bin" ]; then
    bin="$here/crest-bin"
elif [ -x "$here/../../bin/crest" ]; then
    bin="$here/../../bin/crest"
else
    echo "crest-launcher: crest is not installed next to $here" >&2
    exit 127
fi

shell=${SHELL:-}
if [ -z "$shell" ]; then
    case "$(uname -s)" in
        Darwin) shell=/bin/zsh ;;
        *) shell=/bin/sh ;;
    esac
fi
case "$(basename "$shell")" in
    sh | bash | zsh | dash | ksh | mksh | yash) ;;
    # fish, nu, xonsh...: other syntax; a POSIX login shell still reads
    # ~/.profile.
    *) shell=/bin/sh ;;
esac

exec "$shell" -l -c 'exec "$0" "$@"' "$bin" "$@"
