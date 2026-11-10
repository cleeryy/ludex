#!/usr/bin/env bash
# Headless desktop smoke test: the Tauri window must open under Xvfb.
#
# Run it with:
#   xvfb-run -a -s "-screen 0 1440x900x24" scripts/desktop-smoke.sh
#
# Requires xdotool. The app is expected to already be built:
#   pnpm --dir apps/desktop tauri build --debug --no-bundle
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

[[ -n "${DISPLAY:-}" ]] || fail "DISPLAY is not set; run this script under xvfb-run"
command -v xdotool >/dev/null 2>&1 || fail "xdotool is required"

BIN="${LUDEX_DESKTOP_BIN:-$ROOT/apps/desktop/src-tauri/target/debug/ludex}"
[[ -x "$BIN" ]] || fail "desktop binary not found at $BIN (build it with tauri build --debug --no-bundle)"

LOG="${LUDEX_DESKTOP_LOG:-${TMPDIR:-/tmp}/ludex-desktop-smoke.log}"
APP_PID=""

cleanup() {
  local code=$?
  trap - EXIT INT TERM
  if [[ -n "$APP_PID" ]] && kill -0 "$APP_PID" 2>/dev/null; then
    kill "$APP_PID" 2>/dev/null || true
    for _ in $(seq 1 50); do
      kill -0 "$APP_PID" 2>/dev/null || break
      sleep 0.1
    done
    kill -9 "$APP_PID" 2>/dev/null || true
    wait "$APP_PID" 2>/dev/null || true
  fi
  if [[ $code -ne 0 && -f "$LOG" ]]; then
    echo "--- desktop log (last 80 lines) ---" >&2
    tail -n 80 "$LOG" >&2 || true
  fi
  exit "$code"
}
trap cleanup EXIT INT TERM

echo "== launching $BIN under DISPLAY=$DISPLAY =="
"$BIN" >"$LOG" 2>&1 &
APP_PID=$!

FOUND=0
for _ in $(seq 1 30); do
  if xdotool search --name "Ludex" >/dev/null 2>&1; then
    FOUND=1
    break
  fi
  if ! kill -0 "$APP_PID" 2>/dev/null; then
    break
  fi
  sleep 1
done

[[ "$FOUND" -eq 1 ]] || fail "no window named Ludex appeared within 30s"
echo "PASS: Ludex desktop window opened under Xvfb"
