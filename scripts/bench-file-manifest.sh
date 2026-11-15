#!/usr/bin/env bash
# Benchmark: classic archive download vs server-side extracted cache + file
# manifest (#231).
#
#   scripts/bench-file-manifest.sh --url URL --token TOKEN --game ID --version ID [options]
#
# The script measures the two transports end to end on one version:
#
#   classic   download the archive, verify the archive SHA-256, extract it;
#   manifest  ask the server to extract once, download the files in parallel,
#             verify every file SHA-256 (no archive, no client extraction).
#
# It also runs the two recovery cases the prototype gate requires:
#
#   resume    kill the client mid-download, then finish; only missing or
#             mismatched files are transferred;
#   corrupt   flip one byte of a downloaded file and prove the next pass
#             detects it, re-downloads it and commits the right bytes.
#
# Per phase it prints wall time, client CPU time, server CPU time and disk
# reads/writes when a server shell is configured, the client peak disk use and
# the bytes moved. The last line is the verdict against the 1.5x gate.
#
# Options:
#   -u, --url URL          base URL of the Ludex server (https://ludex.example)
#   -t, --token TOKEN      administrator bearer token. It authenticates the
#                          admin extract route and the public file routes.
#   -g, --game ID          game id (needed for the classic archive download)
#   -v, --version ID       version id to benchmark
#   -j, --jobs N           parallel file downloads for the manifest rows (default 8)
#   -w, --work DIR         scratch directory (default: mktemp -d)
#   -s, --server-ssh HOST  optional SSH target for server CPU/disk sampling,
#                          e.g. pve3. Without it the server columns print n/a.
#       --extract-cmd CMD  classic extraction command (default: 7z x -y for
#                          .7z/.rar, unzip -q otherwise)
#       --no-resume        skip the resume phase
#       --no-corrupt       skip the corrupted-file phase
#       --classic-only     run only the classic phase
#       --manifest-only    run only the manifest phase (server cache must be ready)
#       --self-check       offline check of the arithmetic/verdict, no network
#   -h, --help
#
# Requirements: curl, sha256sum, python3 (JSON parsing), one extractor for the
# classic phase. Record the printed table and the procedure in
# docs/PERFORMANCE.md with the issue link; the prototype is only worth keeping
# at 1.5-2x end to end.
set -uo pipefail

JOBS=8
URL=""
TOKEN=""
GAME=""
VERSION=""
WORK=""
SERVER_SSH=""
EXTRACT_CMD=""
RUN_CLASSIC=1
RUN_MANIFEST=1
RUN_RESUME=1
RUN_CORRUPT=1
SELF_CHECK=0
POLL_SECONDS=5
POLL_LIMIT=720 # 1 h at 5 s

usage() {
  sed -n '2,50p' "$0" | sed 's/^# \{0,1\}//'
}

die() {
  printf 'error: %s\n' "$*" >&2
  exit 2
}

note() {
  printf '%s\n' "$*" >&2
}

now_ms() {
  if [[ -n "${EPOCHREALTIME:-}" ]]; then
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
  die "cannot read a millisecond clock"
}

# Seconds with three decimals between two millisecond instants.
seconds_between() {
  local start=$1 end=$2
  local delta=$((end - start))
  printf '%d.%03d' $((delta / 1000)) $((delta % 1000))
}

# Ratio with two decimals; prints n/a for a zero denominator.
ratio() {
  local numerator=$1 denominator=$2
  python3 - "$numerator" "$denominator" <<'PY'
import sys
try:
    num = float(sys.argv[1])
    den = float(sys.argv[2])
except ValueError:
    print("n/a")
    raise SystemExit(0)
print(f"{num / den:.2f}" if den else "n/a")
PY
}

mib() {
  python3 - "$1" <<'PY'
import sys
value = float(sys.argv[1]) / (1024 * 1024)
print(f"{value:.1f}")
PY
}

# Whole-host CPU seconds and disk MiB from a Linux procfs snapshot.
proc_cpu_seconds() {
  awk '/^cpu / { total = 0; for (i = 2; i <= NF; i++) total += $i; print total / 100 }' "$1"
}

proc_disk_mib() {
  awk '$3 ~ /^(sd|vd|nvme|dm-|md)/ { read += $6; write += $10 } END { print (read * 512) / 1048576, (write * 512) / 1048576 }' "$1"
}

