#!/usr/bin/env bash
# Regenerate the screenshots used by README.md and docs/ (#265).
#
#   scripts/screenshots.sh               # full run: web build + captures
#   scripts/screenshots.sh --skip-build  # reuse apps/web/dist
#   scripts/screenshots.sh --only mobile # one set only
#
# The captures come from the demo build of the web UI (VITE_USE_MOCKS=true):
# no server, no archive, no token, no network. Output lands in
# docs/assets/screenshots/ with a manifest recording the version, commit,
# viewport, theme and locale. See docs/assets/screenshots/README.md.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

if ! command -v pnpm >/dev/null 2>&1; then
  echo "pnpm is required (Node 22, pnpm 9). See CONTRIBUTING.md." >&2
  exit 1
fi

if [[ ! -d node_modules || ! -d e2e/node_modules ]]; then
  echo "== installing workspace dependencies =="
  pnpm install
fi

if [[ ! -d "$HOME/.cache/ms-playwright" ]]; then
  echo "== downloading Chromium for Playwright =="
  pnpm e2e:install
fi

exec pnpm --filter @ludex/e2e screenshots "$@"
