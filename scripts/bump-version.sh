#!/usr/bin/env bash
# Bump every version location listed in docs/RELEASING.md.
#
# Usage: scripts/bump-version.sh 0.3.0
#
# The script only edits files; it does not commit, tag or push. Review the diff,
# update CHANGELOG.md, then refresh the lockfiles:
#   cargo update --workspace --offline   (root workspace)
#   (cd apps/desktop/src-tauri && cargo update --offline)
#   pnpm install --lockfile-only
set -euo pipefail

if [ "$#" -ne 1 ]; then
  echo "usage: $0 <version>  (for example 0.3.0)" >&2
  exit 2
fi

VERSION="$1"
if ! [[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?$ ]]; then
  echo "error: '$VERSION' is not a semver version" >&2
  exit 2
fi

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

# Replace the *package* version only: never touch dependency versions.
python3 - "$VERSION" <<'PY'
import json
import re
import sys

version = sys.argv[1]

def edit(path, pattern, replacement, count=1):
    with open(path, encoding="utf-8") as handle:
        text = handle.read()
    updated, changes = re.subn(pattern, replacement, text, count=count, flags=re.MULTILINE)
    if changes == 0:
        raise SystemExit(f"error: expected version not found in {path}")
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(updated)
    print(f"  {path}")

# Root workspace package version (the [workspace.package] block only).
edit("Cargo.toml", r'(\[workspace\.package\][^\[]*?\nversion = ")[^"]+(")', r"\g<1>%s\g<2>" % version)
# Desktop crate (outside the workspace).
edit("apps/desktop/src-tauri/Cargo.toml", r'(^version = ")[^"]+(")', r"\g<1>%s\g<2>" % version)

# JSON manifests: replace the first "version" string only. Parsing and
# re-dumping would reformat unrelated arrays and keys (noisy diffs).
for path in (
    "apps/desktop/src-tauri/tauri.conf.json",
    "package.json",
    "apps/web/package.json",
    "apps/desktop/package.json",
    "e2e/package.json",
):
    edit(path, r'(^  "version": ")[^"]+(")', r"\g<1>%s\g<2>" % version)
    # Validate the result is still valid JSON.
    with open(path, encoding="utf-8") as handle:
        json.load(handle)
    print(f"  {path}")
PY

echo
echo "Version set to $VERSION. Next:"
echo "  1. update CHANGELOG.md (move [Unreleased] under [$VERSION] - $(date -u +%Y-%m-%d))"
echo "  2. PATH=/root/.cargo/bin:\$PATH cargo update --workspace --offline"
echo "  3. (cd apps/desktop/src-tauri && PATH=/root/.cargo/bin:\$PATH cargo update --offline)"
echo "  4. pnpm install --lockfile-only"
echo "  5. run the release checks from docs/RELEASING.md, then tag: git tag -a v$VERSION -m \"Ludex v$VERSION\""
