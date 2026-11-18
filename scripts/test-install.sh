#!/usr/bin/env bash
# Offline fixture test for install.sh (#275).
#
# A local `python3 -m http.server` serves fake release assets signed with a
# throwaway minisign key; nothing is downloaded from GitHub. The install.sh copy
# under test has exactly one line patched (the pinned pubkey) so the throwaway
# key is the trust anchor; the real script's pin is compared with
# apps/desktop/src-tauri/tauri.conf.json first, so the patch cannot hide drift.
#
# Cases: valid install, --version pin, in-place upgrade, tampered artifact,
# tampered SHA256SUMS (before the artifact is fetched), tampered signature,
# unsupported architecture, missing minisign, missing libwebkit2gtk-4.1,
# --uninstall keeps user data, --purge removes it, usage errors, edge channel.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
INSTALLER="$ROOT/install.sh"
ICON_SOURCE="$ROOT/apps/desktop/src-tauri/icons/128x128@2x.png"
VERSION="0.4.22"
TAG="v$VERSION"
ASSET="Ludex_${VERSION}_amd64.AppImage"
EDGE_VERSION="0.4.23-edge.1"
EDGE_ASSET="Ludex_${EDGE_VERSION}_amd64.AppImage"

CASE="setup"
OUT=""
RC=0
SHIM=""
ARCH_OVERRIDE=""
SERVER_PID=""

for tool in minisign python3 curl sha256sum base64; do
	if ! command -v "$tool" >/dev/null 2>&1; then
		printf 'SKIP: %s is not installed; the fixture test needs minisign, python3, curl, sha256sum and base64 (no real asset is downloaded).\n' "$tool" >&2
		exit 0
	fi
done

WORK=$(mktemp -d "${TMPDIR:-/tmp}/ludex-install-test.XXXXXX")
cleanup() {
	if [ -n "$SERVER_PID" ]; then
		kill "$SERVER_PID" 2>/dev/null || true
		wait "$SERVER_PID" 2>/dev/null || true
	fi
	rm -rf "$WORK"
}
trap cleanup EXIT

fail() {
	printf 'FAIL [%s]: %s\n' "$CASE" "$*" >&2
	exit 1
}

ok() {
	printf 'ok - %s\n' "$*"
}

assert_eq() { # $1 got, $2 want, $3 message
	[ "$1" = "$2" ] || fail "$3 (expected '$2', got '$1')"
}

assert_file() {
	[ -f "$1" ] || fail "$2 (missing file: $1)"
}

assert_no_file() {
	[ ! -e "$1" ] || fail "$2 (unexpected file: $1)"
}

assert_contains() { # $1 haystack, $2 needle, $3 message
	case $1 in
		*"$2"*) ;;
		*) fail "$3 (missing '$2' in output):
$1" ;;
	esac
}

# --- static checks ---------------------------------------------------------

CASE="static"
if grep -Eq '(^|[^[:alnum:]_])eval([^[:alnum:]_]|$)' "$INSTALLER"; then
	fail "install.sh must not use eval"
fi
if grep -Ev '^[[:space:]]*#' "$INSTALLER" | grep -Eq 'curl[^|]*\|[[:space:]]*(ba)?sh'; then
	fail "install.sh must not pipe curl into a shell"
fi
python3 - "$INSTALLER" "$ROOT/apps/desktop/src-tauri/tauri.conf.json" <<'PY'
import json, re, sys

source = open(sys.argv[1], encoding="utf-8").read()
match = re.search(r"^PUBKEY_B64='([A-Za-z0-9+/=]+)'$", source, re.M)
assert match, "install.sh has no PUBKEY_B64 line"
config = json.load(open(sys.argv[2], encoding="utf-8"))
pinned = config["plugins"]["updater"]["pubkey"]
assert match.group(1) == pinned, "install.sh pubkey does not match tauri.conf.json plugins.updater.pubkey"
PY
ok "install.sh has no eval, no curl|sh, and pins plugins.updater.pubkey"

# --- fixture release -------------------------------------------------------

mkdir -p "$WORK/keys"
minisign -G -p "$WORK/keys/fixture.pub" -s "$WORK/keys/fixture.key" -W >/dev/null
FIXTURE_PUB_B64=$(base64 <"$WORK/keys/fixture.pub" | tr -d '\n')
FIXTURE_KEY_LINE=$(sed -n '2p' "$WORK/keys/fixture.pub")

