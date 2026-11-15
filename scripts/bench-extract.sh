#!/usr/bin/env bash
# Extraction benchmark for Ludex (#232).
#
#   scripts/bench-extract.sh [options] <archive>
#
# Compares the classic extractor against the parallel/native paths added in
# #232 on one archive, and prints one table plus the 1.5x verdict from the
# issue:
#
#   ZIP  : workers 1 (sequential) vs 2/4/8 (parallel inflate)
#   7z   : sevenz-rust2 (LUDEX_SEVEN_ZIP=crate) vs the system 7z (-mmt=on)
#   RAR  : a single sequential row; RAR5 solid is sequential by design
#
# Metrics per run: wall time, effective MB/s (declared bytes), CPU per core,
# peak RSS, filesystem reads/writes, and the cancellation latency for the
# `--cancel-after` row. Results are best-of-N so a noisy neighbour does not
# decide the verdict.
#
# Options:
#   -d, --dest DIR        extraction root (default ${TMPDIR:-/tmp}/ludex-bench-extract)
#   -w, --workers LIST    ZIP worker counts (default 1,2,4,8)
#   -r, --runs N          repetitions per row, best kept (default 1)
#   -f, --format FMT      auto|zip|7z|rar (default auto, from the extension)
#   -c, --cancel-after S  also measure cancellation latency after S seconds (default 5)
#   -b, --bin PATH        prebuilt extract example (default: cargo build --release)
#   -k, --keep            keep the extracted trees
#   -s, --self-check      offline check of the table/verdict logic, no archive
#   -h, --help
#
# The 40 GB procedure is in docs/PERFORMANCE.md. Run the script on the same
# host, with the archive on the disk you ship from, and put the printed table
# in the issue.
set -uo pipefail

usage() {
  sed -n '2,40p' "$0" | sed 's/^# \{0,1\}//'
}

die() {
  printf 'error: %s\n' "$*" >&2
  exit 2
}

# Ratio of two wall times, two decimals. Floats: awk decides, not `[`.
ratio() {
  local numerator=$1 denominator=$2
  awk -v n="$numerator" -v d="$denominator" 'BEGIN {
    if (d + 0 <= 0) printf "0"; else printf "%.2f", n / d;
  }'
}

cpu_from_percent() {
  # GNU time prints `912%` (one core is 100%); show cores.
  local value=${1%\%}
  awk -v v="$value" 'BEGIN { printf "%.1f", v / 100 }'
}

blocks_to_mib() {
  # rusage block counts are 512-byte units.
  awk -v b="${1:-0}" 'BEGIN { printf "%.0f", b * 512 / 1048576 }'
}

# Verdict for a speedup ratio (new/baseline both in seconds).
# 1.50 and above passes; below 1.05 is "no win".
verdict() {
  awk -v r="$1" 'BEGIN {
    if (r + 0 >= 1.50) print "PASS (>= 1.5x)";
    else if (r + 0 >= 1.05) print "below gate";
    else print "no win";
  }'
}

format_of() {
  local archive=$1
  case ${archive,,} in
    *.zip) printf 'zip' ;;
    *.7z) printf '7z' ;;
    *.rar) printf 'rar' ;;
    *) printf 'unknown' ;;
  esac
}

self_check() {
  local failures=0
  check() {
    local label=$1 actual=$2 expected=$3
    if [ "$actual" != "$expected" ]; then
      printf 'self-check FAIL: %s: %s != %s\n' "$label" "$actual" "$expected" >&2
      failures=$((failures + 1))
    fi
  }

  check "format zip" "$(format_of '/tmp/Game.ZIP')" "zip"
  check "format 7z" "$(format_of 'a.7z')" "7z"
  check "format rar" "$(format_of 'a.rar')" "rar"
  check "format other" "$(format_of 'a.tar')" "unknown"
  check "verdict 2x" "$(verdict 2.0)" "PASS (>= 1.5x)"
  check "verdict 1.2x" "$(verdict 1.2)" "below gate"
  check "verdict 1x" "$(verdict 1.0)" "no win"
  check "verdict 1.5x" "$(verdict 1.5)" "PASS (>= 1.5x)"
  check "ratio" "$(ratio 10 4)" "2.50"
  check "ratio zero" "$(ratio 10 0)" "0"
  check "cpu" "$(cpu_from_percent '912%')" "9.1"
  check "blocks" "$(blocks_to_mib 2048)" "1"

  if [ "$failures" -eq 0 ]; then
    printf 'self-check: ok\n'
    return 0
  fi
  printf 'self-check: %d failure(s)\n' "$failures" >&2
  return 1
}

