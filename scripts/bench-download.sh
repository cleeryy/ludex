#!/usr/bin/env bash
# Measure one download URL the way docs/PERFORMANCE.md describes (#194).
#
#   scripts/bench-download.sh '<url>' ['<bearer token>']
#
# The URL is a Ludex download URL, direct or through Caddy, with a ticket
# (`?versionId=…&token=…`) or protected by an Authorization header. The script
# runs one HTTP/2 and one HTTP/1.1 request, then aria2c -x8 -s8 when installed.
# It never uploads anything and writes only to a temporary file.
set -euo pipefail

url=${1:?usage: scripts/bench-download.sh <download-url> [bearer-token]}
token=${2:-}
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

auth=()
if [[ -n "$token" ]]; then
  auth=(-H "Authorization: Bearer $token")
fi

run_curl() {
  local label=$1
  shift
  printf '%-14s ' "$label"
  # %{speed_download} is bytes/s; the shell divides once for MB/s.
  local out
  out=$(curl -sS -L --max-time 3600 -o "$tmp/body" \
    -w '%{http_code} %{speed_download} %{time_total} %{size_download}' \
    "${auth[@]}" "$@" "$url")
  awk -v label="$label" '{
    printf "HTTP %s  %.1f MB/s  %.1fs  %.2f GB\n", $1, $2/1048576, $3, $4/1073741824
  }' <<<"$out"
}

echo "URL: $url"
echo
run_curl "HTTP/2" "--http2"
run_curl "HTTP/1.1" "--http1.1"

if command -v aria2c >/dev/null 2>&1; then
  echo
  echo "aria2c -x8 -s8 (parallel ranges)"
  # A ticket URL is enough; aria2c does not follow our bearer header.
  aria2c -x8 -s8 -k 1M --summary-interval=0 --console-log-level=warn \
    -d "$tmp" -o aria.bin "$url" >/dev/null
  size=$(stat -c %s "$tmp/aria.bin" 2>/dev/null || stat -f %z "$tmp/aria.bin")
  echo "aria2c downloaded $size bytes (time it with \`time\` for MB/s)"
else
  echo
  echo "aria2c not installed; skip the parallel-range comparison"
fi

echo
echo "Record these numbers in the issue with the client link, server disk and"
echo "Caddy version; see docs/PERFORMANCE.md for the full matrix."
