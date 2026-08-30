#!/usr/bin/env bash
# Remove Vidnux. Use --system if it was installed with ./install.sh --system.
set -euo pipefail

cd "$(dirname "$(readlink -f "$0")")"

if [[ "${1:-}" == "--system" ]]; then
  PREFIX=${PREFIX:-/usr/local}
  FILES=("$PREFIX/bin/vidnux" "$PREFIX/share/applications/vidnux.desktop" \
         "$PREFIX/share/icons/hicolor/scalable/apps/vidnux.svg")
  APPS="$PREFIX/share/applications"
  if [[ $EUID -ne 0 ]]; then
    echo "--system needs root:  sudo ./uninstall.sh --system" >&2
    exit 1
  fi
else
  FILES=("$HOME/.local/bin/vidnux" "$HOME/.local/share/applications/vidnux.desktop" \
         "$HOME/.local/share/icons/hicolor/scalable/apps/vidnux.svg")
  APPS="$HOME/.local/share/applications"
fi

for f in "${FILES[@]}"; do
  [[ -e "$f" ]] && rm -f "$f" && echo "removed $f"
done

command -v update-desktop-database >/dev/null && update-desktop-database "$APPS" 2>/dev/null || true

if [[ "${2:-${1:-}}" == "--purge" || "${1:-}" == "--purge" ]]; then
  rm -rf target && echo "removed build artifacts (target/)"
fi

echo "Vidnux uninstalled. Your converted videos were not touched."