# The copy under test differs from install.sh by exactly the pinned key line.
sed "s|^PUBKEY_B64=.*|PUBKEY_B64='$FIXTURE_PUB_B64'|" "$INSTALLER" >"$WORK/install-fixed.sh"
grep -qF "PUBKEY_B64='$FIXTURE_PUB_B64'" "$WORK/install-fixed.sh" ||
	fail "could not patch the pinned key in the installer copy"
INSTALLER_TEST="$WORK/install-fixed.sh"

SERVE="$WORK/serve"
REL="$SERVE/releases/download/$TAG"
EDGE_DIR="$SERVE/releases/download/edge"
LATEST_DIR="$SERVE/releases/latest/download"
mkdir -p "$REL" "$EDGE_DIR" "$LATEST_DIR"

make_artifact() {
	printf 'ludex fixture artifact\n' >"$1"
	head -c 8192 /dev/urandom >>"$1"
}

sign_asset() { # $1 file, $2 signature output (base64 like the release assets)
	minisign -S -s "$WORK/keys/fixture.key" -m "$1" -x "$1.minisig" >/dev/null
	base64 <"$1.minisig" | tr -d '\n' >"$2"
}

write_manifest() { # $1 version
	cat >"$2" <<EOF
{
  "version": "$1",
  "notes": "",
  "pub_date": "2026-11-18T00:00:00.000Z",
  "platforms": {
    "linux-x86_64": {
      "signature": "",
      "url": "http://127.0.0.1/Ludex_$1_amd64.AppImage"
    }
  }
}
EOF
}

make_artifact "$REL/$ASSET"
(cd "$REL" && sha256sum "$ASSET" >SHA256SUMS)
sign_asset "$REL/SHA256SUMS" "$REL/SHA256SUMS.sig"
rm -f "$REL/SHA256SUMS.minisig"
write_manifest "$VERSION" "$LATEST_DIR/latest.json"

make_artifact "$EDGE_DIR/$EDGE_ASSET"
sign_asset "$EDGE_DIR/$EDGE_ASSET" "$EDGE_DIR/$EDGE_ASSET.sig"
write_manifest "$EDGE_VERSION" "$EDGE_DIR/latest.json"

cp "$REL/$ASSET" "$WORK/asset.orig"
cp "$REL/SHA256SUMS" "$WORK/sums.orig"
cp "$REL/SHA256SUMS.sig" "$WORK/sums.sig.orig"

# --- fixture server --------------------------------------------------------

PORT=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')
python3 -m http.server "$PORT" --bind 127.0.0.1 --directory "$SERVE" >"$WORK/server.log" 2>&1 &
SERVER_PID=$!
BASE="http://127.0.0.1:$PORT"
READY=0
for _ in $(seq 1 100); do
	if curl -fsS -o /dev/null "$BASE/releases/latest/download/latest.json" 2>/dev/null; then
		READY=1
		break
	fi
	sleep 0.1
done
if [ "$READY" != 1 ]; then
	cat "$WORK/server.log" >&2
	fail "the fixture http.server did not start on port $PORT"
fi
ok "fixture server on $BASE"

# --- helpers ---------------------------------------------------------------

run_installer() { # $1 home dir, remaining args
	local home=$1
	shift
	local shell_bin="sh"
	local path_value="$PATH"
	if [ -n "$SHIM" ]; then
		shell_bin="$SHIM/sh"
		path_value="$SHIM"
	fi
	set +e
	OUT=$(HOME="$home" PATH="$path_value" LC_ALL=C LUDEX_BASE_URL="$BASE" \
		LUDEX_ARCH_OVERRIDE="$ARCH_OVERRIDE" "$shell_bin" "$INSTALLER_TEST" "$@" 2>&1)
	RC=$?
	set -e
}

count_asset_gets() {
	local n
	n=$(grep -c "GET .*$ASSET HTTP" "$WORK/server.log" || true)
	printf '%s' "${n:-0}"
}

make_shim() { # $1 dir, $2 1 to include minisign
	local dir=$1
	local with_minisign=$2
	mkdir -p "$dir"
	local tool src
	for tool in sh uname mktemp curl base64 sha256sum awk sed grep head tr cp mv rm chmod mkdir cat date id; do
		src=$(command -v "$tool") || fail "the fixture shim needs $tool on the host"
		ln -sf "$src" "$dir/$tool"
	done
	if [ "$with_minisign" = 1 ]; then
		ln -sf "$(command -v minisign)" "$dir/minisign"
	fi
	# Pretend the host has no webkit (and no package manager to suggest).
	printf '#!/bin/sh\nexit 0\n' >"$dir/ldconfig"
	printf '#!/bin/sh\nexit 0\n' >"$dir/find"
	chmod +x "$dir/ldconfig" "$dir/find"
}

