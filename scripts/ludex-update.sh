#!/usr/bin/env bash
# Pull-based Ludex update agent.
#
# Downloads the release artifact for the configured channel, compares its
# SHA-256 with the installed binary, and only then stops/restarts ludex.service.
# The games, data and media directories are never touched.
#
# Usage:
#   ludex-update             # apply an update if one is available
#   ludex-update --check     # dry-run: 0 = up to date, 10 = update available
#
# Environment:
#   LUDEX_UPDATE_CHANNEL     edge (default) or stable
#   LUDEX_UPDATE_TAG         release tag; defaults to "edge" for the edge channel.
#                            Required for stable, e.g. v0.2.0.
#   LUDEX_UPDATE_BASE_URL    default https://github.com/cleeryy/ludex/releases/download
#   LUDEX_UPDATE_INSTALL_DIR default /srv/ludex
#   LUDEX_UPDATE_SERVICE     default ludex.service
#   LUDEX_UPDATE_HEALTH_URL  default http://127.0.0.1:8080/api/health
#   LUDEX_UPDATE_LOCK        default /run/ludex-update.lock
set -euo pipefail

CHANNEL="${LUDEX_UPDATE_CHANNEL:-edge}"
BASE_URL="${LUDEX_UPDATE_BASE_URL:-https://github.com/cleeryy/ludex/releases/download}"
BASE_URL="${BASE_URL%/}"
INSTALL_DIR="${LUDEX_UPDATE_INSTALL_DIR:-/srv/ludex}"
SERVICE="${LUDEX_UPDATE_SERVICE:-ludex.service}"
HEALTH_URL="${LUDEX_UPDATE_HEALTH_URL:-http://127.0.0.1:8080/api/health}"
LOCK_FILE="${LUDEX_UPDATE_LOCK:-/run/ludex-update.lock}"

CHECK=0
case "${1:-}" in
  --check)
    CHECK=1
    ;;
  --help | -h)
    sed -n '2,20p' "${BASH_SOURCE[0]}"
    exit 0
    ;;
  "")
    ;;
  *)
    echo "Unknown argument: $1 (use --check or --help)" >&2
    exit 2
    ;;
esac

fail() {
  echo "ludex-update: $*" >&2
  exit 1
}

log() {
  echo "ludex-update: $*"
}

command -v curl >/dev/null 2>&1 || fail "curl is required"
command -v sha256sum >/dev/null 2>&1 || fail "sha256sum is required"
command -v tar >/dev/null 2>&1 || fail "tar is required"

case "$CHANNEL" in
  edge | stable) ;;
  *) fail "LUDEX_UPDATE_CHANNEL must be 'edge' or 'stable' (got '$CHANNEL')" ;;
esac

TAG="${LUDEX_UPDATE_TAG:-}"
if [[ -z "$TAG" ]]; then
  if [[ "$CHANNEL" == "edge" ]]; then
    TAG="edge"
  else
    fail "LUDEX_UPDATE_TAG is required for the stable channel (for example v0.2.0)"
  fi
fi

ASSET="ludex-server-${TAG}-x86_64-unknown-linux-gnu.tar.gz"
REMOTE_SHA_URL="${BASE_URL}/${TAG}/${ASSET}.sha256"
REMOTE_TAR_URL="${BASE_URL}/${TAG}/${ASSET}"
LOCAL_BIN="${INSTALL_DIR}/ludex-server"
LOCAL_SHA_FILE="${INSTALL_DIR}/ludex-server.sha256"
BACKUP_BIN="${INSTALL_DIR}/ludex-server.previous"
BACKUP_SHA_FILE="${INSTALL_DIR}/ludex-server.previous.sha256"

if [[ "$CHECK" -eq 0 ]]; then
  command -v systemctl >/dev/null 2>&1 || fail "systemctl is required to apply updates"
  command -v flock >/dev/null 2>&1 || fail "flock is required to apply updates"
  exec 9>"$LOCK_FILE"
  if ! flock -n 9; then
    log "another update is already running; skipping"
    exit 0
  fi
fi

TMP="$(mktemp -d "${TMPDIR:-/tmp}/ludex-update.XXXXXX")"
cleanup() {
  rm -rf "$TMP"
}
trap cleanup EXIT

