#!/usr/bin/env bash
# Network benchmark matrix for Ludex (#200; builds on #194/#195).
#
#   scripts/bench-download.sh [options] <url-through-caddy>
#
# The URL is a Ludex download URL, direct or through Caddy, with a ticket
# (`?versionId=…&token=…`) or protected by an Authorization header. The script
# runs the rows of the matrix in docs/NETWORK-ARCHITECTURE.md:
#
#   caddy h2, caddy h1.1, caddy h3 (optional), direct h1.1 (optional),
#   aria2c -x8 -s8 against Caddy (and direct), iperf3 single and 8 streams.
#
# It prints one comparison table and verdicts against the documented
# thresholds. It never uploads anything, never prints the token, and writes
# only to a temporary directory.
#
# Options:
#   -d, --direct URL    direct server URL (http://host:8080/...), no Caddy
#   -t, --token TOKEN   bearer token for curl and aria2c
#   -i, --iperf3 HOST   run iperf3 -c HOST as the link baseline
#   -r, --runs N        curl repetitions per row, best kept (default 1)
#   -3, --http3         also try HTTP/3 (curl must be built with HTTP/3)
#   -s, --self-check    offline check of the table/verdict logic, no network
#   -h, --help
#
# Legacy form, still accepted:
#   scripts/bench-download.sh <url> [bearer-token]
#
# Record the printed table, the RTT and the Caddy/server logs in the issue.
# See docs/NETWORK-ARCHITECTURE.md for the full matrix and the thresholds.
set -uo pipefail

usage() {
  sed -n '2,30p' "$0" | sed 's/^# \{0,1\}//'
}

die() {
  printf 'error: %s\n' "$*" >&2
  exit 2
}

now_ms() {
  if [[ -n "${EPOCHREALTIME:-}" ]]; then
    # Bash 5: seconds.microseconds -> milliseconds.
    local digits=${EPOCHREALTIME//[!0-9]/}
    printf '%s\n' "$((digits / 1000))"
    return
  fi
  local ns
  ns=$(date +%s%N 2>/dev/null || true)
  if [[ "$ns" =~ ^[0-9]+$ ]]; then
    printf '%s\n' "$((ns / 1000000))"
    return
  fi
  if command -v perl >/dev/null 2>&1; then
    perl -MTime::HiRes=time -e 'printf "%d\n", time() * 1000'
    return
  fi
  if command -v python3 >/dev/null 2>&1; then
    python3 -c 'import time; print(int(time.time() * 1000))'
    return
  fi
  printf '%s000\n' "$(date +%s)"
}

curl_has_h3() {
  curl --version 2>/dev/null | grep -q 'HTTP3'
}

# Never print a ticket token: `?token=…` is a 15-minute capability.
redact_url() {
  printf '%s' "$1" | sed -E 's/([?&]token=)[^&]*/\1<redacted>/'
}

# `Authorization: Bearer <token>` from the variable, never a literal
# placeholder: `--token` must reach curl and aria2c verbatim (#211, 7.1).
auth_header() {
  local value=${1:-}
  [ -n "$value" ] || return 1
  printf 'Authorization: Bearer %s' "$value"
}

# Base headers for both clients, plus the bearer header when a token is set.
# Kept in one function so `--self-check` exercises the exact call site.
set_headers() {
  local value=${1:-} header
  curl_headers=(-H "User-Agent: ludex-bench/1")
  aria_headers=(--user-agent=ludex-bench/1)
  [ -n "$value" ] || return 0
  header=$(auth_header "$value") || return 1
  curl_headers+=(-H "$header")
  aria_headers+=(--header="$header")
}

url=""
token=""
direct=""
iperf_host=""
runs=1
try_h3=0
selfcheck=0
positionals=()

while [ "$#" -gt 0 ]; do
  case "$1" in
    -d | --direct)
      [ "$#" -ge 2 ] || die "--direct requires a URL"
      direct=$2
      shift 2
      ;;
    -t | --token)
      [ "$#" -ge 2 ] || die "--token requires a value"
      token=$2
      shift 2
      ;;
    -i | --iperf3)
      [ "$#" -ge 2 ] || die "--iperf3 requires a host"
      iperf_host=$2
      shift 2
      ;;
    -r | --runs)
      [ "$#" -ge 2 ] || die "--runs requires a number"
      runs=$2
      shift 2
      ;;
    -3 | --http3)
      try_h3=1
      shift
      ;;
    -s | --self-check)
      selfcheck=1
      shift
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    --)
      shift
      while [ "$#" -gt 0 ]; do
        positionals+=("$1")
        shift
      done
      ;;
    -*)
      die "unknown option: $1 (try --help)"
      ;;
    *)
      positionals+=("$1")
      shift
      ;;
  esac