# --- 1. valid AppImage install --------------------------------------------

CASE="valid install"
HOME_A="$WORK/home-a"
mkdir -p "$HOME_A"
run_installer "$HOME_A" --force
assert_eq "$RC" 0 "install.sh must exit 0:
$OUT"
assert_file "$HOME_A/.local/bin/ludex" "the AppImage must be installed"
[ -x "$HOME_A/.local/bin/ludex" ] || fail "the installed binary must be executable"
assert_file "$HOME_A/.local/share/applications/ludex.desktop" "the desktop entry must be installed"
assert_file "$HOME_A/.local/share/icons/hicolor/256x256/apps/ludex.png" "the icon must be installed"
assert_file "$HOME_A/.local/share/ludex/install.json" "the install record must exist"
assert_contains "$OUT" "signature verified (SHA256SUMS)" "the checksum list must be signature-verified"
assert_contains "$OUT" "sha256 verified ($ASSET)" "the artifact hash must be checked"
assert_contains "$OUT" "ludex $VERSION installed at $HOME_A/.local/bin/ludex" "the final line must name the version and path"
grep -qF "Exec=\"$HOME_A/.local/bin/ludex\" %U" "$HOME_A/.local/share/applications/ludex.desktop" ||
	fail "the desktop entry must exec the installed binary"
cmp -s "$HOME_A/.local/share/icons/hicolor/256x256/apps/ludex.png" "$ICON_SOURCE" ||
	fail "the embedded icon drifted from apps/desktop/src-tauri/icons/128x128@2x.png"
python3 - "$HOME_A/.local/share/ludex/install.json" "$HOME_A/.local" "$VERSION" <<'PY'
import json, os, sys

record = json.load(open(sys.argv[1], encoding="utf-8"))
prefix, version = sys.argv[2], sys.argv[3]
assert record["version"] == version, record
assert record["channel"] == "stable", record
assert record["method"] == "appimage", record
assert record["prefix"] == prefix, record
files = record["files"]
assert len(files) == 3, files
for path in files:
    assert os.path.isfile(path), path
    assert path.startswith(prefix + "/"), path
PY
ok "valid install: $ASSET verified, installed and recorded"

# --- 2. --version pins a tag without latest.json ---------------------------

CASE="--version pin"
HOME_B="$WORK/home-b"
mkdir -p "$HOME_B"
mv "$LATEST_DIR/latest.json" "$WORK/latest.json.bak"
run_installer "$HOME_B" --version "$VERSION" --force
assert_eq "$RC" 0 "--version must not need latest.json:
$OUT"
assert_contains "$OUT" "tag $TAG" "--version must resolve to the pinned tag"
assert_file "$HOME_B/.local/bin/ludex" "the pinned install must land"
mv "$WORK/latest.json.bak" "$LATEST_DIR/latest.json"
ok "--version $VERSION pins the tag"

# --- 3. re-run upgrades in place, keeps the recorded prefix ----------------

CASE="idempotent upgrade"
HOME_C="$WORK/home-c"
CUSTOM="$HOME_C/opt/ludex"
mkdir -p "$HOME_C"
run_installer "$HOME_C" --force --dir "$CUSTOM"
assert_eq "$RC" 0 "the first custom-prefix install must succeed:
$OUT"
assert_file "$CUSTOM/bin/ludex" "the custom prefix must receive the binary"
run_installer "$HOME_C" --force
assert_eq "$RC" 0 "the upgrade must succeed:
$OUT"
assert_contains "$OUT" "upgrading the install recorded in" "a re-run must reuse the record"
assert_file "$CUSTOM/bin/ludex" "the recorded prefix must survive a re-run"
assert_no_file "$HOME_C/.local/bin/ludex" "a re-run must not move the install"
python3 - "$CUSTOM/share/applications/ludex.desktop" "$CUSTOM/bin/ludex" <<'PY'
import sys

entry = open(sys.argv[1], encoding="utf-8").read()
assert f'Exec="{sys.argv[2]}" %U' in entry, entry
PY
ok "a re-run upgrades in place and keeps the recorded prefix"

# --- 4. tampered artifact --------------------------------------------------

