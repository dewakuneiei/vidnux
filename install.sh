#!/usr/bin/env bash
# Build Vidnux and install it for the current user (or system-wide with --system).
set -euo pipefail

cd "$(dirname "$(readlink -f "$0")")"

SYSTEM=0
[[ "${1:-}" == "--system" ]] && SYSTEM=1

if [[ $SYSTEM -eq 1 ]]; then
  PREFIX=${PREFIX:-/usr/local}
  BIN="$PREFIX/bin"
  APPS="$PREFIX/share/applications"
  ICONS="$PREFIX/share/icons/hicolor/scalable/apps"
  if [[ $EUID -ne 0 ]]; then
    echo "--system needs root:  sudo ./install.sh --system" >&2
    exit 1
  fi
else
  BIN="$HOME/.local/bin"
  APPS="$HOME/.local/share/applications"
  ICONS="$HOME/.local/share/icons/hicolor/scalable/apps"
fi

# --- dependencies -----------------------------------------------------------
# `sudo` usually drops ~/.cargo/bin from PATH, so look for cargo where rustup
# actually puts it before giving up.
find_cargo() {
  local c
  c=$(command -v cargo 2>/dev/null) && { echo "$c"; return; }
  for c in "${HOME:-}/.cargo/bin/cargo" \
           "$(getent passwd "${SUDO_USER:-${USER:-root}}" | cut -d: -f6)/.cargo/bin/cargo" \
           /usr/local/cargo/bin/cargo /usr/bin/cargo; do
    [[ -x "$c" ]] && { echo "$c"; return; }
  done
  return 1
}

CARGO=$(find_cargo) || CARGO=""
MISSING=()
[[ -n "$CARGO" ]] || MISSING+=(cargo)
command -v ffmpeg  >/dev/null || MISSING+=(ffmpeg)
command -v ffprobe >/dev/null || MISSING+=(ffprobe)

if (( ${#MISSING[@]} )); then
  echo "Missing: ${MISSING[*]}" >&2
  cat >&2 <<'MSG'

Install what is missing first:
  Fedora / Nobara : sudo dnf install ffmpeg rust cargo
  Debian / Ubuntu : sudo apt install ffmpeg cargo
  Arch            : sudo pacman -S ffmpeg rust

Rust installed through rustup instead? Then cargo lives in ~/.cargo/bin and is
simply not on this shell's PATH:
  export PATH="$HOME/.cargo/bin:$PATH"
MSG
  exit 1
fi

echo "==> Building (this takes a few minutes the first time)"
if [[ $SYSTEM -eq 1 && -n "${SUDO_USER:-}" ]]; then
  # Build as the real user so target/ and ~/.cargo stay owned by them.
  sudo -u "$SUDO_USER" -H "$CARGO" build --release
else
  "$CARGO" build --release
fi

echo "==> Installing to $BIN"
install -Dm755 target/release/vidnux "$BIN/vidnux"
install -Dm644 vidnux.desktop "$APPS/vidnux.desktop"
install -Dm644 vidnux.svg "$ICONS/vidnux.svg"

command -v update-desktop-database >/dev/null && update-desktop-database "$APPS" 2>/dev/null || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -qtf "$(dirname "$(dirname "$(dirname "$ICONS")")")" 2>/dev/null || true

echo "==> Done. Launch it from your app menu, or run: vidnux"
case ":$PATH:" in
  *":$BIN:"*) ;;
  *) echo "Note: $BIN is not on your PATH. Add this to ~/.bashrc:"
     echo "      export PATH=\"\$HOME/.local/bin:\$PATH\"" ;;
esac