done

if [ "${#positionals[@]}" -gt 0 ] && [ -z "$url" ]; then
  url=${positionals[0]}
fi
if [ "${#positionals[@]}" -gt 1 ] && [ -z "$token" ]; then
  token=${positionals[1]}
fi

case "$runs" in
  '' | *[!0-9]*) die "--runs expects a positive integer" ;;
esac
[ "$runs" -ge 1 ] || die "--runs expects a positive integer"

tmp=$(mktemp -d "${TMPDIR:-/tmp}/ludex-bench.XXXXXX") || die "mktemp failed"
trap 'rm -rf "$tmp"' EXIT
rows="$tmp/rows.tsv"
: >"$rows"

curl_headers=(-H "User-Agent: ludex-bench/1")
aria_headers=(--user-agent=ludex-bench/1)
if [ -n "$token" ]; then
  set_headers "$token" || die "--token is empty"
fi

add_row() {
  printf '%s\t%s\t%s\t%s\t%s\n' "$1" "$2" "$3" "$4" "$5" >>"$rows"
}

# One curl row. Keeps the fastest of --runs attempts. Speed is bytes/second.
run_curl() {
  local label=$1 proto=$2 target=$3
  local i=1 best_speed=0 best_bytes=0 best_seconds=0 best_code=000
  local out code bytes seconds speed
  while [ "$i" -le "$runs" ]; do
    out=$(curl -sS -L --connect-timeout 20 --max-time 14400 -o /dev/null \
      -w '%{http_code} %{size_download} %{time_total} %{speed_download}' \
      "${curl_headers[@]}" "$proto" "$target" 2>"$tmp/curl.err") || out="000 0 0 0"
    read -r code bytes seconds speed <<<"$out" || true
    code=${code:-000}
    bytes=${bytes:-0}
    seconds=${seconds:-0}
    speed=${speed:-0}
    case "$code" in
      200 | 206)
        if [ "$best_code" != "200" ] && [ "$best_code" != "206" ]; then
          best_code=$code
          best_bytes=$bytes
          best_seconds=$seconds
          best_speed=$speed
        elif awk -v a="$speed" -v b="$best_speed" 'BEGIN { exit !(a > b) }'; then
          best_code=$code
          best_bytes=$bytes
          best_seconds=$seconds
          best_speed=$speed
        fi
        ;;
      *)
        # Keep the first failure visible if no attempt succeeds; never let a
        # tiny error body win the "fastest" comparison.
        if [ "$best_code" = "000" ]; then
          best_code=$code
          best_bytes=$bytes
          best_seconds=$seconds
        fi
        ;;
    esac
    i=$((i + 1))
  done
  add_row "$label" "$best_code" "$best_bytes" "$best_seconds" "$best_speed"
  printf '  %-20s HTTP %-4s %s\n' "$label" "$best_code" \
    "$(awk -v s="$best_speed" 'BEGIN { printf "%.1f MB/s", s / 1048576 }')" >&2
}

