#!/usr/bin/env bash
# #332: lint the committed portable-bundle scripts.
#
# Ludex generates run.sh and install.sh into every Linux install folder. The
# committed copies under apps/desktop/src-tauri/tests/fixtures/portable are the
# goldens the Rust tests compare against, so shellchecking them here checks
# exactly what a player will run. Regenerate the goldens with:
#
#   cd apps/desktop/src-tauri
#   cargo test --offline --lib portable::tests::write_goldens -- --ignored
#
# No network, no build: shellcheck only.
set -euo pipefail

root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
fixtures="$root/apps/desktop/src-tauri/tests/fixtures/portable"

if ! command -v shellcheck >/dev/null 2>&1; then
  printf '%s\n' "shellcheck is required: https://www.shellcheck.net" >&2
  exit 1
fi

if [ ! -d "$fixtures" ]; then
  printf 'Missing fixtures: %s\n' "$fixtures" >&2
  exit 1
fi

mapfile -t scripts < <(find "$fixtures" -type f -name '*.sh' | sort)
if [ "${#scripts[@]}" -eq 0 ]; then
  printf 'No portable scripts found under %s\n' "$fixtures" >&2
  exit 1
fi

printf 'shellcheck %d portable script(s)\n' "${#scripts[@]}"
shellcheck --shell=bash --severity=style "${scripts[@]}"
printf 'portable scripts: OK\n'
