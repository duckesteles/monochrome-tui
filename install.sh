#!/bin/sh
# Installs monochrome, a terminal client for monochrome.tf.
#
#   curl -fsSL https://raw.githubusercontent.com/duckesteles/monochrome-tui/main/install.sh | sh
#
# Nothing is left behind but the program itself. To remove it later:
#
#   monochrome --uninstall

set -eu

REPO=https://github.com/duckesteles/monochrome-tui.git
PREFIX="${MONOCHROME_PREFIX:-$HOME/.local}"
BIN="$PREFIX/bin/monochrome"
WORK=""
BORROWED_RUST=""

say()  { printf '%s\n' "$*"; }
step() { printf '\n\033[1m%s\033[0m\n' "$*"; }
die()  { printf '\nerror: %s\n' "$*" >&2; exit 1; }

cleanup() {
    [ -n "$WORK" ] && [ -d "$WORK" ] && rm -rf "$WORK"
    return 0
}
trap cleanup EXIT INT TERM

[ "$(uname -s)" = "Linux" ] || die "monochrome is Linux only for now, this is $(uname -s)"

for tool in git curl; do
    command -v "$tool" >/dev/null 2>&1 || die "$tool is needed and was not found"
done

if ! pkg-config --exists alsa 2>/dev/null && [ ! -f /usr/include/alsa/asoundlib.h ]; then
    say "The ALSA development headers are needed to build the audio output."
    if   command -v pacman  >/dev/null 2>&1; then die "install them with: sudo pacman -S alsa-lib"
    elif command -v apt-get >/dev/null 2>&1; then die "install them with: sudo apt-get install libasound2-dev"
    elif command -v dnf     >/dev/null 2>&1; then die "install them with: sudo dnf install alsa-lib-devel"
    elif command -v zypper  >/dev/null 2>&1; then die "install them with: sudo zypper install alsa-lib-devel"
    else die "install your distribution's alsa-lib development package"
    fi
fi

WORK=$(mktemp -d)

if command -v cargo >/dev/null 2>&1; then
    step "Using the Rust toolchain you already have"
else
    step "Borrowing a Rust toolchain (removed again when this finishes)"
    export CARGO_HOME="$WORK/cargo" RUSTUP_HOME="$WORK/rustup"
    if ! curl -fsSL https://sh.rustup.rs \
        | sh -s -- -y --no-modify-path --profile minimal --default-toolchain stable \
          >"$WORK/rustup.log" 2>&1
    then
        say ""
        tail -20 "$WORK/rustup.log" >&2
        die "could not fetch a Rust toolchain"
    fi
    PATH="$CARGO_HOME/bin:$PATH"
    export PATH
    BORROWED_RUST=yes
fi

step "Fetching the source"
git clone --depth 1 --quiet "$REPO" "$WORK/src"

step "Building, this takes a couple of minutes"
cd "$WORK/src"
if ! cargo build --release --locked --quiet --package monochrome-tui 2>"$WORK/build.log"; then
    say ""
    tail -30 "$WORK/build.log" >&2
    die "the build failed"
fi

step "Installing"
mkdir -p "$PREFIX/bin"
install -m 755 "$WORK/src/target/release/monochrome" "$BIN"
say "  $BIN"

cd /
cleanup
WORK=""

[ -n "$BORROWED_RUST" ] && say "  the borrowed toolchain has been removed"

step "Done"
if command -v monochrome >/dev/null 2>&1; then
    say "Run: monochrome"
else
    say "$PREFIX/bin is not on your PATH yet. Add it:"
    say ""
    case "${SHELL##*/}" in
        fish) say "  fish_add_path $PREFIX/bin" ;;
        zsh)  say "  echo 'export PATH=\"$PREFIX/bin:\$PATH\"' >> ~/.zshrc && exec zsh" ;;
        *)    say "  echo 'export PATH=\"$PREFIX/bin:\$PATH\"' >> ~/.bashrc && exec bash" ;;
    esac
    say ""
    say "Then run: monochrome"
fi
say "Remove it any time with: monochrome --uninstall"