# aria2c uses HTTP/1.1 with -x8 separate connections: this is the parallel-range
# ceiling, not the desktop client's HTTP/2 path.
run_aria() {
  local label=$1 target=$2
  if ! command -v aria2c >/dev/null 2>&1; then
    printf '  %-20s skipped (aria2c not installed)\n' "$label" >&2
    return
  fi
  local slug start end ms size speed
  slug=$(printf '%s' "$label" | tr ' /' '__')
  start=$(now_ms)
  if ! aria2c -x8 -s8 -k 1M --file-allocation=none --summary-interval=0 \
    --console-log-level=warn --allow-overwrite=true --auto-file-renaming=false \
    "${aria_headers[@]}" -d "$tmp" -o "aria-$slug.bin" "$target" \
    >/dev/null 2>"$tmp/aria.err"; then
    printf '  %-20s aria2c failed: %s\n' "$label" \
      "$(tail -n 1 "$tmp/aria.err" 2>/dev/null)" >&2
    return
  fi
  end=$(now_ms)
  ms=$((end - start))
  [ "$ms" -gt 0 ] || ms=1
  size=$(stat -c %s "$tmp/aria-$slug.bin" 2>/dev/null ||
    stat -f %z "$tmp/aria-$slug.bin" 2>/dev/null || echo 0)
  speed=$(awk -v b="$size" -v ms="$ms" 'BEGIN { printf "%.0f", b * 1000 / ms }')
  add_row "$label" "200" "$size" \
    "$(awk -v ms="$ms" 'BEGIN { printf "%.3f", ms / 1000 }')" "$speed"
  rm -f "$tmp/aria-$slug.bin"
  printf '  %-20s %s\n' "$label" \
    "$(awk -v s="$speed" 'BEGIN { printf "%.1f MB/s", s / 1048576 }')" >&2
}

iperf_single=0
iperf_parallel=0
iperf_seconds=15

# Aggregate bitrate of an iperf3 run, in Mbit/s.
#
# `-P8` prints one `sender` line per stream plus a `[SUM]` aggregate. Taking
# the first `sender` line reported one stream and made `-P8/-P1` look <1x
# (#211, 7.2). Prefer the `[SUM]` line, and fall back to summing the sender
# lines of the last block, because a single-stream run prints no `[SUM]`.
parse_iperf_mbps() {
  awk '
    { line[NR] = $0 }
    END {
      block = 0
      for (i = 1; i <= NR; i++) {
        if (line[i] ~ /^- - - - /) {
          block++
          continue
        }
        blocks[block] = blocks[block] "\n" line[i]
      }
      for (b = block; b >= 0; b--) {
        if (blocks[b] !~ /sender/) continue
        rows = split(blocks[b], row, "\n")
        sum = 0
        found = 0
        for (r = 1; r <= rows; r++) {
          value = 0
          unit = ""
          fields = split(row[r], field, /[ \t]+/)
          for (f = 2; f <= fields; f++) {
            if (field[f] ~ /^(K|M|G|T)?bits\/sec$/) {
              value = field[f - 1] + 0
              unit = field[f]
            }
          }
          if (value == 0) continue
          if (unit == "Kbits/sec") value /= 1000
          else if (unit == "Gbits/sec") value *= 1000
          else if (unit == "Tbits/sec") value *= 1000000
          if (row[r] ~ /\[SUM\]/ && row[r] ~ /sender/) {
            printf "%.1f\n", value
            found = -1
            break
          }
          if (row[r] ~ /sender/) {
            sum += value
            found = 1
          }
        }
        if (found == -1) exit 0
        if (found) {
          printf "%.1f\n", sum
          exit 0
        }
      }
      exit 1
    }
  '
}

