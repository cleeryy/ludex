#!/usr/bin/env bash
# Full browser E2E harness for Ludex.
#
# Builds the web UI and the debug server, starts two isolated server instances
# (one with an admin + seeded archive, one fresh for the setup flow) and a tiny
# HTTP server for the ingest test, then runs Playwright/Chromium.
#
# Usage:
#   scripts/e2e.sh
#   LUDEX_E2E_BIN=/path/to/ludex-server scripts/e2e.sh   # reuse an existing binary
#   LUDEX_E2E_SKIP_WEB_BUILD=1 scripts/e2e.sh            # reuse apps/web/dist
#
# Requires: bash, curl, python3, pnpm, Node 22, Playwright Chromium.
# CI installs the browser with `pnpm e2e:install` before this script.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

command -v curl >/dev/null 2>&1 || fail "curl is required"
command -v python3 >/dev/null 2>&1 || fail "python3 is required"
command -v pnpm >/dev/null 2>&1 || fail "pnpm is required"

TMP="$(mktemp -d "${TMPDIR:-/tmp}/ludex-e2e.XXXXXX")"
MAIN_DATA="$TMP/main/data"
MAIN_GAMES="$TMP/main/games"
MAIN_MEDIA="$TMP/main/media"
SETUP_DATA="$TMP/setup/data"
SETUP_GAMES="$TMP/setup/games"
SETUP_MEDIA="$TMP/setup/media"
INGEST_DIR="$TMP/ingest"
MAIN_LOG="$TMP/main-server.log"
SETUP_LOG="$TMP/setup-server.log"
INGEST_LOG="$TMP/ingest-http.log"
mkdir -p "$MAIN_DATA" "$MAIN_GAMES" "$MAIN_MEDIA" \
  "$SETUP_DATA" "$SETUP_GAMES" "$SETUP_MEDIA" "$INGEST_DIR"

