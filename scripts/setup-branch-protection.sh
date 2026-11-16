#!/usr/bin/env bash
# Idempotent branch protection for the default branch (#263).
#
# The required checks below are exactly the job names rendered by
# .github/workflows/*.yml; docs/CI.md lists what each one gates. Re-run this
# script whenever a workflow job is added, renamed or promoted to required:
# it replaces the whole protection object, so the repository never drifts.
#
# Usage:
#   scripts/setup-branch-protection.sh --dry-run          # print the payload
#   scripts/setup-branch-protection.sh                    # apply
#   scripts/setup-branch-protection.sh --show             # current protection
#   ENFORCE_ADMINS=false scripts/setup-branch-protection.sh
#   scripts/setup-branch-protection.sh --add "Coverage (web)"
#
# Requires: gh (authenticated with admin on the repository), python3 for JSON.
# The script never pushes anything and never touches workflow files.

set -euo pipefail

branch="${BRANCH:-main}"
repo="${REPO:-}"
enforce_admins="${ENFORCE_ADMINS:-true}"
dry_run=0
show=0
extra=()

# Checks every pull request must pass before it can merge.
required=(
  "Rust (fmt, clippy, test)"
  "Rust (PostgreSQL backend)"
  "Web (install, build, typecheck)"
  "Desktop (cargo check)"
  "Desktop (fmt, clippy, test)"
  "Desktop (Windows cargo check)"
  "E2E"
  "Docker image (build only)"
  "Coverage (Rust workspace)"
  "Security (Trivy filesystem)"
  "Security (cargo deny)"
  "Security (pnpm audit)"
)

usage() {
  sed -n '2,19p' "$0" | sed 's/^# \{0,1\}//'
  cat <<'USAGE'

Options:
  --dry-run             print the payload and exit
  --show                print the current protection and exit
  --branch <name>       branch to protect (default: main, or $BRANCH)
  --repo <owner/name>   repository (default: the current directory's remote)
  --add <check name>    add a check to the required list (repeatable)
  -h, --help            this help

Environment:
  ENFORCE_ADMINS        true (default) or false: do the rules apply to admins?
  BRANCH, REPO          same as --branch / --repo
USAGE
}

while [ $# -gt 0 ]; do
  case "$1" in
    --dry-run) dry_run=1 ;;
    --show) show=1 ;;
    --branch) branch="${2:?--branch needs a value}"; shift ;;
    --repo) repo="${2:?--repo needs a value}"; shift ;;
    --add) extra+=("${2:?--add needs a value}"); shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
  shift
done

command -v gh >/dev/null 2>&1 || { echo "gh is required" >&2; exit 1; }
command -v python3 >/dev/null 2>&1 || { echo "python3 is required" >&2; exit 1; }

if [ -z "$repo" ]; then
  repo="$(gh repo view --json nameWithOwner --jq .nameWithOwner)"
fi

case "$enforce_admins" in
  true|false) ;;
  *) echo "ENFORCE_ADMINS must be true or false" >&2; exit 2 ;;
esac

contexts=("${required[@]}")
for check in "${extra[@]:-}"; do
  [ -n "$check" ] && contexts+=("$check")
done

if [ "$show" -eq 1 ]; then
  gh api "repos/$repo/branches/$branch/protection"
  exit 0
fi

payload="$(python3 - "$enforce_admins" "${contexts[@]}" <<'PY'
import json
import sys

enforce_admins = sys.argv[1] == "true"
contexts = [c for c in sys.argv[2:] if c]

# PUT replaces the whole protection object: every field we care about is
# explicit here, so the result is the same no matter what was configured
# before (that is what makes the script idempotent).
print(
    json.dumps(
        {
            "required_status_checks": {"strict": True, "contexts": contexts},
            # A solo maintainer: no review requirement, but every check above
            # must be green, including for the owner when enforce_admins is on.
            "required_pull_request_reviews": None,
            "enforce_admins": enforce_admins,
            "restrictions": None,
            "required_linear_history": True,
            "allow_force_pushes": False,
            "allow_deletions": False,
            "required_conversation_resolution": True,
        },
        indent=2,
    )
)
PY
)"

if [ "$dry_run" -eq 1 ]; then
  echo "PUT repos/$repo/branches/$branch/protection"
  printf '%s\n' "$payload"
  exit 0
fi

printf '%s\n' "$payload" |
  gh api --method PUT \
    -H "Accept: application/vnd.github+json" \
    "repos/$repo/branches/$branch/protection" \
    --input - >/dev/null

echo "branch protection applied to $repo@$branch"
gh api "repos/$repo/branches/$branch/protection" \
  --jq '{strict: .required_status_checks.strict, enforce_admins: .enforce_admins.enabled, contexts: .required_status_checks.contexts}'