run_iperf() {
  local streams=$1
  if ! command -v iperf3 >/dev/null 2>&1; then
    printf '  iperf3 skipped (not installed)\n' >&2
    return
  fi
  local out mbps
  out=$(iperf3 -c "$iperf_host" -t "$iperf_seconds" -P "$streams" 2>&1) || {
    printf '  iperf3 -P %s failed (server not reachable?)\n' "$streams" >&2
    return
  }
  mbps=$(printf '%s\n' "$out" | parse_iperf_mbps)
  if [ -z "$mbps" ]; then
    printf '  iperf3 -P %s: could not parse output\n' "$streams" >&2
    return
  fi
  if [ "$streams" = "1" ]; then
    iperf_single=$mbps
  else
    iperf_parallel=$mbps
  fi
  printf '  iperf3 -P %-2s %s Mbit/s (%s MB/s)\n' "$streams" "$mbps" \
    "$(awk -v m="$mbps" 'BEGIN { printf "%.1f", m / 8 }')" >&2
}

ping_avg=""

run_ping() {
  if ! command -v ping >/dev/null 2>&1; then
    return
  fi
  local out avg
  out=$(ping -c 10 -q "$iperf_host" 2>/dev/null) || return
  avg=$(printf '%s\n' "$out" | awk -F'[=/]' '
    /min\/avg\/max/ {
      value = $6
      gsub(/ /, "", value)
      print value
      exit
    }')
  if [ -n "$avg" ]; then
    ping_avg=$avg
    printf '  ping %-13s avg %s ms\n' "$iperf_host" "$avg" >&2
  fi
}

# Render the TSV rows and print the verdicts. Arguments: rows file,
# iperf3 single Mbit/s, iperf3 -P8 Mbit/s, ping average ms.
render_results() {
  local file=$1
  local i1=${2:-0}
  local i8=${3:-0}
  local rtt=${4:-}
  awk -F '\t' -v iperf1="$i1" -v iperf8="$i8" -v rtt="$rtt" '
    BEGIN {
      printf "%-20s %-6s %9s %8s %9s %6s\n", "target", "http", "size(GB)", "time(s)", "MB/s", "xbest"
      printf "%-20s %-6s %9s %8s %9s %6s\n", "--------------------", "------", "---------", "--------", "---------", "------"
    }
    FNR == NR {
      speed = $5 + 0
      if (speed > best) best = speed
      if ($1 ~ /^caddy / && speed > caddy) caddy = speed
      if ($1 ~ /^direct / && speed > direct) direct = speed
      if ($1 ~ /^aria2c / && speed > aria) aria = speed
      if ($1 == "caddy h2") h2 = speed
      if ($1 == "caddy h1.1") h1 = speed
      if ($1 == "caddy h3") h3 = speed
      next
    }
    {
      speed = $5 + 0
      ratio = (best > 0) ? speed / best : 0
      printf "%-20s %-6s %9.2f %8.1f %9.1f %6.2f\n", $1, $2, $3 / 1073741824, $4, speed / 1048576, ratio
    }
    END {
      printf "\n"
      if (rtt != "") printf "RTT (avg): %s ms\n", rtt
      printf "Verdicts (thresholds: docs/NETWORK-ARCHITECTURE.md)\n"

      if (h2 > 0 && h1 > 0) {
        r = h1 / h2
        if (r >= 1.25)
          printf "- h1.1/h2 = %.2fx: HTTP/2 flow control (or a proxy window) is limiting. Fix the single flow first: larger client h2 window, or h1.1.\n", r
        else
          printf "- h1.1/h2 = %.2fx: HTTP/2 is not the limit.\n", r
      } else {
        printf "- h1.1 vs h2: insufficient data (need both rows).\n"
      }

      if (h3 > 0 && h2 > 0) {
        r = h3 / h2
        if (r >= 1.15)
          printf "- h3/h2 = %.2fx: HTTP/3 helps on this path; keep it enabled (UDP 443 must be reachable).\n", r
        else if (r < 0.90)
          printf "- h3/h2 = %.2fx: HTTP/3 is slower here; keep h2 as the default and treat h3 as opportunistic.\n", r
        else
          printf "- h3/h2 = %.2fx: no material difference; keep h2 and let Caddy advertise h3 opportunistically.\n", r
      } else {
        printf "- h3 vs h2: not measured (pass --http3 with a curl that supports it).\n"
      }

      if (direct > 0 && caddy > 0) {
        r = direct / caddy
        if (r > 1.10)
          printf "- direct/caddy = %.2fx: Caddy adds %.0f%%. Check buffering, TLS CPU and the h2 window before bypassing it.\n", r, (r - 1) * 100
        else if (r >= 0.90)
          printf "- direct/caddy = %.2fx: direct is within 10%% of Caddy, so Caddy is not the limit. Keep Caddy.\n", r
        else if (r >= 0.50)
          printf "- direct/caddy = %.2fx: direct is %.0f%% slower than Caddy. Caddy is not the limit: investigate the direct path (disk, CPU, network namespace, bind address).\n", r, (1 - r) * 100
        else
          printf "- direct/caddy = %.2fx: direct is %.0f%% slower than Caddy. The direct path is broken or throttled; fix it before trusting its rows.\n", r, (1 - r) * 100
      } else {
        printf "- direct vs Caddy: insufficient data (pass --direct to compare).\n"
      }

      if (aria > 0 && caddy > 0) {
        r = aria / caddy
        if (r >= 1.50)
          printf "- aria2c -x8 / single = %.2fx: parallel ranges are justified (>1.5x); implement them in a separate issue with per-range .part files and pwrite.\n", r
        else if (r >= 1.20)
          printf "- aria2c -x8 / single = %.2fx: marginal (1.2-1.5x); fix the single flow (h2 window/BBR) before adding parallel ranges.\n", r
        else
          printf "- aria2c -x8 / single = %.2fx: parallel ranges are NOT justified (<1.2x); the link, disk, CPU or proxy is the limit.\n", r
      } else {
        printf "- aria2c -x8 vs single: insufficient data.\n"
      }

      if (iperf1 > 0 && best > 0) {
        used = (best / 1048576) * 8 / iperf1
        if (used >= 0.90)
          printf "- download/iperf3 -P1 = %.0f%%: the link is the limit; no code change will beat it.\n", used * 100
        else
          printf "- download/iperf3 -P1 = %.0f%%: the link is not the limit (server, disk, CPU or proxy is); parallel ranges will not help unless aria2c proves otherwise.\n", used * 100
      } else {
        printf "- link baseline: run with --iperf3 HOST to compare against the link.\n"
      }

      if (iperf1 > 0 && iperf8 > 0) {
        r = iperf8 / iperf1
        if (r >= 2.0)
          printf "- iperf3 -P8/-P1 = %.2fx: a single flow is BDP-limited; BBR or parallel ranges may help.\n", r
        else
          printf "- iperf3 -P8/-P1 = %.2fx: the link is not single-flow limited.\n", r
      }
    }
  ' "$file" "$file"
}

checks=0
failures=0

assert_contains() {
  local haystack=$1 needle=$2 label=$3
  checks=$((checks + 1))
  if printf '%s' "$haystack" | grep -Fq -- "$needle"; then
    printf 'PASS %s\n' "$label"
  else
    printf 'FAIL %s (missing: %s)\n' "$label" "$needle"
    failures=$((failures + 1))
  fi
}

self_check() {
  local a b

  checks=$((checks + 1))
  a=$(now_ms)
  b=$(now_ms)
  case "$a" in
    '' | *[!0-9]*)
      printf 'FAIL now_ms is not an integer: %s\n' "$a"
      failures=$((failures + 1))
      ;;
    *)
      printf 'PASS now_ms is an integer\n'
      ;;
  esac
  checks=$((checks + 1))
  if [ "$b" -ge "$a" ]; then
    printf 'PASS now_ms does not go backwards\n'
  else
    printf 'FAIL now_ms went backwards (%s then %s)\n' "$a" "$b"
    failures=$((failures + 1))
  fi

  # Fixture 1: everything healthy. Direct within 10% of Caddy, h1.1/h2 ~1,
  # aria2c ~1.03x, h3 1.2x.
  local f1="$tmp/self-1.tsv"
  {
    printf 'caddy h2\t200\t419430400\t4.0\t104857600\n'
    printf 'caddy h1.1\t200\t419430400\t3.96\t105906176\n'
    printf 'caddy h3\t200\t419430400\t3.33\t125829120\n'
    printf 'direct h1.1\t200\t419430400\t4.04\t103809024\n'
    printf 'aria2c -x8 caddy\t200\t419430400\t3.85\t109051904\n'
  } >"$f1"
  local out1
  out1=$(render_results "$f1" 0 0 "")
  assert_contains "$out1" "Caddy is not the limit" "fixture 1: Caddy is not the limit"
  assert_contains "$out1" "HTTP/2 is not the limit" "fixture 1: h2 is not the limit"
  assert_contains "$out1" "parallel ranges are NOT justified" "fixture 1: no parallel ranges"
  assert_contains "$out1" "HTTP/3 helps" "fixture 1: h3 helps"
  assert_contains "$out1" "run with --iperf3" "fixture 1: missing iperf3 is reported"

  # Fixture 2: the 36 MB/s case. h2 is half of h1.1, aria2c doubles it, and the
  # link (2 Gbit/s single) is far from saturated.
  local f2="$tmp/self-2.tsv"
  {
    printf 'caddy h2\t200\t419430400\t11.1\t37748736\n'
    printf 'caddy h1.1\t200\t419430400\t4.44\t94371840\n'
    printf 'direct h1.1\t200\t419430400\t4.40\t95420416\n'
    printf 'aria2c -x8 caddy\t200\t419430400\t2.22\t188743680\n'
  } >"$f2"
  local out2
  out2=$(render_results "$f2" 2000 4000 "42.0")
  assert_contains "$out2" "HTTP/2 flow control" "fixture 2: h2 flow control"
  assert_contains "$out2" "parallel ranges are justified" "fixture 2: parallel ranges justified"
  assert_contains "$out2" "Caddy is not the limit" "fixture 2: direct within 10%"
  assert_contains "$out2" "the link is not the limit" "fixture 2: link not saturated"
  assert_contains "$out2" "BDP-limited" "fixture 2: single-flow BDP"
  assert_contains "$out2" "RTT (avg): 42.0 ms" "fixture 2: RTT is printed"

  # Fixture 3: the link is saturated and h3 hurts.
  local f3="$tmp/self-3.tsv"
  {
    printf 'caddy h2\t200\t419430400\t3.5\t119537664\n'
    printf 'caddy h1.1\t200\t419430400\t3.5\t119537664\n'
    printf 'caddy h3\t200\t419430400\t5.0\t83886080\n'
    printf 'aria2c -x8 caddy\t200\t419430400\t3.6\t116508444\n'
  } >"$f3"
  local out3
  out3=$(render_results "$f3" 960 1000 "")
  assert_contains "$out3" "the link is the limit" "fixture 3: link is the limit"
  assert_contains "$out3" "HTTP/3 is slower" "fixture 3: h3 hurts"
  assert_contains "$out3" "parallel ranges are NOT justified" "fixture 3: no parallel ranges"

  # 7.1: the bearer header must contain the token value, on both clients. A
  # hardcoded placeholder would still pass "contains Bearer", hence the exact
  # value check.
  checks=$((checks + 1))
  set_headers "tok-abc.123"
  if printf '%s\n' "${curl_headers[@]}" | grep -Fxq -- "Authorization: Bearer tok-abc.123"; then
    printf 'PASS curl sends the token from --token\n'
  else
    printf 'FAIL curl header is: %s\n' "${curl_headers[*]}"
    failures=$((failures + 1))
  fi
  checks=$((checks + 1))
  if printf '%s\n' "${aria_headers[@]}" | grep -Fxq -- "--header=Authorization: Bearer tok-abc.123"; then
    printf 'PASS aria2c sends the token from --token\n'
  else
    printf 'FAIL aria2c header is: %s\n' "${aria_headers[*]}"
    failures=$((failures + 1))
  fi
  checks=$((checks + 1))
  set_headers ""
  if [ "${#curl_headers[@]}" -eq 2 ] && [ "${#aria_headers[@]}" -eq 1 ]; then
    printf 'PASS no token means no Authorization header\n'
  else
    printf 'FAIL headers without a token: %s / %s\n' \
      "${curl_headers[*]}" "${aria_headers[*]}"
    failures=$((failures + 1))
  fi

  # 7.2: the -P8 total is the `[SUM]` line, not the first stream. Fixture is a
  # trimmed real run: 8 x 20.6 Gbit/s streams aggregate to 70.7 Gbit/s.
  local iperf8 iperf1
  iperf8=$(parse_iperf_mbps <<'EOF'
Connecting to host 10.1.20.58, port 5201
[ ID] Interval           Transfer     Bitrate         Retr  Cwnd
[  5]   0.00-1.00   sec  2.40 GBytes  20.6 Gbits/sec    0   3.12 MBytes
[  5]   1.00-2.00   sec  2.40 GBytes  20.6 Gbits/sec    0   3.12 MBytes
- - - - - - - - - - - - - - - - - - - - - - - - -
[ ID] Interval           Transfer     Bitrate         Retr
[  5]   0.00-15.00  sec  36.0 GBytes  20.6 Gbits/sec    0             sender
[  6]   0.00-15.00  sec  36.0 GBytes  20.6 Gbits/sec    0             sender
[  7]   0.00-15.00  sec  36.0 GBytes  20.6 Gbits/sec    0             sender
[  8]   0.00-15.00  sec  36.0 GBytes  20.6 Gbits/sec    0             sender
[  9]   0.00-15.00  sec  36.0 GBytes  20.6 Gbits/sec    0             sender
[ 10]   0.00-15.00  sec  36.0 GBytes  20.6 Gbits/sec    0             sender
[ 11]   0.00-15.00  sec  36.0 GBytes  20.6 Gbits/sec    0             sender
[ 12]   0.00-15.00  sec  36.0 GBytes  20.6 Gbits/sec    0             sender
[SUM]   0.00-15.00  sec   288 GBytes  70.7 Gbits/sec    0             sender
[  5]   0.00-15.00  sec  36.0 GBytes  20.6 Gbits/sec                  receiver
[SUM]   0.00-15.00  sec   288 GBytes  70.7 Gbits/sec                  receiver

iperf Done.
EOF
)
  assert_contains "$iperf8" "70700.0" "fixture 7.2: -P8 uses the [SUM] line"

  # A single-stream run prints no `[SUM]`; the sender line must still be read.
  iperf1=$(parse_iperf_mbps <<'EOF'
[ ID] Interval           Transfer     Bitrate         Retr
[  5]   0.00-15.00  sec  35.0 GBytes  19.9 Gbits/sec    0             sender
[  5]   0.00-15.00  sec  35.0 GBytes  19.9 Gbits/sec                  receiver

iperf Done.
EOF
)
  assert_contains "$iperf1" "19900.0" "fixture 7.2: -P1 has no [SUM]"

  # Fixture 4: the field report's direct/caddy = 0.55x. 0.55x is not "within
  # 10%": the direct path is 45% slower, and Caddy is still not the limit.
  local f4="$tmp/self-4.tsv"
  {
    printf 'caddy h2\t200\t47935843067\t480.7\t100000000\n'
    printf 'caddy h1.1\t200\t47935843067\t573.5\t85000000\n'
    printf 'direct h1.1\t200\t47935843067\t734.5\t55000000\n'
  } >"$f4"
  local out4
  out4=$(render_results "$f4" 0 0 "")
  assert_contains "$out4" "45% slower than Caddy" "fixture 4: 0.55x is not 'within 10%'"

  printf '\n'
  if [ "$failures" -eq 0 ]; then
    printf 'self-check: %s checks passed\n' "$checks"
    return 0
  fi
  printf 'self-check: %s of %s checks failed\n' "$failures" "$checks"
  return 1
}