PIDS=()
cleanup() {
  local code=$?
  trap - EXIT INT TERM
  if [[ $code -ne 0 ]]; then
    for log in "$MAIN_LOG" "$SETUP_LOG" "$INGEST_LOG"; do
      if [[ -f "$log" ]]; then
        echo "--- $(basename "$log") (last 80 lines) ---" >&2
        tail -n 80 "$log" >&2 || true
      fi
    done
  fi
  if ((${#PIDS[@]})); then
    for pid in "${PIDS[@]}"; do
      kill "$pid" 2>/dev/null || true
      for _ in $(seq 1 50); do
        kill -0 "$pid" 2>/dev/null || break
        sleep 0.1
      done
      kill -9 "$pid" 2>/dev/null || true
      wait "$pid" 2>/dev/null || true
    done
  fi
  rm -rf "$TMP"
  exit "$code"
}
trap cleanup EXIT INT TERM

json_path() {
  # json_path 'items.0.id' reads JSON on stdin.
  python3 -c '
import json, sys
value = json.load(sys.stdin)
for key in sys.argv[1].split("."):
    value = value[int(key)] if isinstance(value, list) else value[key]
print(value)
' "$1"
}

# Three unique loopback ports, held open until all are chosen.
read -r MAIN_PORT SETUP_PORT INGEST_PORT < <(python3 - <<'PY'
import socket

sockets = []
ports = []
for _ in range(3):
    sock = socket.socket()
    sock.bind(("127.0.0.1", 0))
    sockets.append(sock)
    ports.append(sock.getsockname()[1])
print(*ports)
for sock in sockets:
    sock.close()
PY
)

wait_health() {
  local base=$1
  local pid=$2
  local log=$3
  for _ in $(seq 1 120); do
    if curl -fsS --max-time 2 "$base/api/health" >/dev/null 2>&1; then
      return 0
    fi
    if ! kill -0 "$pid" 2>/dev/null; then
      echo "server exited during startup; log:" >&2
      tail -n 80 "$log" >&2 || true
      return 1
    fi
    sleep 0.5
  done
  echo "health endpoint did not become ready in 60s; log:" >&2
  tail -n 80 "$log" >&2 || true
  return 1
}

echo "== building web UI =="
if [[ "${LUDEX_E2E_SKIP_WEB_BUILD:-0}" != "1" ]]; then
  pnpm --filter @ludex/web build
fi
[[ -f "$ROOT/apps/web/dist/index.html" ]] || fail "apps/web/dist/index.html is missing"

BIN="${LUDEX_E2E_BIN:-$ROOT/target/debug/ludex-server}"
if [[ -z "${LUDEX_E2E_BIN:-}" ]]; then
  command -v cargo >/dev/null 2>&1 || fail "cargo is required to build ludex-server (or set LUDEX_E2E_BIN)"
  echo "== building ludex-server (debug) =="
  if [[ -f Cargo.lock ]]; then
    cargo build --locked -p ludex-server
  else
    cargo build -p ludex-server
  fi
fi
[[ -x "$BIN" ]] || fail "server binary not found or not executable: $BIN"

echo "== creating fixtures =="
python3 - "$MAIN_GAMES/Ludex E2E Game (2024).zip" "$INGEST_DIR/ingest-game.zip" <<'PY'
import sys
import zipfile


def make(path: str, payload_size: int, marker: str) -> None:
    # ZIP_STORED keeps the archive above the scanner's 1 MiB minimum.
    with zipfile.ZipFile(path, "w", zipfile.ZIP_STORED) as archive:
        archive.writestr("readme.txt", marker)
        archive.writestr("payload.bin", b"0" * payload_size)


make(sys.argv[1], 2 * 1024 * 1024, "ludex-e2e-known-file")
make(sys.argv[2], 1200 * 1024, "ludex-e2e-ingest")
PY

echo "== starting local ingest HTTP server on 127.0.0.1:$INGEST_PORT =="
python3 -m http.server "$INGEST_PORT" --bind 127.0.0.1 --directory "$INGEST_DIR" \
  >"$INGEST_LOG" 2>&1 &
PIDS+=("$!")
INGEST_URL="http://127.0.0.1:$INGEST_PORT/ingest-game.zip"
for _ in $(seq 1 40); do
  if curl -fsSI --max-time 2 "$INGEST_URL" >/dev/null 2>&1; then break; fi
  sleep 0.25
done
curl -fsSI --max-time 2 "$INGEST_URL" >/dev/null || fail "ingest HTTP server did not start"

echo "== starting main server on 127.0.0.1:$MAIN_PORT =="
LUDEX_BIND="127.0.0.1:$MAIN_PORT" \
LUDEX_DATA_DIR="$MAIN_DATA" \
LUDEX_GAMES_DIR="$MAIN_GAMES" \
LUDEX_MEDIA_DIR="$MAIN_MEDIA" \
LUDEX_WEB_DIR="$ROOT/apps/web/dist" \
LUDEX_SCAN_INTERVAL_SECONDS=0 \
LUDEX_INGEST_ALLOW_PRIVATE=true \
LUDEX_SERVER_NAME="Ludex E2E" \
RUST_LOG="ludex_server=warn,tower_http=warn" \
"$BIN" >"$MAIN_LOG" 2>&1 &
MAIN_PID=$!
PIDS+=("$MAIN_PID")
wait_health "http://127.0.0.1:$MAIN_PORT" "$MAIN_PID" "$MAIN_LOG" \
  || fail "main server did not become healthy"

MAIN_BASE="http://127.0.0.1:$MAIN_PORT"

echo "== creating the admin and indexing the seeded archive =="
SETUP_JSON="$(curl -fsS -X POST "$MAIN_BASE/api/setup" \
  -H 'content-type: application/json' \
  -d '{"username":"admin","password":"E2E-Password-123","serverName":"Ludex E2E"}')"
TOKEN="$(printf '%s' "$SETUP_JSON" | json_path token)"
[[ -n "$TOKEN" ]] || fail "setup did not return a token"

curl -fsS -X POST "$MAIN_BASE/api/library/scan" \
  -H "authorization: Bearer $TOKEN" \
  -H 'content-type: application/json' \
  -d '{}' >/dev/null

GAME_ID=""
for _ in $(seq 1 120); do
  LIST_JSON="$(curl -fsS "$MAIN_BASE/api/games?q=Ludex%20E2E&limit=10" \
    -H "authorization: Bearer $TOKEN")"
  TOTAL="$(printf '%s' "$LIST_JSON" | json_path total)"
  if [[ "$TOTAL" -ge 1 ]]; then
    GAME_ID="$(printf '%s' "$LIST_JSON" | json_path 'items.0.id')"
    break
  fi
  sleep 0.5
done
[[ -n "$GAME_ID" ]] || fail "seeded archive was not indexed within 60s"

curl -fsS -X PATCH "$MAIN_BASE/api/games/$GAME_ID" \
  -H "authorization: Bearer $TOKEN" \
  -H 'content-type: application/json' \
  -d '{"description":"A deterministic end-to-end test game."}' >/dev/null

echo "== starting fresh setup server on 127.0.0.1:$SETUP_PORT =="
LUDEX_BIND="127.0.0.1:$SETUP_PORT" \
LUDEX_DATA_DIR="$SETUP_DATA" \
LUDEX_GAMES_DIR="$SETUP_GAMES" \
LUDEX_MEDIA_DIR="$SETUP_MEDIA" \
LUDEX_WEB_DIR="$ROOT/apps/web/dist" \
LUDEX_SCAN_INTERVAL_SECONDS=0 \
LUDEX_SERVER_NAME="Ludex E2E Setup" \
RUST_LOG="ludex_server=warn,tower_http=warn" \
"$BIN" >"$SETUP_LOG" 2>&1 &
SETUP_PID=$!
PIDS+=("$SETUP_PID")
wait_health "http://127.0.0.1:$SETUP_PORT" "$SETUP_PID" "$SETUP_LOG" \
  || fail "setup server did not become healthy"

echo "== running Playwright =="
export LUDEX_E2E_BASE_URL="$MAIN_BASE"
export LUDEX_E2E_TOKEN="$TOKEN"
export LUDEX_E2E_SETUP_URL="http://127.0.0.1:$SETUP_PORT"
export LUDEX_E2E_INGEST_URL="$INGEST_URL"
export LUDEX_E2E_GAME_TITLE="Ludex E2E Game"
export LUDEX_E2E_GAME_FILE="Ludex E2E Game (2024).zip"
export LUDEX_E2E_DESCRIPTION="A deterministic end-to-end test game."
export LUDEX_E2E_INGEST_TITLE="Ingested E2E Game"

pnpm --dir "$ROOT/e2e" exec playwright test

echo
echo "PASS: browser E2E (library, search/filter, detail, download, setup, ingest, navigation, Big Picture)"
