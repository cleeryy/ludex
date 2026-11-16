#!/usr/bin/env bash
# Generate the body of a GitHub release: curated CHANGELOG section, contributor
# thanks, per-asset SHA-256 digests and a "how to verify" block.
#
# The body is generated, never hand-edited: edit CHANGELOG.md and re-run. The
# release workflow calls this script after every artifact is uploaded, so the
# checksums in the body are the digests of the files on the release page.
#
# Usage:
#   scripts/release-notes.sh --tag v0.4.14                    # -> stdout
#   scripts/release-notes.sh --tag v0.4.14 --assets dist \
#     --sums-out dist/SHA256SUMS --sums-signature SHA256SUMS.sig \
#     --out release-body.md
#   scripts/release-notes.sh --tag edge --channel edge --version 0.4.15-edge.247
#
# Options:
#   --tag TAG              release tag (default: $TAG, $GITHUB_REF_NAME, git describe)
#   --version VERSION      version string (default: TAG without the leading "v")
#   --previous TAG         previous release tag (default: nearest v* tag before TAG)
#   --channel CHANNEL      stable | edge (default: derived from the tag)
#   --commit SHA           build commit (default: $GITHUB_SHA, then the tag's commit)
#   --repo OWNER/NAME      repository (default: $GITHUB_REPOSITORY, then origin URL)
#   --assets DIR           checksum every artifact in DIR (recursive)
#   --checksums FILE       reuse an existing `sha256sum` output instead of --assets
#   --sums-out FILE        also write the SHA256SUMS file
#   --sums-signature NAME  signature asset covering SHA256SUMS (e.g. SHA256SUMS.sig)
#   --changelog FILE       CHANGELOG to read (default: CHANGELOG.md)
#   --out FILE             write the body to FILE (default: stdout)
#   --no-highlights        skip the derived Highlights section
#   -h, --help             this help
#
# Exit codes: 0 body generated, 1 generation failed, 2 bad usage.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

TAG="${TAG:-}"
VERSION="${VERSION:-}"
PREVIOUS="${PREVIOUS_TAG:-}"
CHANNEL="${RELEASE_CHANNEL:-}"
COMMIT="${GITHUB_SHA:-}"
REPO="${GITHUB_REPOSITORY:-}"
ASSETS=""
CHECKSUMS=""
SUMS_OUT=""
SUMS_SIGNATURE=""
CHANGELOG="CHANGELOG.md"
OUT=""
HIGHLIGHTS=1

usage() {
  awk 'NR > 1 { if ($0 !~ /^#/) exit; sub(/^# ?/, ""); print }' "${BASH_SOURCE[0]}"
}
die() { echo "error: $*" >&2; exit 1; }

while [ $# -gt 0 ]; do
  case "$1" in
    --tag) TAG="${2:?--tag needs a value}"; shift 2 ;;
    --tag=*) TAG="${1#*=}"; shift ;;
    --version) VERSION="${2:?--version needs a value}"; shift 2 ;;
    --version=*) VERSION="${1#*=}"; shift ;;
    --previous) PREVIOUS="${2:?--previous needs a value}"; shift 2 ;;
    --previous=*) PREVIOUS="${1#*=}"; shift ;;
    --channel) CHANNEL="${2:?--channel needs a value}"; shift 2 ;;
    --channel=*) CHANNEL="${1#*=}"; shift ;;
    --commit) COMMIT="${2:?--commit needs a value}"; shift 2 ;;
    --commit=*) COMMIT="${1#*=}"; shift ;;
    --repo) REPO="${2:?--repo needs a value}"; shift 2 ;;
    --repo=*) REPO="${1#*=}"; shift ;;
    --assets) ASSETS="${2:?--assets needs a value}"; shift 2 ;;
    --assets=*) ASSETS="${1#*=}"; shift ;;
    --checksums) CHECKSUMS="${2:?--checksums needs a value}"; shift 2 ;;
    --checksums=*) CHECKSUMS="${1#*=}"; shift ;;
    --sums-out) SUMS_OUT="${2:?--sums-out needs a value}"; shift 2 ;;
    --sums-out=*) SUMS_OUT="${1#*=}"; shift ;;
    --sums-signature) SUMS_SIGNATURE="${2:?--sums-signature needs a value}"; shift 2 ;;
    --sums-signature=*) SUMS_SIGNATURE="${1#*=}"; shift ;;
    --changelog) CHANGELOG="${2:?--changelog needs a value}"; shift 2 ;;
    --changelog=*) CHANGELOG="${1#*=}"; shift ;;
    --out) OUT="${2:?--out needs a value}"; shift 2 ;;
    --out=*) OUT="${1#*=}"; shift ;;
    --no-highlights) HIGHLIGHTS=0; shift ;;
    -h|--help) usage; exit 0 ;;
    -*) die "unknown option $1 (see --help)" ;;
    *) die "unexpected argument $1 (see --help)" ;;
  esac
