#!/usr/bin/env bash
# Development helper: run ludex-server and the Vite dev server together.
#
#   scripts/dev.sh
#   LUDEX_DEV_PORT=8080 LUDEX_WEB_PORT=5173 scripts/dev.sh
#
# The Vite dev server is expected to proxy /api to LUDEX_DEV_PORT.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

SERVER_PORT="${LUDEX_DEV_PORT:-8080}"
WEB_PORT="${LUDEX_WEB_PORT:-5173}"
DEV_DIR="$ROOT/.dev"
DATA_DIR="$DEV_DIR/data"
GAMES_DIR="$DEV_DIR/games"
MEDIA_DIR="$DEV_DIR/media"
SERVER_LOG="$DEV_DIR/server.log"

mkdir -p "$DATA_DIR" "$GAMES_DIR" "$MEDIA_DIR"

if [[ ! -d node_modules ]]; then
  echo "== installing web dependencies =="
  pnpm install
fi

PID=""
cleanup() {
  trap - EXIT INT TERM
  if [[ -n "$PID" ]] && kill -0 "$PID" 2>/dev/null; then
    kill "$PID" 2>/dev/null || true
    wait "$PID" 2>/dev/null || true
  fi
  exit 0
}
trap cleanup EXIT INT TERM

echo "== starting ludex-server on 127.0.0.1:$SERVER_PORT =="
LUDEX_BIND="127.0.0.1:$SERVER_PORT" \
LUDEX_DATA_DIR="$DATA_DIR" \
LUDEX_GAMES_DIR="$GAMES_DIR" \
LUDEX_MEDIA_DIR="$MEDIA_DIR" \
LUDEX_SERVER_NAME="Ludex Dev" \
RUST_LOG="${RUST_LOG:-ludex_server=debug,tower_http=info}" \
cargo run -p ludex-server >"$SERVER_LOG" 2>&1 &
PID=$!

READY=0
for _ in $(seq 1 120); do
  if curl -fsS "http://127.0.0.1:$SERVER_PORT/api/health" >/dev/null 2>&1; then
    READY=1
    break
  fi
  if ! kill -0 "$PID" 2>/dev/null; then
    echo "server exited; log:" >&2
    tail -n 80 "$SERVER_LOG" >&2 || true
    exit 1
  fi
  sleep 0.5
done
[[ "$READY" -eq 1 ]] || { echo "server did not become healthy; log:" >&2; tail -n 80 "$SERVER_LOG" >&2; exit 1; }

echo "== API:    http://127.0.0.1:$SERVER_PORT =="
echo "== web UI: http://127.0.0.1:$WEB_PORT (proxying /api) =="
echo "== log:    $SERVER_LOG =="

pnpm --filter @ludex/web dev -- --port "$WEB_PORT" --strictPort
