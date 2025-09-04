#!/usr/bin/env bash
set -euo pipefail

# Install repository hooks for this clone by configuring
# git to use the bundled .githooks directory.
# This is a safe one-time action per clone and is intentionally
# explicit so users opt into local hook execution.

REPO_ROOT=$(git rev-parse --show-toplevel 2>/dev/null || echo "")
if [ -z "$REPO_ROOT" ]; then
  echo "Not inside a git repository. Run this from within the repo root." >&2
  exit 1
fi

HOOKS_DIR="$REPO_ROOT/.githooks"
if [ ! -d "$HOOKS_DIR" ]; then
  echo "Hooks directory not found: $HOOKS_DIR" >&2
  exit 1
fi

echo "Setting core.hooksPath to $HOOKS_DIR"
git config core.hooksPath "$HOOKS_DIR"

echo "Making hooks executable"
chmod +x "$HOOKS_DIR"/* || true

echo "Hooks installed. You can undo with: git config --unset core.hooksPath" 

exit 0