if [ "$selfcheck" -eq 1 ]; then
  self_check
  exit $?
fi

[ -n "$url" ] || die "missing download URL (try --help)"

case "$url" in
  https://*) ;;
  *)
    printf 'note: %s is not https; the h2 row will negotiate h1.1\n' "$(redact_url "$url")" >&2
    ;;
esac

printf 'Ludex network benchmark (#200)\n'
printf 'date:   %s\n' "$(date -u +%FT%TZ)"
printf 'client: %s\n' "$(uname -srm)"
printf 'curl:   %s\n' "$(curl --version 2>/dev/null | head -n 1)"
printf 'url:    %s\n' "$(redact_url "$url")"
if [ -n "$direct" ]; then
  printf 'direct: %s\n' "$(redact_url "$direct")"
else
  printf 'direct: none\n'
fi
printf 'token:  %s\n' "$([ -n "$token" ] && echo 'set (not printed)' || echo 'not set')"
printf 'runs:   %s per row (best kept)\n' "$runs"
if command -v sysctl >/dev/null 2>&1; then
  cc=$(sysctl -n net.ipv4.tcp_congestion_control 2>/dev/null || true)
  if [ -n "$cc" ]; then
    printf 'cc:     %s (client; record the server value too)\n' "$cc"
  fi