snapshot_local() {
  local prefix=$1
  {
    head -n 1 /proc/stat > "$prefix.cpu"
    cat /proc/diskstats > "$prefix.disk"
  }
}

snapshot_server() {
  local prefix=$1
  if [[ -z "$SERVER_SSH" ]]; then
    return 0
  fi
  ssh -o BatchMode=yes -o ConnectTimeout=10 "$SERVER_SSH" \
    'echo __OUT__; head -n 1 /proc/stat; cat /proc/diskstats' \
    2>/dev/null | sed -n '/__OUT__/,$p' | tail -n +2 > "$prefix.server" || true
}

server_cpu_seconds() {
  local file=$1
  [[ -s "$file" ]] || { printf 'n/a\n'; return; }
  awk '/^cpu / { total = 0; for (i = 2; i <= NF; i++) total += $i; print total / 100 }' "$file"
}

server_disk_mib() {
  local file=$1
  [[ -s "$file" ]] || { printf 'n/a n/a\n'; return; }
  awk '$1 ~ /^(sd|vd|nvme|dm-|md)/ || $3 ~ /^(sd|vd|nvme|dm-|md)/ {
    for (i = 1; i <= NF; i++) {
      if ($i ~ /^(sd|vd|nvme|dm-|md)/) { read += $(i + 3); write += $(i + 7); break }
    }
  } END { print (read * 512) / 1048576, (write * 512) / 1048576 }' "$file"
}

# Peak bytes under a directory while a background sampler runs.
start_peak_sampler() {
  local directory=$1 output=$2
  : > "$output"
  (
    while :; do
      du -sb "$directory" 2>/dev/null | awk '{print $1}' >> "$output"
      sleep 0.2
    done
  ) &
  echo $!
}

stop_peak_sampler() {
  local pid=$1 output=$2
  kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
  sort -n "$output" 2>/dev/null | tail -n 1
}

json_field() {
  python3 - "$1" "$2" <<'PY'
import json, sys
try:
    with open(sys.argv[1], encoding="utf-8") as handle:
        data = json.load(handle)
except (OSError, ValueError):
    print("")
    raise SystemExit(0)
value = data
for part in sys.argv[2].split("."):
    if isinstance(value, dict):
        value = value.get(part, "")
    else:
        value = ""
        break
print(value if value is not None else "")
PY
}

fetch_json() {
  local url=$1 output=$2
  curl --fail --silent --show-error --location --max-time 60 \
    -H "Authorization: Bearer $TOKEN" -o "$output" "$url"
}

wait_ready() {
  local body="$WORK/extract.json"
  local attempt=0
  while (( attempt < POLL_LIMIT )); do
    if ! fetch_json "$URL/api/admin/versions/$VERSION/extract" "$body"; then
      note "extract request failed; retrying"
    else
      local status
      status=$(json_field "$body" status)
      case "$status" in
        ready) return 0 ;;
        failed) die "server extraction failed: $(json_field "$body" error)" ;;
        unavailable) die "the extracted cache is disabled on this server (LUDEX_EXTRACTED_CACHE=0)" ;;
        queued|extracting|"")
          if (( attempt % 6 == 0 )); then
            note "extract $status: $(json_field "$body" files) files, $(json_field "$body" bytes) bytes"
          fi
          ;;
        *) note "unexpected extract status: $status" ;;
      esac
    fi
    attempt=$((attempt + 1))
    sleep "$POLL_SECONDS"
  done
  die "extraction never became ready"
}

write_files_tsv() {
  local manifest=$1 output=$2
  python3 - "$manifest" "$output" <<'PY'
import json, sys, urllib.parse
with open(sys.argv[1], encoding="utf-8") as handle:
    manifest = json.load(handle)
with open(sys.argv[2], "w", encoding="utf-8") as handle:
    for entry in manifest.get("files", []):
        path = entry["path"]
        encoded = urllib.parse.quote(path, safe="/")
        handle.write("\t".join([path, str(entry["size"]), entry["sha256"], encoded]) + "\n")
PY
}

