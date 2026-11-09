#!/usr/bin/env bash
# End-to-end API smoke test for ludex-server.
#
# Builds (if needed) and starts the server on a random local port with throwaway
# directories, creates the admin account, drops a synthetic archive in the games
# folder, scans, downloads a byte range and checks HTTP 206.
#
# Usage:
#   scripts/smoke.sh
#   LUDEX_SMOKE_BIN=/path/to/ludex-server scripts/smoke.sh   # skip build
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

command -v curl >/dev/null 2>&1 || fail "curl is required"
command -v python3 >/dev/null 2>&1 || fail "python3 is required"

PORT="${LUDEX_SMOKE_PORT:-}"
if [[ -z "$PORT" ]]; then
  PORT="$(python3 - <<'PY'
import socket
sock = socket.socket()
sock.bind(("127.0.0.1", 0))
print(sock.getsockname()[1])
sock.close()
PY
)"
fi

TMP="$(mktemp -d "${TMPDIR:-/tmp}/ludex-smoke.XXXXXX")"
DATA_DIR="$TMP/data"
GAMES_DIR="$TMP/games"
MEDIA_DIR="$TMP/media"
SERVER_LOG="$TMP/server.log"
mkdir -p "$DATA_DIR" "$GAMES_DIR" "$MEDIA_DIR"

PID=""
cleanup() {
  local code=$?
  trap - EXIT INT TERM
  if [[ -n "$PID" ]] && kill -0 "$PID" 2>/dev/null; then
    kill "$PID" 2>/dev/null || true
    for _ in $(seq 1 50); do
      kill -0 "$PID" 2>/dev/null || break
      sleep 0.1
    done
    kill -9 "$PID" 2>/dev/null || true
    wait "$PID" 2>/dev/null || true
  fi
  if [[ $code -ne 0 && -f "$SERVER_LOG" ]]; then
    echo "--- server log (last 80 lines) ---" >&2
    tail -n 80 "$SERVER_LOG" >&2 || true
  fi
  rm -rf "$TMP"
  exit "$code"
}
trap cleanup EXIT INT TERM

json_get() {
  # json_get 'a.b.0.c' reads stdin and prints the selected scalar.
  python3 -c '
import json, sys
value = json.load(sys.stdin)
for key in sys.argv[1].split("."):
    value = value[int(key)] if isinstance(value, list) else value[key]
print(value)
' "$1"
}

BIN="${LUDEX_SMOKE_BIN:-$ROOT/target/debug/ludex-server}"
if [[ ! -x "$BIN" ]]; then
  echo "== building ludex-server (debug) =="
  if [[ -f Cargo.lock ]]; then
    cargo build --locked -p ludex-server
  else
    cargo build -p ludex-server
  fi
fi
[[ -x "$BIN" ]] || fail "server binary not found at $BIN"

BASE="http://127.0.0.1:$PORT"

echo "== starting server on $BASE =="
LUDEX_BIND="127.0.0.1:$PORT" \
LUDEX_DATA_DIR="$DATA_DIR" \
LUDEX_GAMES_DIR="$GAMES_DIR" \
LUDEX_MEDIA_DIR="$MEDIA_DIR" \
LUDEX_SERVER_NAME="Ludex Smoke" \
LUDEX_JWT_SECRET="ludex-smoke-test-secret-0123456789abcdef" \
RUST_LOG="ludex_server=warn,tower_http=warn" \
"$BIN" >"$SERVER_LOG" 2>&1 &
PID=$!

echo "== waiting for /api/health =="
READY=0
for _ in $(seq 1 120); do
  if curl -fsS "$BASE/api/health" >/dev/null 2>&1; then
    READY=1
    break
  fi
  kill -0 "$PID" 2>/dev/null || fail "server exited during startup"
  sleep 0.5
done
[[ "$READY" -eq 1 ]] || fail "health endpoint did not become ready in 60s"

echo "== checking unauthenticated access is rejected =="
UNAUTH_CODE="$(curl -sS -o /dev/null -w '%{http_code}' "$BASE/api/games")"
[[ "$UNAUTH_CODE" == "401" ]] || fail "expected 401 without a token, got $UNAUTH_CODE"

echo "== creating admin account =="
SETUP_JSON="$(curl -fsS -X POST "$BASE/api/setup" \
  -H 'content-type: application/json' \
  -d '{"username":"admin","password":"SmokeTest-Password-123","serverName":"Ludex Smoke"}')"