fi
printf '\n'

if [ -n "$iperf_host" ]; then
  printf 'Link baseline\n' >&2
  run_ping
  run_iperf 1
  run_iperf 8
  printf '\n' >&2
fi

printf 'Downloads\n' >&2
run_curl "caddy h2" "--http2" "$url"
run_curl "caddy h1.1" "--http1.1" "$url"

if [ "$try_h3" -eq 1 ]; then
  if curl_has_h3; then
    run_curl "caddy h3" "--http3-only" "$url"
  else
    printf '  %-20s skipped (curl has no HTTP/3 support)\n' "caddy h3" >&2
  fi
fi

if [ -n "$direct" ]; then
  case "$direct" in
    https://*)
      run_curl "direct h2" "--http2" "$direct"
      run_curl "direct h1.1" "--http1.1" "$direct"
      ;;
    *)
      # The Ludex server itself is plain HTTP: a cleartext h2 row would just
      # measure h2c upgrade behaviour, so only the h1.1 baseline is useful.
      run_curl "direct h1.1" "--http1.1" "$direct"
      ;;
  esac
fi

printf '\n' >&2
run_aria "aria2c -x8 caddy" "$url"
if [ -n "$direct" ]; then
  run_aria "aria2c -x8 direct" "$direct"
fi

printf '\n'
render_results "$rows" "$iperf_single" "$iperf_parallel" "$ping_avg"

cat <<'EOF'

Record in the issue: the table above, RTT, iperf3 single/8, server and Caddy
%CPU (pidstat/top), disk MB/s (dd iflag=direct), the server's
net.ipv4.tcp_congestion_control (BBR or cubic), Caddy version, whether UDP 443
works, LAN or WAN, wired or Wi-Fi, and the cold/warm difference. The full
matrix and the thresholds are in docs/NETWORK-ARCHITECTURE.md.

The native Tauri installer is HTTP/1.1 (reqwest without the http2 feature):
use the h1.1 row as its baseline; the h2 row is the browser/curl path.
EOF