dest_root="${TMPDIR:-/tmp}/ludex-bench-extract"
workers_list="1,2,4,8"
runs=1
format="auto"
cancel_after=5
binary=""
keep=0
archive=""

while [ "$#" -gt 0 ]; do
  case "$1" in
    -d | --dest)
      [ "$#" -ge 2 ] || die "--dest requires a directory"
      dest_root=$2
      shift 2
      ;;
    -w | --workers)
      [ "$#" -ge 2 ] || die "--workers requires a list"
      workers_list=$2
      shift 2
      ;;
    -r | --runs)
      [ "$#" -ge 2 ] || die "--runs requires a number"
      runs=$2
      shift 2
      ;;
    -f | --format)
      [ "$#" -ge 2 ] || die "--format requires a value"
      format=$2
      shift 2
      ;;
    -c | --cancel-after)
      [ "$#" -ge 2 ] || die "--cancel-after requires seconds"
      cancel_after=$2
      shift 2
      ;;
    -b | --bin)
      [ "$#" -ge 2 ] || die "--bin requires a path"
      binary=$2
      shift 2
      ;;
    -k | --keep)
      keep=1
      shift
      ;;
    -s | --self-check)
      self_check
      exit $?
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    --)
      shift
      ;;
    -*)
      die "unknown option $1"
      ;;
    *)
      [ -z "$archive" ] || die "only one archive is supported"
      archive=$1
      shift
      ;;
  esac
done

[ -n "$archive" ] || die "an archive is required (or --self-check)"
[ -f "$archive" ] || die "archive not found: $archive"
case $runs in
  '' | *[!0-9]*) die "--runs must be a positive integer" ;;
esac
[ "$runs" -ge 1 ] || die "--runs must be at least 1"

if [ "$format" = "auto" ]; then
  format=$(format_of "$archive")
fi
case $format in
  zip | 7z | rar) ;;
  *) die "cannot tell the format of $archive; pass --format" ;;
esac