done

command -v python3 >/dev/null 2>&1 || die "python3 is required"
[ -f "$CHANGELOG" ] || die "changelog not found: $CHANGELOG"
if [ -n "$ASSETS" ] && [ ! -d "$ASSETS" ]; then die "assets directory not found: $ASSETS"; fi
if [ -n "$CHECKSUMS" ] && [ ! -f "$CHECKSUMS" ]; then die "checksums file not found: $CHECKSUMS"; fi
if [ -n "$CHECKSUMS" ] && [ -n "$ASSETS" ]; then die "--checksums and --assets are mutually exclusive"; fi

if [ -z "$TAG" ]; then
  TAG="$(git describe --tags --abbrev=0 2>/dev/null || true)"
fi
[ -n "$TAG" ] || die "no tag given and no tag reachable (pass --tag)"
if ! git rev-parse --verify --quiet "${TAG}^{commit}" >/dev/null; then
  # A not-yet-created tag (`edge` on the very first run) is fine when the build
  # commit is explicit: the body only needs a revision to describe.
  if [ -z "$COMMIT" ] || ! git rev-parse --verify --quiet "${COMMIT}^{commit}" >/dev/null; then
    die "unknown tag or commit: $TAG"
  fi
  echo "warning: tag $TAG does not exist yet; using commit $COMMIT" >&2
fi

if [ -z "$COMMIT" ]; then
  COMMIT="$(git rev-list -n 1 "$TAG")"
fi
COMMIT="$(git rev-parse "$COMMIT")" || die "cannot resolve commit: $COMMIT"

if [ -z "$CHANNEL" ]; then
  case "$TAG" in
    edge|edge-*|*"-edge."*) CHANNEL="edge" ;;
    *) CHANNEL="stable" ;;
  esac
fi
case "$CHANNEL" in
  stable|edge) ;;
  *) die "unknown channel: $CHANNEL (stable|edge)" ;;
esac

if [ -z "$VERSION" ]; then
  if [ "$CHANNEL" = "edge" ]; then
    VERSION="edge-$(git rev-parse --short "$COMMIT")"
  else
    VERSION="${TAG#v}"
  fi
fi

if [ -z "$PREVIOUS" ]; then
  if [ "$CHANNEL" = "edge" ]; then
    # The edge tag is force-moved on every main push, so the nearest tag is
    # meaningless: compare against the newest stable release instead.
    PREVIOUS="$(git tag --sort=-v:refname --list 'v*' | grep -v -- '-' | head -n 1 || true)"
  else
    PREVIOUS="$(git describe --tags --abbrev=0 --exclude='edge*' "${TAG}^{}^" 2>/dev/null || true)"
  fi
fi
# Never compare a tag with itself.
if [ "$PREVIOUS" = "$TAG" ]; then PREVIOUS=""; fi

if [ -z "$REPO" ]; then
  origin="$(git config --get remote.origin.url 2>/dev/null || true)"
  case "$origin" in
    *github.com[:/]*)
      REPO="${origin##*github.com}"
      REPO="${REPO#[:/]}"
      REPO="${REPO%.git}"
      ;;
  esac
fi

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

