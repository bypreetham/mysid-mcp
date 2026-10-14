#!/bin/sh
# mysid installer for macOS and Linux (Windows: install.ps1).
#
#   curl -fsSL https://raw.githubusercontent.com/bypreetham/mysid-mcp/main/install.sh | sh
#
# Downloads the prebuilt binary for this system from the GitHub release, checks its checksum and that it runs, installs it to
# ~/.local/bin, puts that folder on PATH (shell profile) when it is not there, and registers the MCP server for Claude Code,
# Claude Desktop (macOS) and Antigravity/Gemini when they are installed.
#
# Environment: MYSID_INSTALL_DIR (default ~/.local/bin), MYSID_NO_PATH=1 (do not edit shell profiles),
#              MYSID_NO_MCP=1 (do not register MCP clients), MYSID_BASE_URL (another download location).
set -eu

REPO="bypreetham/mysid-mcp"
BASE="${MYSID_BASE_URL:-https://github.com/$REPO/releases/download/latest}"
BIN_DIR="${MYSID_INSTALL_DIR:-$HOME/.local/bin}"

say() { printf '%s\n' "$*"; }
fail() { printf 'ERROR: %s\n' "$*" >&2; exit 1; }

# 1. Which build
os=$(uname -s)
arch=$(uname -m)
case "$os" in
  Linux) os=linux ;;
  Darwin) os=macos ;;
  *) fail "unsupported system '$os'. On Windows use install.ps1 (PowerShell)." ;;
esac
case "$arch" in
  x86_64 | amd64) arch=x64 ;;
  aarch64 | arm64) arch=arm64 ;;
  *) fail "unsupported processor '$arch' (builds exist for x86_64 and arm64)." ;;
esac
# A shell running under Rosetta reports x86_64 on an Apple Silicon Mac: use the native build.
if [ "$os" = macos ] && [ "$arch" = x64 ] && [ "$(sysctl -n hw.optional.arm64 2>/dev/null || echo 0)" = 1 ]; then
  arch=arm64
fi
asset="mysid-$os-$arch"

# 2. Download
if command -v curl >/dev/null 2>&1; then
  download() { curl -fsSL "$1" -o "$2"; }
elif command -v wget >/dev/null 2>&1; then
  download() { wget -q "$1" -O "$2"; }
else
  fail "curl or wget is needed to download mysid."
fi
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

say "Downloading $asset ..."
download "$BASE/$asset" "$tmp/mysid" ||
  fail "could not download $BASE/$asset. There may be no build for $os/$arch yet: see https://github.com/$REPO/releases/tag/latest (or build from source: cd rust-mcp && cargo build --release)."

# 3. Checksum (when the release lists one) and a run test, before anything is replaced
if download "$BASE/SHA256SUMS" "$tmp/SHA256SUMS" 2>/dev/null; then
  want=$(awk -v f="$asset" '$2 == f || $2 == "*" f { print $1 }' "$tmp/SHA256SUMS" | head -n 1)
  if [ -n "$want" ]; then
    if command -v sha256sum >/dev/null 2>&1; then
      got=$(sha256sum "$tmp/mysid" | awk '{ print $1 }')
    elif command -v shasum >/dev/null 2>&1; then
      got=$(shasum -a 256 "$tmp/mysid" | awk '{ print $1 }')
    else
      got=""
    fi
    if [ -n "$got" ] && [ "$got" != "$want" ]; then
      fail "checksum mismatch for $asset (expected $want, got $got). Nothing was installed."
    fi
    [ -n "$got" ] && say "Checksum OK."
  fi
fi
chmod +x "$tmp/mysid"
probe=$("$tmp/mysid" help 2>&1 | head -n 1 || true)
case "$probe" in
  *"code intelligence"*) ;;
  *) fail "the downloaded file did not behave like mysid (mysid help printed: $probe). Nothing was installed." ;;
esac

# 4. Install (a running copy keeps working: the new file replaces the old one by rename)
mkdir -p "$BIN_DIR"
cp "$tmp/mysid" "$BIN_DIR/mysid.new"
mv -f "$BIN_DIR/mysid.new" "$BIN_DIR/mysid"
say "Installed: $BIN_DIR/mysid"

# 5. PATH
case ":$PATH:" in
  *":$BIN_DIR:"*) say "$BIN_DIR is already on PATH." ;;
  *)
    line="export PATH=\"$BIN_DIR:\$PATH\""
    if [ "${MYSID_NO_PATH:-}" = 1 ]; then
      say "Add $BIN_DIR to PATH:  $line"
    else
      case "${SHELL:-}" in
        */zsh) profile="$HOME/.zshrc" ;;
        */bash) if [ "$os" = macos ]; then profile="$HOME/.bash_profile"; else profile="$HOME/.bashrc"; fi ;;
        *) profile="$HOME/.profile" ;;
      esac
      if [ -f "$profile" ] && grep -F "$BIN_DIR" "$profile" >/dev/null 2>&1; then
        say "$profile already mentions $BIN_DIR. Open a new terminal."
      else
        printf '\n# mysid\n%s\n' "$line" >> "$profile"
        say "Added $BIN_DIR to PATH in $profile. Open a new terminal (or run: $line)."
      fi
    fi
    ;;
esac

# 6. MCP registration: absolute path, so it works whatever the client's PATH is
register_json() {
  file=$1
  label=$2
  if ! command -v python3 >/dev/null 2>&1; then
    say "  $label: add to $file  { \"mcpServers\": { \"mysid\": { \"command\": \"$BIN_DIR/mysid\", \"args\": [] } } }"
    return 0
  fi
  python3 - "$file" "$BIN_DIR/mysid" <<'PY' && say "  Registered in $label: $file"
import json, os, sys
path, cmd = sys.argv[1], sys.argv[2]
data = {}
if os.path.exists(path) and os.path.getsize(path) > 0:
    with open(path) as f:
        data = json.load(f)
data.setdefault("mcpServers", {})["mysid"] = {"command": cmd, "args": []}
with open(path, "w") as f:
    json.dump(data, f, indent=2)
PY
}

if [ "${MYSID_NO_MCP:-}" = 1 ]; then
  say "Skipping MCP registration (MYSID_NO_MCP=1)."
else
  if command -v claude >/dev/null 2>&1; then
    if claude mcp get mysid >/dev/null 2>&1; then
      say "  Claude Code: mysid is already registered."
    elif claude mcp add --scope user mysid -- "$BIN_DIR/mysid" >/dev/null 2>&1; then
      say "  Registered in Claude Code (user scope). Start a new Claude Code session to load it."
    else
      say "  Could not register in Claude Code. Run:  claude mcp add --scope user mysid -- $BIN_DIR/mysid"
    fi
  else
    say "  Claude Code CLI not found; if you use it, run:  claude mcp add --scope user mysid -- $BIN_DIR/mysid"
  fi
  desktop="$HOME/Library/Application Support/Claude"
  if [ "$os" = macos ] && [ -d "$desktop" ]; then
    register_json "$desktop/claude_desktop_config.json" "Claude Desktop (restart it to load mysid)" || true
  fi
  if [ -d "$HOME/.gemini/config" ]; then
    register_json "$HOME/.gemini/config/mcp_config.json" "Antigravity/Gemini" || true
  fi
fi

say ""
say "mysid is installed. In a project folder: mysid set .   then   mysid help"
"$BIN_DIR/mysid" help 2>&1 | head -n 3