CASE="tampered artifact"
HOME_D="$WORK/home-d"
mkdir -p "$HOME_D"
printf 'tampered' >>"$REL/$ASSET"
run_installer "$HOME_D" --force
assert_eq "$RC" 1 "a tampered artifact must fail"
assert_contains "$OUT" "checksum mismatch for $ASSET" "the failure must be a checksum error"
assert_no_file "$HOME_D/.local/bin/ludex" "a tampered artifact must not be installed"
cp "$WORK/asset.orig" "$REL/$ASSET"
ok "a tampered artifact fails the checksum and stays uninstalled"

# --- 5. tampered SHA256SUMS fails before the artifact is downloaded --------

CASE="tampered SHA256SUMS"
HOME_E="$WORK/home-e"
mkdir -p "$HOME_E"
cp "$WORK/sums.orig" "$REL/SHA256SUMS"
printf '0000000000000000000000000000000000000000000000000000000000000000  extra-file\n' >>"$REL/SHA256SUMS"
gets_before=$(count_asset_gets)
run_installer "$HOME_E" --force
assert_eq "$RC" 1 "a tampered SHA256SUMS must fail"
assert_contains "$OUT" "signature verification failed for SHA256SUMS" "the failure must be a signature error"
gets_after=$(count_asset_gets)
assert_eq "$gets_after" "$gets_before" "the artifact must not be downloaded before SHA256SUMS is verified"
assert_no_file "$HOME_E/.local/bin/ludex" "a tampered SHA256SUMS must not install anything"
cp "$WORK/sums.orig" "$REL/SHA256SUMS"
ok "a tampered SHA256SUMS fails before the artifact is downloaded"

# --- 6. tampered signature -------------------------------------------------

CASE="tampered signature"
HOME_F="$WORK/home-f"
mkdir -p "$HOME_F"
python3 - "$WORK/sums.sig.orig" "$WORK/bad.minisig" <<'PY'
import base64, sys

raw = open(sys.argv[1], encoding="utf-8").read()
lines = base64.b64decode(raw).decode("utf-8").splitlines()
signature = list(lines[1])
signature[10] = "B" if signature[10] != "B" else "C"
lines[1] = "".join(signature)
open(sys.argv[2], "w", encoding="utf-8").write("\n".join(lines) + "\n")
PY
base64 <"$WORK/bad.minisig" | tr -d '\n' >"$REL/SHA256SUMS.sig"
run_installer "$HOME_F" --force
assert_eq "$RC" 1 "a tampered signature must fail"
assert_contains "$OUT" "signature verification failed for SHA256SUMS" "the failure must be a signature error"
assert_no_file "$HOME_F/.local/bin/ludex" "a tampered signature must not install anything"
cp "$WORK/sums.sig.orig" "$REL/SHA256SUMS.sig"
ok "a tampered SHA256SUMS.sig fails verification"

# --- 7. unsupported architecture ------------------------------------------

CASE="unsupported architecture"
HOME_G="$WORK/home-g"
mkdir -p "$HOME_G"
ARCH_OVERRIDE="aarch64"
run_installer "$HOME_G" --force
ARCH_OVERRIDE=""
assert_eq "$RC" 2 "a non-x86_64 host must exit 2"
assert_contains "$OUT" "unsupported architecture: aarch64" "the refusal must name the architecture"
assert_contains "$OUT" "x86_64 (amd64)" "the refusal must say which builds exist"
assert_no_file "$HOME_G/.local/bin/ludex" "an unsupported host must not install anything"
ok "a non-x86_64 host is refused with exit 2"

# --- 8. missing minisign prints the audited commands -----------------------

CASE="missing minisign"
HOME_H="$WORK/home-h"
mkdir -p "$HOME_H"
SHIM="$WORK/shim-no-minisign"
make_shim "$SHIM" 0
run_installer "$HOME_H"
SHIM=""
assert_eq "$RC" 1 "a host without minisign must exit 1"
assert_contains "$OUT" "minisign is not installed" "the refusal must explain itself"
assert_contains "$OUT" "minisign -Vm SHA256SUMS" "the audited verify command must be printed"
assert_contains "$OUT" "$FIXTURE_KEY_LINE" "the pinned key must be printed verbatim"
assert_contains "$OUT" "sha256sum -c --ignore-missing SHA256SUMS" "the checksum step must be printed"
assert_no_file "$HOME_H/.local/bin/ludex" "a host without minisign must not install anything"
ok "missing minisign prints the audited commands and exits 1"