# ---------------------------------------------------------------------------
# Contributors since the previous tag. GitHub logins come from the compare API
# (author + Co-authored-by trailers), with git log as the offline fallback.
# Bots are filtered in the Python formatter.
# ---------------------------------------------------------------------------
contributors_file="$tmpdir/contributors.txt"
contrib_mode="git"
: > "$contributors_file"

if [ -n "$REPO" ] && command -v gh >/dev/null 2>&1; then
  range="$COMMIT"
  [ -n "$PREVIOUS" ] && range="${PREVIOUS}...${COMMIT}"
  if gh api "repos/${REPO}/compare/${range}" --jq '.commits[] | @json' \
      > "$contributors_file.gh" 2>/dev/null && [ -s "$contributors_file.gh" ]; then
    mv "$contributors_file.gh" "$contributors_file"
    contrib_mode="gh"
  else
    rm -f "$contributors_file.gh"
    echo "warning: compare API unavailable for ${range}; falling back to git log" >&2
  fi
fi

if [ "$contrib_mode" = "git" ]; then
  if [ -n "$PREVIOUS" ]; then
    git log --no-merges --format='%aN%x1f%aE%x1f%B%x1e' "${PREVIOUS}..${COMMIT}" \
      > "$contributors_file"
  else
    git log --no-merges --format='%aN%x1f%aE%x1f%B%x1e' "$COMMIT" \
      > "$contributors_file"
  fi
fi

echo "release notes: tag=$TAG version=$VERSION channel=$CHANNEL previous=${PREVIOUS:-none} commit=$COMMIT" >&2

CONTRIB_MODE="$contrib_mode" \
CONTRIBUTORS_FILE="$contributors_file" \
TAG="$TAG" \
VERSION="$VERSION" \
PREVIOUS="$PREVIOUS" \
CHANNEL="$CHANNEL" \
COMMIT="$COMMIT" \
REPO="$REPO" \
ASSETS="$ASSETS" \
CHECKSUMS="$CHECKSUMS" \
SUMS_OUT="$SUMS_OUT" \
SUMS_SIGNATURE="$SUMS_SIGNATURE" \
CHANGELOG="$CHANGELOG" \
HIGHLIGHTS="$HIGHLIGHTS" \
OUT="$OUT" \
python3 - <<'PY'
import hashlib
import json
import os
import re
import sys

env = os.environ
tag = env["TAG"]
version = env["VERSION"]
previous = env["PREVIOUS"]
channel = env["CHANNEL"]
commit = env["COMMIT"]
repo = env["REPO"]
assets_dir = env["ASSETS"]
checksums_path = env["CHECKSUMS"]
sums_out = env["SUMS_OUT"]
sums_signature = env["SUMS_SIGNATURE"]
changelog_path = env["CHANGELOG"]
highlights_enabled = env["HIGHLIGHTS"] == "1"
out_path = env["OUT"]
contrib_mode = env["CONTRIB_MODE"]
contributors_path = env["CONTRIBUTORS_FILE"]

MAIN_HEADING = re.compile(r"^## \[([^\]]+)\]")
SUB_HEADING = re.compile(r"^###\s+(.+?)\s*$")
UPGRADE = re.compile(r"(upgrade|migration|breaking)", re.I)

BOT_LOGINS = {
    "dependabot",
    "renovate",
    "github-actions",
    "pre-commit-ci",
    "allcontributors",
    "ludex-release",
    "web-flow",
}
NOREPLY = re.compile(r"^(?:\d+\+)?([^@+]+)@users\.noreply\.github\.com$", re.I)


def die(message):
    sys.stderr.write(f"error: {message}\n")
    raise SystemExit(1)


def read(path):
    with open(path, encoding="utf-8") as handle:
        return handle.read()


def extract_section(text, wanted):
    lines = text.splitlines()
    start = None
    for index, line in enumerate(lines):
        match = MAIN_HEADING.match(line)
        if match and match.group(1).strip() == wanted:
            start = index + 1
            break
    if start is None:
        return None
    body = []
    for line in lines[start:]:
        if MAIN_HEADING.match(line):
            break
        body.append(line)
    return "\n".join(body)