# Downloads one file; reuses a completed file only when its digest matches.
download_one() {
  local destination=$1 encoded=$2 expected=$3
  local target="$DEST/$destination"
  if [[ -f "$target" ]] && [[ "$(sha256sum "$target" | awk '{print $1}')" == "$expected" ]]; then
    printf 'keep\t%s\n' "$destination" >> "$WORK/download.log"
    return 0
  fi
  mkdir -p "$(dirname "$target")"
  if curl --fail --silent --show-error --location --max-time 3600 \
      -H "Authorization: Bearer $TOKEN" \
      -o "$target" "$URL/api/versions/$VERSION/files/$encoded"; then
    local digest
    digest=$(sha256sum "$target" | awk '{print $1}')
    if [[ "$digest" == "$expected" ]]; then
      printf 'ok\t%s\n' "$destination" >> "$WORK/download.log"
      return 0
    fi
    printf 'corrupt\t%s\n' "$destination" >> "$WORK/download.log"
    return 1
  fi
  printf 'error\t%s\n' "$destination" >> "$WORK/download.log"
  return 1
}

export -f download_one
export DEST URL VERSION TOKEN WORK

# Parallel download of every file in the manifest; fails on any bad digest.
download_all() {
  local files=$1 jobs=$2
  : > "$WORK/download.log"
  cut -f1,3,4 "$files" | tr '\t' '\n' > "$WORK/download.args"
  local failures=0
  # Three arguments per record (path, digest, encoded path) through NUL-safe
  # xargs so spaces, quotes and unicode survive.
  python3 - "$files" <<'PY' | xargs -0 -n 3 -P "$jobs" bash -c 'download_one "$@" || exit 255' _ || failures=1
import sys
with open(sys.argv[1], encoding="utf-8") as handle:
    for line in handle:
        path, _size, digest, encoded = line.rstrip("\n").split("\t")
        sys.stdout.write(path + "\0" + encoded + "\0" + digest + "\0")
PY
  [[ "$failures" == "0" ]] || return 1
  local kept errors corrupt
  kept=$(awk '$1 == "keep"' "$WORK/download.log" | wc -l)
  errors=$(awk '$1 == "error"' "$WORK/download.log" | wc -l)
  corrupt=$(awk '$1 == "corrupt"' "$WORK/download.log" | wc -l)
  note "files: $(wc -l < "$files") total, $kept kept, $corrupt corrupt, $errors failed"
  (( errors == 0 && corrupt == 0 ))
}

run_classic() {
  local archive="$WORK/classic/archive.bin"
  local extracted="$WORK/classic/extracted"
  mkdir -p "$WORK/classic" "$extracted"
  local sampler peak
  sampler=$(start_peak_sampler "$WORK/classic" "$WORK/classic.peak")
  snapshot_local "$WORK/client.before"
  snapshot_server "$WORK/server.before"
  local start end download_ms digest
  start=$(now_ms)
  curl --fail --silent --show-error --location --max-time 86400 \
    -H "Authorization: Bearer $TOKEN" \
    -o "$archive" "$URL/api/games/$GAME/download?versionId=$VERSION" \
    || { stop_peak_sampler "$sampler" "$WORK/classic.peak" >/dev/null; die "classic download failed"; }
  download_ms=$(( $(now_ms) - start ))
  digest=$(sha256sum "$archive" | awk '{print $1}')
  local command=${EXTRACT_CMD:-}
  if [[ -z "$command" ]]; then
    case "$archive" in
      *.7z|*.rar) command="7z x -y -o$extracted $archive" ;;
      *) command="unzip -q -o $archive -d $extracted" ;;
    esac
  fi
  note "classic: archive sha256 $digest"
  note "classic: extracting with: $command"
  bash -c "$command" >/dev/null 2>&1 || note "classic: extraction command failed (record it, wall time still counts)"
  end=$(now_ms)
  snapshot_local "$WORK/client.after"
  snapshot_server "$WORK/server.after"
  peak=$(stop_peak_sampler "$sampler" "$WORK/classic.peak")
  CLASSIC_WALL=$(seconds_between "$start" "$end")
  CLASSIC_DOWNLOAD=$(seconds_between "$start" "$((start + download_ms))")
  CLASSIC_BYTES=$(du -sb "$WORK/classic" 2>/dev/null | awk '{print $1}')
  CLASSIC_PEAK=${peak:-0}
}

