#!/usr/bin/env bash
set -euo pipefail

# Make repository scripts executable and optionally symlink them into ~/.local/bin
INSTALL_BIN_DIR="${1:-$HOME/.local/bin}"
DO_SYMLINK=${2:-1}

echo "Making scripts executable under scripts/"
find scripts -maxdepth 1 -type f -name "*.sh" -exec chmod +x {} +

if [[ "$DO_SYMLINK" -ne 0 ]]; then
  mkdir -p "$INSTALL_BIN_DIR"
  for f in scripts/*.sh; do
    name=$(basename "$f" .sh)
    ln -sf "$(pwd)/$f" "$INSTALL_BIN_DIR/$name"
    echo "Linked $f -> $INSTALL_BIN_DIR/$name"
  done
  echo "Ensure $INSTALL_BIN_DIR is in your PATH. Example:"
  echo "  export PATH=\"$INSTALL_BIN_DIR:\$PATH\""
fi