def parse_sections(body):
    """Merge repeated ### headings (the changelog can repeat Added/Changed)."""
    sections = []
    by_name = {}
    current = None
    entry = None
    for raw in body.splitlines():
        line = raw.rstrip()
        sub = SUB_HEADING.match(line)
        if sub:
            name = sub.group(1).strip()
            current = by_name.get(name.lower())
            if current is None:
                current = {"name": name, "entries": []}
                by_name[name.lower()] = current
                sections.append(current)
            entry = None
            continue
        if not line.strip():
            entry = None
            continue
        if current is None:
            # Intro prose before the first ### heading: keep it as its own block.
            current = {"name": "", "entries": []}
            sections.append(current)
            by_name[""] = current
        if line.startswith("- "):
            entry = [line[2:].strip()]
            current["entries"].append(entry)
        elif entry is not None:
            entry.append(line.strip())
        else:
            entry = [line.strip()]
            current["entries"].append(entry)
    for section in sections:
        section["entries"] = [" ".join(entry) for entry in section["entries"] if entry]
    return [section for section in sections if section["name"] or section["entries"]]


def summarize(text):
    """First clause or first sentence, short enough for a highlights bullet."""
    text = " ".join(text.split())
    for separator in (": ", ". ", "; "):
        head, found, _ = text.partition(separator)
        if found and 20 <= len(head) <= 160:
            return head.rstrip(".,;:")
    if len(text) <= 160:
        return text.rstrip(".,;:")
    cut = text[:157].rsplit(" ", 1)[0]
    return cut.rstrip(".,;:") + "…"


def normalize_space(text):
    return " ".join(text.split())


def collect_candidates():
    logins, names = set(), set()
    raw = read(contributors_path)

    def bot_login(login):
        lower = login.lower()
        return (
            lower.endswith("[bot]")
            or lower.endswith("-bot")
            or lower in BOT_LOGINS
            or lower.startswith("dependabot")
            or lower.startswith("renovate")
        )

    def bot_name(name):
        lower = name.lower()
        return "[bot]" in lower or lower in BOT_LOGINS or "github actions" in lower

    def add(login, name, email, message=""):
        login = (login or "").strip().lstrip("@")
        name = normalize_space(name or "")
        email = (email or "").strip()
        if not login and email:
            match = NOREPLY.match(email)
            if match:
                login = match.group(1)
        if login:
            if not bot_login(login):
                logins.add(login)
        elif name and not bot_name(name):
            names.add(name)
        # Co-authored-by trailers are credits even when the commit author is a
        # maintainer or a bot.
        for line in message.splitlines():
            trailer = line.strip()
            if not trailer.lower().startswith("co-authored-by:"):
                continue
            trailer = trailer.split(":", 1)[1].strip()
            match = re.match(r"^(.*?)\s*<([^>]+)>$", trailer)
            if not match:
                continue
            co_name, co_email = normalize_space(match.group(1)), match.group(2)
            noreply = NOREPLY.match(co_email)
            co_login = noreply.group(1) if noreply else ""
            if co_login and not bot_login(co_login):
                logins.add(co_login)
            elif co_name and not bot_name(co_name):
                names.add(co_name)

    if contrib_mode == "gh":
        for line in raw.splitlines():
            line = line.strip()
            if not line:
                continue
            try:
                data = json.loads(line)
            except json.JSONDecodeError:
                continue
            author = data.get("author") or {}
            commit_data = data.get("commit") or {}
            author_data = commit_data.get("author") or {}
            add(
                author.get("login") or "",
                author_data.get("name") or "",
                author_data.get("email") or "",
                commit_data.get("message") or "",
            )
    else:
        for record in raw.split("\x1e"):
            record = record.strip("\n")
            if not record.strip():
                continue
            fields = record.split("\x1f")
            if len(fields) < 2:
                continue
            name = fields[0]
            email = fields[1]
            message = fields[2] if len(fields) > 2 else ""
            match = NOREPLY.match(email.strip())
            add(match.group(1) if match else "", name, email, message)

    return sorted(logins, key=str.lower), sorted(names)