run_manifest() {
  local dest="$WORK/manifest/files"
  rm -rf "$dest"
  mkdir -p "$dest"
  note "manifest: asking the server to extract version $VERSION"
  local started
  started=$(now_ms)
  wait_ready
  SERVER_EXTRACT_WALL=$(seconds_between "$started" "$(now_ms)")
  fetch_json "$URL/api/versions/$VERSION/manifest" "$WORK/manifest.json" \
    || die "manifest request failed"
  write_files_tsv "$WORK/manifest.json" "$WORK/manifest.tsv"
  local manifest_files
  manifest_files=$(wc -l < "$WORK/manifest.tsv")
  note "manifest: $manifest_files files, $(json_field "$WORK/manifest.json" totalBytes) bytes"

  local sampler peak start end
  sampler=$(start_peak_sampler "$dest" "$WORK/manifest.peak")
  snapshot_local "$WORK/client.before"
  snapshot_server "$WORK/server.before"
  start=$(now_ms)
  DEST="$dest"
  export DEST
  download_all "$WORK/manifest.tsv" "$JOBS" || note "manifest: some files did not verify"
  end=$(now_ms)
  snapshot_local "$WORK/client.after"
  snapshot_server "$WORK/server.after"
  peak=$(stop_peak_sampler "$sampler" "$WORK/manifest.peak")
  MANIFEST_WALL=$(seconds_between "$start" "$end")
  MANIFEST_DOWNLOAD=$MANIFEST_WALL
  MANIFEST_BYTES=$(du -sb "$dest" 2>/dev/null | awk '{print $1}')
  MANIFEST_PEAK=${peak:-0}
}