# The example binary is the measurement harness: `cargo run` would add its own
# process and hide the exit code, so build once and run the binary directly.
if [ -z "$binary" ]; then
  printf 'building the extract example (release)…\n' >&2
  cargo build --release -q -p ludex-core --example extract || die "cargo build failed"
  target_dir=${CARGO_TARGET_DIR:-target}
  case $target_dir in
    /*) binary="$target_dir/release/examples/extract" ;;
    *) binary="$PWD/$target_dir/release/examples/extract" ;;
  esac
fi
[ -x "$binary" ] || die "extract binary not executable: $binary"

time_bin=""
if [ -x /usr/bin/time ]; then
  time_bin=/usr/bin/time
fi

size_bytes=$(wc -c <"$archive")
size_mib=$(awk -v b="$size_bytes" 'BEGIN { printf "%.0f", b / 1048576 }')

run_once() {
  # run_once WORKERS ENV_ASSIGNMENT LABEL CANCEL
  local workers=$1 env_assignment=$2 cancel=${3:-}
  local dir="$dest_root/run"
  rm -rf "$dir"
  mkdir -p "$dir"

  local metrics="$dest_root/metrics"
  local out="$dest_root/stdout.json"
  local args=(--workers "$workers")
  if [ -n "$cancel" ]; then
    args+=(--cancel-after "$cancel")
  fi

  if [ -n "$time_bin" ]; then
    env $env_assignment "$time_bin" -f '%e %P %M %I %O' -o "$metrics" \
      "$binary" "$archive" "$dir" "${args[@]}" >"$out" 2>"$dest_root/stderr"
  else
    local started ended
    started=$(date +%s%N)
    env $env_assignment "$binary" "$archive" "$dir" "${args[@]}" >"$out" 2>"$dest_root/stderr"
    ended=$(date +%s%N)
    printf '%s 100 0 0 0\n' "$(awk -v s="$started" -v e="$ended" 'BEGIN { printf "%.2f", (e - s) / 1000000000 }')" >"$metrics"
  fi
  # Command substitution runs this function in a subshell: the status must
  # travel on stdout, not through a global.
  local status=$?

  local wall cpu rss blocks_in blocks_out
  read -r wall cpu rss blocks_in blocks_out <"$metrics" || true
  printf '%s %s %s %s %s %s\n' \
    "${wall:-0}" "${cpu:-0}" "${rss:-0}" "${blocks_in:-0}" "${blocks_out:-0}" "$status"
}

best_of() {
  # best_of WORKERS ENV CANCEL -> "wall cpu rss blocks_in blocks_out exit"
  local workers=$1 env_assignment=$2 cancel=${3:-}
  local best_wall=""
  local best_line=""
  local line exit_code=0
  local attempt=1
  while [ "$attempt" -le "$runs" ]; do
    line=$(run_once "$workers" "$env_assignment" "$cancel")
    exit_code=$(awk '{print $6}' <<<"$line")
    local wall=${line%% *}
    if [ -z "$best_wall" ] || awk -v a="$wall" -v b="$best_wall" 'BEGIN { exit !(a < b) }'; then
      best_wall=$wall
      best_line=$line
    fi
    attempt=$((attempt + 1))
  done
  printf '%s\n' "$best_line"
}

rows_file="$dest_root/rows.tsv"
mkdir -p "$dest_root"
: >"$rows_file"

add_row() {
  # add_row BACKEND WORKERS WALL CPU RSS BLOCKS_IN BLOCKS_OUT EXIT
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$@" >>"$rows_file"
}

run_row() {
  # run_row BACKEND WORKERS ENV [CANCEL]
  local backend=$1 workers=$2 env_assignment=$3 cancel=${4:-}
  local result wall cpu rss blocks_in blocks_out exit_code
  result=$(best_of "$workers" "$env_assignment" "$cancel")
  wall=$(awk '{print $1}' <<<"$result")
  cpu=$(awk '{print $2}' <<<"$result")
  rss=$(awk '{print $3}' <<<"$result")
  blocks_in=$(awk '{print $4}' <<<"$result")
  blocks_out=$(awk '{print $5}' <<<"$result")
  exit_code=$(awk '{print $6}' <<<"$result")
  add_row "$backend" "$workers" "$wall" "$cpu" "$rss" "$blocks_in" "$blocks_out" "$exit_code"
}

seven_zip_binary() {
  local candidate
  for candidate in 7zz 7z 7za; do
    if command -v "$candidate" >/dev/null 2>&1; then
      command -v "$candidate"
      return 0
    fi
  done
  return 1
}

printf 'archive: %s (%s MiB, %s)\n' "$archive" "$size_mib" "$format"
printf 'binary : %s\n' "$binary"
printf 'runs   : %s (best kept)\n\n' "$runs"

case $format in
  zip)
    IFS=',' read -r -a workers <<<"$workers_list"
    for worker in "${workers[@]}"; do
      run_row "zip" "$worker" "LUDEX_SEVEN_ZIP=crate"
    done
    ;;
  7z)
    run_row "7z-crate" 1 "LUDEX_SEVEN_ZIP=crate"
    if seven_zip_path=$(seven_zip_binary); then
      run_row "7z-native" 1 "LUDEX_SEVEN_ZIP="
    else
      printf 'note: no system 7z/7zz found; native 7z row skipped\n' >&2
    fi
    ;;
  rar)
    run_row "rar" 1 "LUDEX_SEVEN_ZIP=crate"
    printf 'note: RAR5 solid archives are sequential by design (#232); a single row is the honest result\n\n' >&2
    ;;
esac

printf '%-10s %-8s %9s %10s %7s %10s %9s %9s %6s\n' \
  "backend" "workers" "wall(s)" "MB/s" "cores" "peakRSS" "readMiB" "writeMiB" "exit"
printf '%-10s %-8s %9s %10s %7s %10s %9s %9s %6s\n' \
  "----------" "--------" "---------" "----------" "-------" "----------" "---------" "---------" "------"

best_sequential=""
best_parallel=""
best_crate=""
best_native=""

while IFS=$'\t' read -r backend workers wall cpu rss blocks_in blocks_out exit_code; do
  [ -n "$backend" ] || continue
  throughput=$(awk -v b="$size_bytes" -v w="$wall" 'BEGIN { if (w + 0 > 0) printf "%.1f", b / w / 1000000; else printf "0" }')
  printf '%-10s %-8s %9s %10s %7s %10s %9s %9s %6s\n' \
    "$backend" "$workers" "$wall" "$throughput" "$(cpu_from_percent "$cpu")" \
    "$(blocks_to_mib "$rss")" "$(blocks_to_mib "$blocks_in")" "$(blocks_to_mib "$blocks_out")" "$exit_code"
  case "$backend:$workers" in
    zip:1) best_sequential=$wall ;;
    zip:*) if [ -z "$best_parallel" ] || awk -v a="$wall" -v b="$best_parallel" 'BEGIN { exit !(a < b) }'; then best_parallel=$wall; fi ;;
  esac
  case "$backend" in
    7z-crate) best_crate=$wall ;;
    7z-native) best_native=$wall ;;
  esac
done <"$rows_file"

printf '\n'
case $format in
  zip)
    if [ -n "$best_sequential" ] && [ -n "$best_parallel" ]; then
      speedup=$(ratio "$best_sequential" "$best_parallel")
      printf 'parallel vs sequential ZIP: %sx — %s\n' "$speedup" "$(verdict "$speedup")"
    fi
    ;;
  7z)
    if [ -n "$best_crate" ] && [ -n "$best_native" ]; then
      speedup=$(ratio "$best_crate" "$best_native")
      printf 'native vs crate 7z: %sx — %s\n' "$speedup" "$(verdict "$speedup")"
    else
      printf 'native vs crate 7z: no system binary, verdict unknown\n'
    fi
    ;;
  rar)
    printf 'RAR: sequential by design (RAR5 solid); the gate does not apply\n'
    ;;
esac

# Cancellation latency (#232): wall time from process start to exit minus the
# moment the flag was raised. The flag must land mid-run, so a requested delay
# longer than the fastest extraction is clamped to half of it.
if awk -v s="$cancel_after" 'BEGIN { exit !(s > 0) }'; then
  case $format in
    zip) cancel_workers=8 ;;
    *) cancel_workers=1 ;;
  esac
  fastest=$(awk -F '\t' 'BEGIN { best = 0 } { if (best == 0 || $3 + 0 < best) best = $3 + 0 } END { print best }' "$rows_file")
  delay=$(awk -v d="$cancel_after" -v f="$fastest" 'BEGIN {
    half = f / 2;
    if (half < 0.05) half = 0.05;
    printf "%.2f", (d > half ? half : d);
  }')
  result=$(best_of "$cancel_workers" "LUDEX_SEVEN_ZIP=crate" "$delay")
  cancel_wall=$(awk '{print $1}' <<<"$result")
  cancel_exit=$(awk '{print $6}' <<<"$result")
  latency=$(awk -v w="$cancel_wall" -v d="$delay" 'BEGIN { l = w - d; if (l < 0) l = 0; printf "%.2f", l }')
  printf 'cancellation latency: %ss after a %ss request (exit %s)\n' "$latency" "$delay" "$cancel_exit"
fi

if [ "$keep" -eq 0 ]; then
  rm -rf "$dest_root/run"
fi

printf '\nrecord this table and the host (CPU, disk, Defender state) in #232\n'