def human_list(items):
    if not items:
        return ""
    if len(items) == 1:
        return items[0]
    return ", ".join(items[:-1]) + " and " + items[-1]


def collect_asset_lines():
    if checksums_path:
        lines = []
        for line in read(checksums_path).splitlines():
            line = line.rstrip()
            if line and not line.startswith("#"):
                lines.append(line)
        return lines
    if not assets_dir:
        return []
    skip_names = {"sha256sums", "sha256sums.sig", "release-body.md"}
    files = []
    for root, _dirs, names in os.walk(assets_dir):
        for name in names:
            if name.startswith("."):
                continue
            lower = name.lower()
            if lower in skip_names or lower.endswith(".sha256") or lower.endswith(".sig"):
                continue
            files.append(os.path.join(root, name))
    files.sort(key=lambda path: os.path.basename(path).lower())
    seen = {}
    lines = []
    for path in files:
        base = os.path.basename(path)
        if base in seen:
            die(f"two assets share the name {base}: {seen[base]} and {path}")
        seen[base] = path
        digest = hashlib.sha256()
        with open(path, "rb") as handle:
            for chunk in iter(lambda: handle.read(1024 * 1024), b""):
                digest.update(chunk)
        lines.append(f"{digest.hexdigest()}  {base}")
    return lines


def changelog_for(tag, channel, version):
    text = read(changelog_path)
    if channel == "edge":
        return extract_section(text, "Unreleased") or ""
    for candidate in dict.fromkeys((version, tag, f"v{version}")):
        body = extract_section(text, candidate)
        if body is not None:
            return body
    return None


body = changelog_for(tag, channel, version)
if body is None:
    available = [
        match.group(1)
        for match in (MAIN_HEADING.match(line) for line in read(changelog_path).splitlines())
        if match
    ]
    die(
        f"CHANGELOG.md has no [{'/'.join(sorted({version, tag, 'v' + version}))}] section "
        f"(found: {', '.join(available[:6])}…)"
    )

sections = parse_sections(body)
if not sections and channel == "stable":
    die(f"CHANGELOG.md section for {version} is empty")

highlights = []
upgrade_sections = []
change_sections = []
for section in sections:
    if UPGRADE.search(section["name"]):
        upgrade_sections.append(section)
    else:
        change_sections.append(section)

explicit = next((s for s in change_sections if s["name"].lower() == "highlights"), None)
if highlights_enabled:
    if explicit:
        highlights = explicit["entries"]
        change_sections.remove(explicit)
    else:
        priority = {"added": 0, "changed": 1, "fixed": 2, "performance": 3, "security": 4}
        ordered = sorted(
            (s for s in change_sections if s["name"].lower() in priority and s["entries"]),
            key=lambda s: priority[s["name"].lower()],
        )
        for section in ordered:
            if len(highlights) >= 4:
                break
            highlights.append(f"**{section['name']}** — {summarize(section['entries'][0])}")

logins, names = collect_candidates()
thanks = [f"@{login}" for login in logins] + names

asset_lines = collect_asset_lines()
if sums_out:
    with open(sums_out, "w", encoding="utf-8") as handle:
        handle.write("\n".join(asset_lines) + ("\n" if asset_lines else ""))

short = commit[:7]
repo_suffix = f"https://github.com/{repo}" if repo else ""
commit_link = f"{repo_suffix}/commit/{commit}" if repo_suffix else ""

out = []
out.append(f"<!-- generated by scripts/release-notes.sh for {tag}; edit CHANGELOG.md, not this body -->")
out.append("")

if channel == "edge":
    out.append("> **Edge build** — rolling prerelease built from `main`. Not for production.")
    out.append(
        f"> It is published as a GitHub prerelease, so the stable updater endpoint "
        f"(`/releases/latest/download/latest.json`) keeps pointing at the newest `vX.Y.Z` release."
    )
    out.append("")