run_resume() {
  local dest="$WORK/manifest/files"
  [[ -d "$dest" ]] || { note "resume: skipped, no manifest download"; return 0; }
  # Drop the two largest files: a partial transfer plus a wrong digest.
  python3 - "$WORK/manifest.tsv" "$dest" <<'PY'
import os, sys
path, dest = sys.argv[1], sys.argv[2]
rows = []
with open(path, encoding="utf-8") as handle:
    for line in handle:
        rows.append(line.rstrip("\n").split("\t"))
rows.sort(key=lambda row: int(row[1]), reverse=True)
for row in rows[:2]:
    target = os.path.join(dest, row[0])
    if os.path.exists(target):
        size = os.path.getsize(target)
        with open(target, "r+b") as handle:
            handle.truncate(max(1, size // 2))
PY
  note "resume: truncated the two largest files, finishing the install"
  local sampler peak start end
  sampler=$(start_peak_sampler "$dest" "$WORK/resume.peak")
  snapshot_local "$WORK/client.before"
  snapshot_server "$WORK/server.before"
  start=$(now_ms)
  DEST="$dest"
  export DEST
  download_all "$WORK/manifest.tsv" "$JOBS" || note "resume: some files did not verify"
  end=$(now_ms)
  snapshot_local "$WORK/client.after"
  snapshot_server "$WORK/server.after"
  peak=$(stop_peak_sampler "$sampler" "$WORK/resume.peak")
  RESUME_WALL=$(seconds_between "$start" "$end")
  RESUME_PEAK=${peak:-0}
}

run_corrupt() {
  local dest="$WORK/manifest/files"
  local files=$WORK/manifest.tsv
  [[ -d "$dest" ]] || { note "corrupt: skipped, no manifest download"; return 0; }
  local victim
  victim=$(python3 - "$files" <<'PY'
import sys
rows = [line.rstrip("\n").split("\t") for line in open(sys.argv[1], encoding="utf-8")]
rows = [row for row in rows if int(row[1]) > 1]
rows.sort(key=lambda row: int(row[1]), reverse=True)
print(rows[0][0] if rows else "")
PY
)
  [[ -n "$victim" ]] || { note "corrupt: skipped, no file larger than one byte"; return 0; }
  python3 - "$dest/$victim" <<'PY'
import sys
path = sys.argv[1]
with open(path, "r+b") as handle:
    first = handle.read(1)
    handle.seek(0)
    handle.write(bytes([first[0] ^ 0xFF]))
PY
  note "corrupt: flipped one byte in $victim"
  local sampler peak start end
  sampler=$(start_peak_sampler "$dest" "$WORK/corrupt.peak")
  start=$(now_ms)
  DEST="$dest"
  export DEST
  local detected=0
  if ! download_all "$files" "$JOBS"; then
    detected=1
  fi
  # A second pass over the same file proves the recovery converges.
  if [[ "$detected" == "1" ]]; then
    download_all "$files" "$JOBS" || note "corrupt: recovery did not converge"
  fi
  end=$(now_ms)
  peak=$(stop_peak_sampler "$sampler" "$WORK/corrupt.peak")
  CORRUPT_WALL=$(seconds_between "$start" "$end")
  CORRUPT_PEAK=${peak:-0}
  if [[ "$detected" == "1" ]]; then
    CORRUPT_RESULT="detected"
  else
    CORRUPT_RESULT="NOT detected"
  fi
}

report_row() {
  local phase=$1 wall=$2 peak=$3 bytes=$4
  local client_cpu server_cpu server_read server_write
  client_cpu=$(python3 - "$(proc_cpu_seconds "$WORK/client.before.cpu")" "$(proc_cpu_seconds "$WORK/client.after.cpu")" <<'PY'
import sys
print(f"{float(sys.argv[2]) - float(sys.argv[1]):.1f}")
PY
)
  if [[ -s "$WORK/server.before.server" && -s "$WORK/server.after.server" ]]; then
    server_cpu=$(python3 - "$(server_cpu_seconds "$WORK/server.before.server")" "$(server_cpu_seconds "$WORK/server.after.server")" <<'PY'
import sys
print(f"{float(sys.argv[2]) - float(sys.argv[1]):.1f}")
PY
)
    read server_read server_write <<< "$(python3 - "$(server_disk_mib "$WORK/server.before.server")" "$(server_disk_mib "$WORK/server.after.server")" <<'PY'
import sys
before = sys.argv[1].split()
after = sys.argv[2].split()
print(f"{float(after[0]) - float(before[0]):.1f}", f"{float(after[1]) - float(before[1]):.1f}")
PY
)"
  else
    server_cpu="n/a"
    server_read="n/a"
    server_write="n/a"
  fi
  printf '%-24s %8s %10s %11s %12s %12s %11s %10s\n' \
    "$phase" "$wall" "$client_cpu" "$server_cpu" "$server_read" "$server_write" \
    "$(mib "$peak")" "$(mib "$bytes")"
}

print_table() {
  printf '\n%-24s %8s %10s %11s %12s %12s %11s %10s\n' \
    "phase" "wall_s" "client_cpu" "server_cpu" "server_rd" "server_wr" "peak_disk" "moved"
  printf '%s\n' "------------------------------------------------------------------------------------------------------------"
  if [[ -n "${CLASSIC_WALL:-}" ]]; then
    report_row "classic" "$CLASSIC_WALL" "$CLASSIC_PEAK" "$CLASSIC_BYTES"
  fi
  if [[ -n "${MANIFEST_WALL:-}" ]]; then
    report_row "manifest (${JOBS} jobs)" "$MANIFEST_WALL" "$MANIFEST_PEAK" "$MANIFEST_BYTES"
  fi
  if [[ -n "${RESUME_WALL:-}" ]]; then
    report_row "resume (after kill)" "$RESUME_WALL" "$RESUME_PEAK" "$MANIFEST_BYTES"
  fi
  if [[ -n "${CORRUPT_WALL:-}" ]]; then
    report_row "corrupt file" "$CORRUPT_WALL" "$CORRUPT_PEAK" "$MANIFEST_BYTES"
  fi
  printf '%s\n' "------------------------------------------------------------------------------------------------------------"
  printf 'columns: wall/cpu in seconds, bytes in MiB; server columns are whole-host\n'
  if [[ -n "${CLASSIC_DOWNLOAD:-}" ]]; then
    printf 'classic archive download alone: %ss (rest is verification + extraction)\n' "$CLASSIC_DOWNLOAD"
  fi
  if [[ -n "${SERVER_EXTRACT_WALL:-}" ]]; then
    printf 'server extraction (one-off): %ss; manifest bytes: %s\n' \
      "$SERVER_EXTRACT_WALL" "$(mib "$(json_field "$WORK/manifest.json" totalBytes)")"
  fi
  if [[ -n "${CORRUPT_RESULT:-}" ]]; then
    printf 'corrupted file: %s / re-downloaded\n' "$CORRUPT_RESULT"
  fi
}

print_verdict() {
  if [[ -z "${CLASSIC_WALL:-}" || -z "${MANIFEST_WALL:-}" ]]; then
    printf 'verdict: n/a (run both phases)\n'
    return 0
  fi
  local speedup
  speedup=$(ratio "$CLASSIC_WALL" "$MANIFEST_WALL")
  printf 'verdict: classic/manifest speedup %sx (gate 1.5x, strong 2.0x)\n' "$speedup"
  python3 - "$speedup" <<'PY'
import sys
try:
    value = float(sys.argv[1])
except ValueError:
    print("verdict: cannot compare, keep the classic transport")
    raise SystemExit(0)
if value >= 2.0:
    print("verdict: PASS (>= 2.0x) - file manifest is a clear win end to end")
elif value >= 1.5:
    print("verdict: PASS (>= 1.5x) - clears the prototype gate")
else:
    print("verdict: FAIL (< 1.5x) - keep the classic transport, publish the numbers")
PY
}

self_check() {
  # Exercise the arithmetic and the verdict without any network access.
  CLASSIC_WALL=100.000
  CLASSIC_PEAK=1048576
  CLASSIC_BYTES=1048576
  MANIFEST_WALL=50.000
  MANIFEST_PEAK=524288
  MANIFEST_BYTES=524288
  RESUME_WALL=5.000
  RESUME_PEAK=524288
  CORRUPT_WALL=2.000
  CORRUPT_PEAK=524288
  CORRUPT_RESULT="detected"
  SERVER_EXTRACT_WALL=30.000
  mkdir -p "$WORK"
  cp /proc/stat "$WORK/client.before.cpu"
  cp /proc/stat "$WORK/client.after.cpu"
  printf '{ "totalBytes": 524288 }' > "$WORK/manifest.json"
  print_table
  print_verdict
  note "self-check: expected speedup 2.00x and PASS"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    -u|--url) URL=${2:-}; shift 2 ;;
    -t|--token) TOKEN=${2:-}; shift 2 ;;
    -g|--game) GAME=${2:-}; shift 2 ;;
    -v|--version) VERSION=${2:-}; shift 2 ;;
    -j|--jobs) JOBS=${2:-8}; shift 2 ;;
    -w|--work) WORK=${2:-}; shift 2 ;;
    -s|--server-ssh) SERVER_SSH=${2:-}; shift 2 ;;
    --extract-cmd) EXTRACT_CMD=${2:-}; shift 2 ;;
    --no-resume) RUN_RESUME=0; shift ;;
    --no-corrupt) RUN_CORRUPT=0; shift ;;
    --classic-only) RUN_MANIFEST=0; RUN_RESUME=0; RUN_CORRUPT=0; shift ;;
    --manifest-only) RUN_CLASSIC=0; shift ;;
    --self-check) SELF_CHECK=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) die "unknown option: $1 (try --help)" ;;
  esac