# --- 9. missing libwebkit2gtk-4.1 needs --force ---------------------------

CASE="missing libwebkit2gtk-4.1"
HOME_I="$WORK/home-i"
mkdir -p "$HOME_I"
SHIM="$WORK/shim-webkit"
make_shim "$SHIM" 1
run_installer "$HOME_I"
assert_eq "$RC" 1 "a missing runtime library must exit 1"
assert_contains "$OUT" "libwebkit2gtk-4.1 is missing" "the preflight must name the library"
assert_contains "$OUT" "install it with:" "the preflight must print the install command"
assert_contains "$OUT" "rerun with --force" "the preflight must offer --force"
assert_no_file "$HOME_I/.local/bin/ludex" "a missing runtime must block the install"

CASE="missing libwebkit2gtk-4.1 --force"
run_installer "$HOME_I" --force
assert_eq "$RC" 0 "--force must install anyway:
$OUT"
assert_file "$HOME_I/.local/bin/ludex" "--force must install the binary"
SHIM=""
ok "a missing libwebkit2gtk-4.1 blocks the install unless --force is passed"

# --- 10. --uninstall keeps user data --------------------------------------

CASE="--uninstall"
mkdir -p "$HOME_I/.local/share/ludex"
printf '{"saves": true}\n' >"$HOME_I/.local/share/ludex/user-data.json"
run_installer "$HOME_I" --uninstall
assert_eq "$RC" 0 "--uninstall must succeed:
$OUT"
assert_no_file "$HOME_I/.local/bin/ludex" "--uninstall must remove the binary"
assert_no_file "$HOME_I/.local/share/applications/ludex.desktop" "--uninstall must remove the desktop entry"
assert_no_file "$HOME_I/.local/share/icons/hicolor/256x256/apps/ludex.png" "--uninstall must remove the icon"
assert_no_file "$HOME_I/.local/share/ludex/install.json" "--uninstall must remove the record"
assert_file "$HOME_I/.local/share/ludex/user-data.json" "--uninstall must keep user data"
assert_contains "$OUT" "kept user data in $HOME_I/.local/share/ludex" "the output must say the data was kept"
ok "--uninstall removes the recorded files and keeps user data"

# --- 11. --purge removes user data ----------------------------------------

CASE="--purge"
run_installer "$HOME_I" --force
assert_eq "$RC" 0 "reinstalling before --purge must succeed:
$OUT"
run_installer "$HOME_I" --uninstall --purge
assert_eq "$RC" 0 "--uninstall --purge must succeed:
$OUT"
assert_no_file "$HOME_I/.local/share/ludex/user-data.json" "--purge must remove user data"
assert_no_file "$HOME_I/.local/share/ludex" "--purge must remove the data directory"
ok "--uninstall --purge removes the user data directory"

# --- 12. usage errors ------------------------------------------------------

CASE="usage errors"
HOME_K="$WORK/home-k"
mkdir -p "$HOME_K"
for args in "--nope" "--channel beta" "--dir" "--purge" "--uninstall --dir $HOME_K/x" "--uninstall --version v1.0.0" "--version v0.4.22 --channel edge"; do
	# shellcheck disable=SC2086 # the argument list is intentionally split
	run_installer "$HOME_K" $args
	assert_eq "$RC" 3 "usage error expected for: $args (got $RC):
$OUT"
	assert_contains "$OUT" "usage error" "the failure must be a usage error: $args"
done
ok "usage errors exit 3"

# --- 13. edge channel ------------------------------------------------------

CASE="edge channel"
HOME_L="$WORK/home-l"
mkdir -p "$HOME_L"
run_installer "$HOME_L" --channel edge --force
assert_eq "$RC" 0 "the edge channel must install:
$OUT"
assert_contains "$OUT" "channel edge, tag edge, version $EDGE_VERSION" "the edge manifest must resolve the version"
assert_contains "$OUT" "signature verified ($EDGE_ASSET)" "the edge artifact must be signature-verified"
assert_contains "$OUT" "ludex $EDGE_VERSION installed at" "the edge version must be reported"
python3 - "$HOME_L/.local/share/ludex/install.json" "$EDGE_VERSION" <<'PY'
import json, sys

record = json.load(open(sys.argv[1], encoding="utf-8"))
assert record["channel"] == "edge", record
assert record["version"] == sys.argv[2], record
PY
ok "--channel edge verifies the per-asset signature"

printf '\nall install.sh fixture cases passed\n'