if highlights:
    out.append("## Highlights")
    out.append("")
    for item in highlights:
        out.append(f"- {item}")
    out.append("")

for section in upgrade_sections:
    out.append("## Upgrade notes")
    out.append("")
    for item in section["entries"]:
        out.append(f"- {item}")
    out.append("")

out.append("## What's changed")
out.append("")
if change_sections:
    for section in change_sections:
        if section["name"]:
            out.append(f"### {section['name']}")
            out.append("")
        for item in section["entries"]:
            out.append(f"- {item}")
        out.append("")
else:
    out.append("- Maintenance release: dependency, CI and documentation updates.")
    out.append("")

out.append("## Contributors")
out.append("")
if thanks:
    out.append(f"Thanks to {human_list(thanks)} for contributing to this release.")
else:
    out.append("Thanks to everyone who reported issues and tested this build.")
out.append("")
if previous:
    out.append(f"Changes since [`{previous}`]({repo_suffix}/releases/tag/{previous})." if repo_suffix else f"Changes since `{previous}`.")
    out.append("")

out.append("## Assets and checksums")
out.append("")
if asset_lines:
    out.append("`SHA256SUMS` is attached to this release and covers every other attached file:")
    out.append("")
    out.append("```text")
    out.extend(asset_lines)
    out.append("```")
elif channel == "stable":
    out.append(
        "`SHA256SUMS` and its minisign signature are attached to the published release; "
        "the per-file `.sha256` files carry the same digests."
    )
else:
    out.append(
        "Download the artifacts from this page; the per-file `.sha256` files carry the same digests."
    )
out.append("")

out.append("## How to verify")
out.append("")
version_line = f"- Version `{version}` from tag `{tag}`, commit "
version_line += f"[`{short}`]({commit_link})." if commit_link else f"`{commit}`."
out.append(version_line)
out.append(
    "- Installers are signed with the updater minisign key; `latest.json` carries those "
    "signatures and the app refuses a tampered or unsigned bundle."
)
if sums_signature:
    out.append(f"- `{sums_signature}` signs `SHA256SUMS` with the same updater key.")
out.append("")
use_sums = bool(asset_lines) or bool(checksums_path) or channel == "stable"
if use_sums:
    out.append("```sh")
    out.append(f"gh release download {tag} --pattern SHA256SUMS")
    out.append(f"gh release download {tag} --pattern 'Ludex*'")
    out.append("sha256sum -c SHA256SUMS --ignore-missing")
    out.append("```")
    out.append("")
else:
    out.append("Each bundle ships next to a `.sha256` file; check the one you downloaded:")
    out.append("")
    out.append("```sh")
    out.append(f"gh release download {tag} --pattern '*.AppImage*'")
    out.append("sha256sum -c Ludex_*.AppImage.sha256")
    out.append("```")
    out.append("")
if sums_signature:
    out.append("Verify the checksums file itself against the committed updater key:")
    out.append("")
    out.append("```sh")
    out.append(
        "python3 -c 'import base64,json;print(base64.b64decode(json.load(open("
        "\"apps/desktop/src-tauri/tauri.conf.json\"))[\"plugins\"][\"updater\"][\"pubkey\"]"
        ").decode(),end=\"\")' > ludex.pub"
    )
    out.append(f"base64 -d {sums_signature} > SHA256SUMS.minisig")
    out.append("minisign -Vm SHA256SUMS -p ludex.pub -x SHA256SUMS.minisig")
    out.append("```")
    out.append("")
out.append(
    "Then launch the app and check Settings → Updates: the version and commit must match "
    "the values above (`LUDEX_BUILD_COMMIT` is stamped into every bundle)."
)
out.append("")

text = "\n".join(out).replace("\n\n\n", "\n\n").rstrip() + "\n"

if out_path:
    with open(out_path, "w", encoding="utf-8") as handle:
        handle.write(text)
    print(f"release body -> {out_path} ({len(text.splitlines())} lines, {len(asset_lines)} assets)", file=sys.stderr)
else:
    sys.stdout.write(text)
PY