done

for tool in curl sha256sum python3; do
  command -v "$tool" >/dev/null 2>&1 || die "$tool is required"
done

if [[ "$SELF_CHECK" == "1" ]]; then
  WORK=${WORK:-$(mktemp -d)}
  self_check
  exit 0
fi

[[ -n "$URL" ]] || die "--url is required"
[[ -n "$TOKEN" ]] || die "--token is required (administrator bearer token)"
[[ -n "$VERSION" ]] || die "--version is required"
if [[ "$RUN_CLASSIC" == "1" && -z "$GAME" ]]; then
  die "--game is required for the classic download"
fi
URL=${URL%/}
WORK=${WORK:-$(mktemp -d)}
mkdir -p "$WORK"
[[ "$JOBS" =~ ^[0-9]+$ ]] && (( JOBS >= 1 )) || die "--jobs must be a positive integer"
DEST=""

note "work directory: $WORK"
note "server: $URL, version: $VERSION, jobs: $JOBS"

if [[ "$RUN_CLASSIC" == "1" ]]; then
  run_classic
fi
if [[ "$RUN_MANIFEST" == "1" ]]; then
  run_manifest
  if [[ "$RUN_RESUME" == "1" ]]; then
    run_resume
  fi
  if [[ "$RUN_CORRUPT" == "1" ]]; then
    run_corrupt
  fi
fi

print_table
print_verdict
printf 'work directory kept at %s\n' "$WORK"