log "channel=$CHANNEL tag=$TAG install=$INSTALL_DIR"

if ! curl -fsSL --retry 3 --connect-timeout 10 --max-time 60 \
  "$REMOTE_SHA_URL" -o "$TMP/remote.sha256"; then
  fail "could not download $REMOTE_SHA_URL"
fi

REMOTE_SHA="$(awk 'NR == 1 { print $1 }' "$TMP/remote.sha256")"
[[ "$REMOTE_SHA" =~ ^[0-9a-fA-F]{64}$ ]] || fail "remote checksum file is malformed"

LOCAL_SHA=""
if [[ -f "$LOCAL_SHA_FILE" ]]; then
  LOCAL_SHA="$(awk 'NR == 1 { print $1 }' "$LOCAL_SHA_FILE")"
elif [[ -x "$LOCAL_BIN" ]]; then
  LOCAL_SHA="$(sha256sum "$LOCAL_BIN" | awk '{ print $1 }')"
fi

if [[ "${LOCAL_SHA,,}" == "${REMOTE_SHA,,}" ]]; then
  log "up to date ($REMOTE_SHA)"
  exit 0
fi

if [[ "$CHECK" -eq 1 ]]; then
  log "update available: local=${LOCAL_SHA:-none} remote=$REMOTE_SHA"
  exit 10
fi

log "downloading $REMOTE_TAR_URL"
if ! curl -fsSL --retry 3 --connect-timeout 10 --max-time 600 \
  "$REMOTE_TAR_URL" -o "$TMP/$ASSET"; then
  fail "could not download $REMOTE_TAR_URL"
fi

if ! (cd "$TMP" && printf '%s  %s\n' "$REMOTE_SHA" "$ASSET" | sha256sum -c - >/dev/null); then
  fail "checksum mismatch for $ASSET"
fi

mkdir -p "$TMP/extract"
tar -xzf "$TMP/$ASSET" -C "$TMP/extract"
NEW_BIN="$TMP/extract/ludex-server"
[[ -x "$NEW_BIN" ]] || fail "archive does not contain an executable ludex-server"

if [[ ! -d "$INSTALL_DIR" ]]; then
  fail "install directory does not exist: $INSTALL_DIR"
fi

install -m 0755 "$NEW_BIN" "${LOCAL_BIN}.new"

# Keep the previous binary and checksum for automatic/manual rollback.
if [[ -f "$LOCAL_BIN" ]]; then
  cp -a "$LOCAL_BIN" "$BACKUP_BIN"
fi
if [[ -f "$LOCAL_SHA_FILE" ]]; then
  cp -a "$LOCAL_SHA_FILE" "$BACKUP_SHA_FILE"
fi

rollback() {
  log "rolling back to the previous binary"
  systemctl stop "$SERVICE" 2>/dev/null || true
  if [[ -f "$BACKUP_BIN" ]]; then
    install -m 0755 "$BACKUP_BIN" "$LOCAL_BIN"
  else
    log "no previous binary to restore"
  fi
  if [[ -f "$BACKUP_SHA_FILE" ]]; then
    cp -a "$BACKUP_SHA_FILE" "$LOCAL_SHA_FILE"
  else
    rm -f "$LOCAL_SHA_FILE"
  fi
  systemctl start "$SERVICE" 2>/dev/null || true
  fail "update failed; previous binary restored"
}

log "stopping $SERVICE"
systemctl stop "$SERVICE"
mv -f "${LOCAL_BIN}.new" "$LOCAL_BIN"
chmod 0755 "$LOCAL_BIN"
printf '%s  %s\n' "$REMOTE_SHA" "ludex-server" >"$LOCAL_SHA_FILE"

log "starting $SERVICE"
if ! systemctl start "$SERVICE"; then
  log "systemctl start failed"
  rollback
fi

HEALTHY=0
for _ in $(seq 1 30); do
  if curl -fsS --max-time 3 "$HEALTH_URL" >/dev/null 2>&1; then
    HEALTHY=1
    break
  fi
  sleep 1
done

if [[ "$HEALTHY" -ne 1 ]]; then
  log "health check failed: $HEALTH_URL"
  rollback
fi

log "updated to $REMOTE_SHA"