TOKEN="$(printf '%s' "$SETUP_JSON" | json_get token)"
[[ -n "$TOKEN" ]] || fail "setup did not return a token"

echo "== logging in =="
LOGIN_JSON="$(curl -fsS -X POST "$BASE/api/auth/login" \
  -H 'content-type: application/json' \
  -d '{"username":"admin","password":"SmokeTest-Password-123"}')"
LOGIN_TOKEN="$(printf '%s' "$LOGIN_JSON" | json_get token)"
[[ -n "$LOGIN_TOKEN" ]] || fail "login did not return a token"

echo "== creating a synthetic archive in the games folder =="
ARCHIVE="$GAMES_DIR/Smoke Test (2024).zip"
python3 - "$ARCHIVE" <<'PY'
import sys
import zipfile

# ZIP_STORED keeps the file above the scanner's 1 MiB minimum.
with zipfile.ZipFile(sys.argv[1], "w", zipfile.ZIP_STORED) as archive:
    archive.writestr("readme.txt", "ludex smoke test\n")
    archive.writestr("payload.bin", b"0" * (2 * 1024 * 1024))
PY

echo "== triggering a library scan =="
curl -fsS -X POST "$BASE/api/library/scan" \
  -H "authorization: Bearer $TOKEN" \
  -H 'content-type: application/json' \
  -d '{}' >/dev/null

echo "== polling until the archive is indexed =="
GAME_ID=""
for _ in $(seq 1 120); do
  LIST_JSON="$(curl -fsS "$BASE/api/games?q=Smoke&limit=10" \
    -H "authorization: Bearer $TOKEN")"
  TOTAL="$(printf '%s' "$LIST_JSON" | json_get total)"
  if [[ "$TOTAL" -ge 1 ]]; then
    GAME_ID="$(printf '%s' "$LIST_JSON" | json_get 'items.0.id')"
    break
  fi
  sleep 0.5
done

if [[ -z "$GAME_ID" ]]; then
  SCAN_JSON="$(curl -fsS "$BASE/api/library/scan" -H "authorization: Bearer $TOKEN")"
  echo "scan status: $SCAN_JSON" >&2
  fail "archive was not indexed within 60s"
fi

echo "== reading game detail =="
DETAIL_JSON="$(curl -fsS "$BASE/api/games/$GAME_ID" -H "authorization: Bearer $TOKEN")"
VERSION_ID="$(printf '%s' "$DETAIL_JSON" | json_get 'versions.0.id')"
[[ -n "$VERSION_ID" ]] || fail "game $GAME_ID has no version"

echo "== downloading bytes 0-99 with a JWT =="
RANGE_CODE="$(curl -sS -o "$TMP/range.bin" -D "$TMP/range.headers" -w '%{http_code}' \
  -H "authorization: Bearer $TOKEN" \
  -H 'Range: bytes=0-99' \
  "$BASE/api/games/$GAME_ID/download?versionId=$VERSION_ID")"
[[ "$RANGE_CODE" == "206" ]] || fail "expected 206 for ranged download, got $RANGE_CODE"
RANGE_SIZE="$(wc -c <"$TMP/range.bin" | tr -d ' ')"
[[ "$RANGE_SIZE" == "100" ]] || fail "expected 100 bytes, got $RANGE_SIZE"
grep -qi '^content-range: bytes 0-99/' "$TMP/range.headers" \
  || fail "missing or invalid Content-Range header"

echo "== downloading with a short-lived ticket (no JWT) =="
TICKET_JSON="$(curl -fsS -X POST "$BASE/api/games/$GAME_ID/download-ticket" \
  -H "authorization: Bearer $TOKEN" \
  -H 'content-type: application/json' \
  -d "{\"versionId\":$VERSION_ID}")"
TICKET_URL="$(printf '%s' "$TICKET_JSON" | json_get url)"
case "$TICKET_URL" in
  http*) ;;
  /*) TICKET_URL="$BASE$TICKET_URL" ;;
  *) TICKET_URL="$BASE/$TICKET_URL" ;;
esac
TICKET_CODE="$(curl -sS -o /dev/null -w '%{http_code}' \
  -H 'Range: bytes=0-9' "$TICKET_URL")"
[[ "$TICKET_CODE" == "206" ]] || fail "expected 206 from ticket URL, got $TICKET_CODE"

echo
echo "PASS: health, setup, login, scan, listing, JWT range download and ticket download"
