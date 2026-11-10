#!/usr/bin/env bash
# Cut a patch (or explicit) release: bump, changelog, commit, tag, push.
#
# Usage:
#   scripts/release.sh                 # next patch (0.2.0 -> 0.2.1)
#   scripts/release.sh 0.2.6           # explicit version
#   scripts/release.sh patch --dry-run # show what would happen
#   scripts/release.sh patch --no-push # commit and tag locally only
#
# The tag push triggers .github/workflows/release.yml (E2E gate, server
# binaries, desktop bundles, GHCR image). Run it from a clean main that has
# already been pushed; the script refuses a dirty tree by default.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"

DRY_RUN=0
PUSH=1
TARGET=""
for arg in "$@"; do
  case "$arg" in
    --dry-run) DRY_RUN=1 ;;
    --no-push) PUSH=0 ;;
    -*) echo "error: unknown option $arg" >&2; exit 2 ;;
    *) TARGET="$arg" ;;
  esac
done

current="$(python3 - <<'PY'
import re
text = open("Cargo.toml", encoding="utf-8").read()
match = re.search(r'\[workspace\.package\][^\[]*?\nversion = "([^"]+)"', text, re.S)
if not match:
    raise SystemExit("error: [workspace.package].version not found")
print(match.group(1))
PY
)"

if [ -z "$TARGET" ] || [ "$TARGET" = "patch" ] || [ "$TARGET" = "minor" ] || [ "$TARGET" = "major" ]; then
  target="$(python3 - "$current" "${TARGET:-patch}" <<'PY'
import sys
current, kind = sys.argv[1], sys.argv[2]
base = current.split("-")[0].split("+")[0]
parts = [int(p) for p in base.split(".")]
if kind == "patch":
    parts[2] += 1
elif kind == "minor":
    parts[1] += 1
    parts[2] = 0
elif kind == "major":
    parts[0] += 1
    parts[1] = parts[2] = 0
print(".".join(str(p) for p in parts))
PY
)"
else
  target="$TARGET"
fi

if [ "$target" = "$current" ]; then
  echo "error: $target is already the current version" >&2
  exit 2
fi

if [ "$DRY_RUN" -eq 1 ]; then
  echo "would release v$target (current $current)"
  echo "  scripts/bump-version.sh $target"
  echo "  update CHANGELOG.md heading"
  echo "  refresh Cargo.lock + pnpm-lock.yaml"
  echo "  git commit -m \"chore(release): v$target\""
  echo "  git tag -a v$target -m \"Ludex v$target\""
  [ "$PUSH" -eq 1 ] && echo "  git push origin main refs/tags/v$target" || echo "  (not pushed)"
  exit 0
fi

if [ -n "$(git status --porcelain)" ]; then
  echo "error: the working tree is dirty; commit or stash first" >&2
  git status --short >&2
  exit 2
fi

branch="$(git rev-parse --abbrev-ref HEAD)"
if [ "$branch" != "main" ]; then
  echo "error: release from main, not $branch" >&2
  exit 2
fi

if git rev-parse -q --verify "refs/tags/v$target" >/dev/null; then
  echo "error: tag v$target already exists" >&2
  exit 2
fi

"$ROOT/scripts/bump-version.sh" "$target" >/dev/null
echo "version -> $target"

python3 - "$target" <<'PY'
import datetime
import sys

version = sys.argv[1]
today = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%d")
path = "CHANGELOG.md"
text = open(path, encoding="utf-8").read()
if f"## [{version}]" in text:
    raise SystemExit(f"error: CHANGELOG already has {version}")
marker = "## [Unreleased]\n"
if marker not in text:
    raise SystemExit("error: CHANGELOG has no [Unreleased] heading")
heading = f"## [{version}] - {today}\n"
section_start = text.index(marker)
# The release notes are the current Unreleased body up to the next heading.
rest = text[section_start + len(marker):]
next_heading = rest.find("\n## ")
body = rest if next_heading == -1 else rest[:next_heading]
tail = "" if next_heading == -1 else rest[next_heading:]
if not body.strip():
    raise SystemExit("error: CHANGELOG Unreleased section is empty")
# Move the notes out of Unreleased (never duplicate them): the next batch
# starts from an empty Unreleased section.
text = text[:section_start] + marker + "\n" + heading + body.rstrip() + "\n" + tail
open(path, "w", encoding="utf-8").write(text)
print(f"changelog -> {version}")
PY

# Refresh lockfiles so the workspace packages carry the new version.
cargo update --workspace --offline >/dev/null
(cd apps/desktop/src-tauri && cargo update --offline >/dev/null)
pnpm install --lockfile-only >/dev/null 2>&1 || true

git add Cargo.toml Cargo.lock apps/desktop/src-tauri/Cargo.toml apps/desktop/src-tauri/Cargo.lock \
  apps/desktop/src-tauri/tauri.conf.json package.json apps/web/package.json \
  apps/desktop/package.json e2e/package.json pnpm-lock.yaml CHANGELOG.md
git commit -q -m "chore(release): v$target"
git tag -a "v$target" -m "Ludex v$target"
echo "committed and tagged v$target"

if [ "$PUSH" -eq 1 ]; then
  git push -q origin main
  git push -q origin "refs/tags/v$target"
  echo "pushed main and v$target (release workflow started)"
fi
