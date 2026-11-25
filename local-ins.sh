#!/bin/sh
# Local source installer for mysid on Ubuntu/Linux.
# Installs Rust/Cargo via rustup if missing, builds mysid from source,
# installs it to ~/.local/bin/mysid, and adds ~/.local/bin to PATH if needed.
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
RUST_DIR="$SCRIPT_DIR/rust-mcp"
BIN_DIR="${MYSID_INSTALL_DIR:-$HOME/.local/bin}"

say() { printf '%s\n' "$*"; }
fail() { printf 'ERROR: %s\n' "$*" >&2; exit 1; }

[ -d "$RUST_DIR" ] || fail "rust-mcp directory not found at $RUST_DIR"

# 1. Install Rust / Cargo if not present
if ! command -v cargo >/dev/null 2>&1; then
  if [ -f "$HOME/.cargo/env" ]; then
    # shellcheck disable=SC1091
    . "$HOME/.cargo/env"
  fi
fi

if ! command -v cargo >/dev/null 2>&1; then
  say "Installing Rust and Cargo via rustup ..."
  if command -v curl >/dev/null 2>&1; then
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
  elif command -v wget >/dev/null 2>&1; then
    wget -qO- https://sh.rustup.rs | sh -s -- -y
  else
    fail "curl or wget is required to install rustup."
  fi
  # shellcheck disable=SC1091
  . "$HOME/.cargo/env"
fi

# 2. Build from source
say "Building mysid from source ..."
(cd "$RUST_DIR" && cargo build --release)

BUILT_BIN="$RUST_DIR/target/release/mysid"
[ -x "$BUILT_BIN" ] || fail "expected binary not found at $BUILT_BIN"

# 3. Verify and install
probe=$("$BUILT_BIN" help 2>&1 | head -n 1 || true)
case "$probe" in
  *"code intelligence"*) ;;
  *) fail "built binary did not behave like mysid (mysid help printed: $probe)" ;;
esac

mkdir -p "$BIN_DIR"
cp "$BUILT_BIN" "$BIN_DIR/mysid.new"
mv -f "$BIN_DIR/mysid.new" "$BIN_DIR/mysid"
say "Installed: $BIN_DIR/mysid"

# 4. Ensure BIN_DIR is on PATH
case ":$PATH:" in
  *":$BIN_DIR:"*) say "$BIN_DIR is already on PATH." ;;
  *)
    line="export PATH=\"$BIN_DIR:\$PATH\""
    case "${SHELL:-}" in
      */zsh) profile="$HOME/.zshrc" ;;
      */bash) profile="$HOME/.bashrc" ;;
      *) profile="$HOME/.profile" ;;
    esac
    if [ -f "$profile" ] && grep -F "$BIN_DIR" "$profile" >/dev/null 2>&1; then
      say "$profile already mentions $BIN_DIR. Open a new terminal or run: $line"
    else
      printf '\n%s\n' "$line" >> "$profile"
      say "Added $BIN_DIR to $profile. Run 'source $profile' or open a new terminal."
    fi
    ;;
esac

say "Done! Run 'mysid help' to get started."
