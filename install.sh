#!/bin/sh
# Ludex Linux installer — minisign-verified AppImage (default) or .deb.
#
#   curl -fsSL https://ludex.cleeryy.cloud/install.sh | sh
#   sh install.sh --version v0.4.22 --dir /opt/ludex
#   sh install.sh --uninstall --purge
#
# Every artifact is verified against the minisign public key committed in
# apps/desktop/src-tauri/tauri.conf.json before it is installed. The audited
# path (download, verify, run) stays available: see --help, and the commands
# this script prints itself when minisign is missing.
#
# Exit codes: 0 ok, 1 verification/dependency/install failure, 2 unsupported
# architecture, 3 usage error.
set -eu

REPO="cleeryy/ludex"
BASE_URL="${LUDEX_BASE_URL:-https://github.com/$REPO}"
API_URL="${LUDEX_API_URL:-https://api.github.com/repos/$REPO/releases/latest}"
DATA_HOME="${XDG_DATA_HOME:-$HOME/.local/share}"
DATA_DIR="$DATA_HOME/ludex"
RECORD="$DATA_DIR/install.json"

# Public key pinned from apps/desktop/src-tauri/tauri.conf.json
# (plugins.updater.pubkey: base64 of the two-line minisign .pub file). The same
# key signs SHA256SUMS.sig and every bundle, so one verification proves both.
PUBKEY_B64='dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDY3REYxRjdENkY4NDI1RDgKUldUWUpZUnZmUi9mWjdndnFKc3dvVkJSak9HeWdIYlJDZ0NreVVFRjVwUUZMWXBpRW53RHRoc0MK'

CHANNEL="stable"
CHANNEL_GIVEN=0
WANT_TAG=""
PREFIX=""
USE_DEB=0
UNINSTALL=0
PURGE=0
FORCE=0
TMP=""
PUB_KEY=""

usage() {
	cat <<'EOF'
Ludex Linux installer

usage: install.sh [options]

  --version <tag>   install an exact release (v0.4.22 or 0.4.22)
  --channel <name>  stable (default) or edge
  --deb             install the .deb with dpkg instead of the AppImage (apt only)
  --dir <path>      absolute install prefix (default: ~/.local, or the record)
  --uninstall       remove exactly what a previous run recorded
  --purge           with --uninstall: also delete the data in ~/.local/share/ludex
  --force           continue when libwebkit2gtk-4.1 is missing
  --help            print this text

Exit codes: 0 ok, 1 verification/dependency/install failure,
2 unsupported architecture, 3 usage error.

No telemetry. sudo is only used by --deb (dpkg).

The audited path, if you would rather not pipe this script into a shell:
  1. download Ludex_<version>_amd64.AppImage, SHA256SUMS and SHA256SUMS.sig
     from https://github.com/cleeryy/ludex/releases
  2. base64 -d SHA256SUMS.sig > SHA256SUMS.minisig
     minisign -Vm SHA256SUMS -x SHA256SUMS.minisig -P '<pinned key>'
  3. sha256sum -c --ignore-missing SHA256SUMS
  4. chmod +x Ludex_<version>_amd64.AppImage && ./Ludex_<version>_amd64.AppImage

The pinned key is the one in apps/desktop/src-tauri/tauri.conf.json
(plugins.updater.pubkey); the script prints it verbatim when minisign is
missing.
EOF
}

step() {
	printf 'ludex: %s\n' "$*"
}

die() {
	_code=$1
	shift
	printf 'ludex: %s\n' "$*" >&2
	exit "$_code"
}

need() {
	command -v "$1" >/dev/null 2>&1 || die 1 "missing required command: $1"
}

as_root() {
	if [ "$(id -u)" -eq 0 ]; then
		"$@"
	else
		sudo "$@"
	fi
}

json_value() {
	# First "key": "value" string of a small JSON document (release manifests).
	_value=$(sed -n "s/^[[:space:]]*\"$2\"[[:space:]]*:[[:space:]]*\"\([^\"]*\)\".*/\1/p" "$1" | head -n 1)
	if [ -z "$_value" ]; then
		_value=$(tr -d '\n' < "$1" | sed -n "s/.*\"$2\"[[:space:]]*:[[:space:]]*\"\([^\"]*\)\".*/\1/p")
	fi
	printf '%s' "$_value"
}

fetch() {
	# Quiet, non-fatal GET used while resolving the version.
	curl -fsSL --retry 2 --retry-delay 2 -o "$2" "$1" 2>/dev/null
}

download() {
	step "downloading $3"
	curl -fsSL --retry 3 --retry-delay 2 -o "$2" "$1" || die 1 "download failed: $1"
}

normalize_tag() {
	case $1 in
		edge) printf 'edge' ;;
		v*) printf '%s' "$1" ;;
		*) printf 'v%s' "$1" ;;
	esac
}

resolve_tag() {
	# --version pins a tag; edge is a rolling prerelease; stable comes from
	# latest.json with the releases API as the fallback.
	if [ -n "$WANT_TAG" ]; then
		printf '%s' "$WANT_TAG"
		return 0
	fi
	if [ "$CHANNEL" = "edge" ]; then
		printf 'edge'
		return 0
	fi
	if fetch "$BASE_URL/releases/latest/download/latest.json" "$TMP/latest.json"; then
		_value=$(json_value "$TMP/latest.json" version)
		if [ -n "$_value" ]; then
			printf 'v%s' "$_value"
			return 0
		fi
	fi
	if fetch "$API_URL" "$TMP/release.json"; then
		_value=$(json_value "$TMP/release.json" tag_name)
		if [ -n "$_value" ]; then
			printf '%s' "$_value"
			return 0
		fi
	fi
	die 1 "could not resolve the latest release tag (network or GitHub API)"
}

resolve_edge_version() {
	fetch "$BASE_URL/releases/download/edge/latest.json" "$TMP/edge.json" ||
		die 1 "could not read the edge release manifest"
	_value=$(json_value "$TMP/edge.json" version)
	[ -n "$_value" ] || die 1 "the edge release manifest has no version field"
	printf '%s' "$_value"
}

decode_signature() {
	# Tauri publishes signatures as base64 of the minisign file; accept raw too.
	if head -n 1 "$1" | grep -q '^untrusted comment:'; then
		cp "$1" "$2"
	elif base64 -d < "$1" > "$2" 2>/dev/null && head -n 1 "$2" | grep -q '^untrusted comment:'; then
		:
	else
		die 1 "not a minisign signature: $1"
	fi
}

verify_minisign() {
	# $1 signed file, $2 signature file, $3 label for messages.
	if ! minisign -Vm "$1" -p "$TMP/ludex.pub" -x "$2" >"$TMP/minisign.log" 2>&1; then
		cat "$TMP/minisign.log" >&2
		die 1 "signature verification failed for $3"
	fi
	step "signature verified ($3)"
}

expected_sha() {
	# Print the SHA-256 SHA256SUMS pins for one asset name.
	awk -v want="$2" '{ name = $2; sub(/^\*/, "", name); if (name == want) { print $1; exit } }' "$1"
}

verify_asset_sha() {
	# $1 artifact path, $2 asset name.
	[ -f "$TMP/SHA256SUMS" ] || die 1 "SHA256SUMS was not downloaded"
	_want=$(expected_sha "$TMP/SHA256SUMS" "$2")
	[ -n "$_want" ] || die 1 "$2 is not listed in SHA256SUMS"
	_got=$(sha256sum "$1" | awk '{ print $1 }')
	[ "$_got" = "$_want" ] || die 1 "checksum mismatch for $2 (expected $_want, got $_got)"
	step "sha256 verified ($2)"
}

print_manual_verification() {
	# $1 tag, $2 version. Printed only when minisign is absent: the script
	# refuses a checksum-only install instead of degrading verification.
	_m_tag=$1
	_m_version=$2
	_m_url="$BASE_URL/releases/download/$_m_tag"
	if [ "$_m_tag" = "edge" ]; then
		cat >&2 <<EOF

minisign is not installed, so nothing was installed: a checksum-only install
is not a verified install. Verify the edge artifact yourself:

  curl -fsSLO "$_m_url/Ludex_${_m_version}_amd64.AppImage"
  curl -fsSLO "$_m_url/Ludex_${_m_version}_amd64.AppImage.sig"
  base64 -d Ludex_${_m_version}_amd64.AppImage.sig > Ludex.minisig
  minisign -Vm Ludex_${_m_version}_amd64.AppImage -P '$PUB_KEY' -x Ludex.minisig
  chmod +x Ludex_${_m_version}_amd64.AppImage
  ./Ludex_${_m_version}_amd64.AppImage

EOF
	else
		cat >&2 <<EOF

minisign is not installed, so nothing was installed: a checksum-only install
is not a verified install. Verify the AppImage yourself:

  curl -fsSLO "$_m_url/Ludex_${_m_version}_amd64.AppImage"
  curl -fsSLO "$_m_url/SHA256SUMS"
  curl -fsSLO "$_m_url/SHA256SUMS.sig"
  base64 -d SHA256SUMS.sig > SHA256SUMS.minisig
  minisign -Vm SHA256SUMS -P '$PUB_KEY' -x SHA256SUMS.minisig
  sha256sum -c --ignore-missing SHA256SUMS
  chmod +x Ludex_${_m_version}_amd64.AppImage
  ./Ludex_${_m_version}_amd64.AppImage

EOF
	fi
}

webkit_present() {
	if command -v ldconfig >/dev/null 2>&1 && ldconfig -p 2>/dev/null | grep -q 'libwebkit2gtk-4\.1\.so'; then
		return 0
	fi
	for _w_dir in /usr/lib /usr/lib64 /usr/lib/x86_64-linux-gnu /usr/local/lib; do
		[ -d "$_w_dir" ] || continue
		if command -v find >/dev/null 2>&1 &&
			[ -n "$(find "$_w_dir" -maxdepth 4 -name 'libwebkit2gtk-4.1.so*' -print -quit 2>/dev/null)" ]; then
			return 0
		fi
	done
	return 1
}

webkit_hint() {
	if command -v apt-get >/dev/null 2>&1; then
		printf 'sudo apt-get install -y libwebkit2gtk-4.1-0 libgtk-3-0'
	elif command -v pacman >/dev/null 2>&1; then
		printf 'sudo pacman -S --needed webkit2gtk-4.1 gtk3'
	elif command -v dnf >/dev/null 2>&1; then
		printf 'sudo dnf install -y webkit2gtk4.1 gtk3'
	else
		printf 'install webkit2gtk 4.1 and gtk3 with your package manager'
	fi
}

record_field() {
	# $1 record file, $2 top-level string field.
	sed -n "s/^[[:space:]]*\"$2\"[[:space:]]*:[[:space:]]*\"\([^\"]*\)\".*/\1/p" "$1" | head -n 1
}

record_files() {
	# One path per line from the "files" array of a record this script wrote.
	sed -n '/"files"[[:space:]]*:[[:space:]]*\[/,/\]/p' "$1" |
		sed -n 's/^[[:space:]]*"\(.*\)"[,]\{0,1\}[[:space:]]*$/\1/p'
}

write_record() {
	# $1 method (appimage|deb), $2 newline-separated file list.
	mkdir -p "$DATA_DIR"
	{
		printf '{\n'
		printf '  "version": "%s",\n' "$VERSION"
		printf '  "channel": "%s",\n' "$CHANNEL"
		printf '  "method": "%s",\n' "$1"
		printf '  "prefix": "%s",\n' "$PREFIX"
		printf '  "installedAt": "%s",\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
		if [ -n "$2" ]; then
			printf '  "files": [\n'
			printf '%s\n' "$2" | awk 'NF { print "    \"" $0 "\"," }' | sed '$ s/,$//'
			printf '  ]\n'
		else
			printf '  "files": []\n'
		fi
		printf '}\n'
	} >"$RECORD"
	step "recorded $RECORD"
}

write_icon() {
	# The app icon, embedded so the installer needs no second download and no
	# extraction from the artifact. Generated from
	# apps/desktop/src-tauri/icons/128x128@2x.png; scripts/test-install.sh
	# fails when the two drift apart.
	base64 -d >"$1" <<'ICON_BASE64'
iVBORw0KGgoAAAANSUhEUgAAAQAAAAEACAYAAABccqhmAAAACXBIWXMAAAsTAAALEwEAmpwYAAAD
8ElEQVR42u3UsQ2CUBCAYSppDN1rDMEVLHQHG92PsA7oHgxg4wBPo7GgUxMTee8r/gHucvcVZbmI
kvKssAQJAJIAIAkAkgAgCQCSACAJAJIAIAkAkgAgCQCSACAJAJIAIAkAkgAgCQCSACAJAJIAIAkA
kgAgCQCSACAJAJIAIAkAkgAgCQCSACAJAJIAIAkAkgAgCQCSACABwBIkAEgCgCQASAKAJABIAsBs
ul7Gv2jd1I5MAACABAAASAAAgAQAAEgAAIAEAABIAACABAAASAAAgAQAAEgAAIAEAABIAACABAAA
SAAAgAQAAEgAAIAEAABIAACABAAASAAAgAQAAEgAAIAAAAAACAAAAIAAAAAACAAAAIAAAAAACAAA
AIAAAAAACAAAAIAAAAAACAAAAIAAAAAACAAAAIAAAAAACAAAcGQCAAAkAABAAgAAJAAAQAIAACQA
AEACAAAkAABAAgAAJAAAQAIAACQAAEACAAAkAABAAgAAJAAAQAIAACQAAEACAAAkAABAAgAAJAAA
QAIAAAQAAABAAAAAAAQAAABAAAAAAAQAAABAAAAAAAQAAABAAAAAAAQAAABAAAAAAAQAAABAAAAA
AAQAAABAAACAIxMAACABAAASAAAgAQAAEgAAIAEAABIAACABAAASAAAgAQAAEgAAIAEAABIAACAB
AAASAAAgAQAAEgAAIAEAABIAACABAAASAAAgAQAAEgAAIAAAAAACAAAAIAAAAAACAAAAIAAAAAAC
AAAAIAAAAAACAAAAIAAAAAA/6NyFGE+rJBraAAAA6JNSef5XAACAAAAAAAgAAACAAAAAAAgAAACA
AAAAAAgAAACAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAmAEAu+3mgYCeVdUSAADIBwBN
Ox72AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA
AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAIBnAwAAACAAAAAAAgAAACAAAAAAAgAAACAAAAAAAgAA
ACAAAAAAAgAAACAAAAAAAgAAACAAAAAAAgAAACAAAAAAAgAAACAAAAAAAgAAACAAAAAAAgAApK8D
AAAEAAAAQDk2tCGZ5+/vswBAEgAkAUASACQBQBIAJAFAEgAkAUASACQBQBIAJAFAEgAkAUASACQB
QBIAJAFAEgAkAUASACQBQAKAJABIAoAkAEgCgCQASAKAJABIAoAkAEgCgCQASAKAJABIAoAkAEgC
gCQASAKAJABIAoAkAEgCgCQASAKAJABIAoAkAEgCgCQASAKApHe6Aabvzd/pUalVAAAAAElFTkSu
QmCC
ICON_BASE64
	chmod 0644 "$1"
}

write_desktop() {
	# $1 target, $2 binary path, $3 icon path.
	cat >"$1" <<EOF
[Desktop Entry]
Type=Application
Name=Ludex
GenericName=Game library
Comment=Self-hosted DRM-free game library
Exec="$2" %U
Icon=$3
Terminal=false
Categories=Game;
Keywords=games;library;
StartupWMClass=ludex
EOF
	chmod 0644 "$1"
}

install_appimage() {
	_bin_dir="$PREFIX/bin"
	_apps_dir="$PREFIX/share/applications"
	_icon_dir="$PREFIX/share/icons/hicolor/256x256/apps"
	mkdir -p "$_bin_dir" "$_apps_dir" "$_icon_dir"
	# New inode, then rename: a running instance keeps the old binary.
	cp "$TMP/$ASSET" "$_bin_dir/.ludex.new"
	chmod 0755 "$_bin_dir/.ludex.new"
	mv -f "$_bin_dir/.ludex.new" "$_bin_dir/ludex"
	step "installed $_bin_dir/ludex"
	write_icon "$_icon_dir/ludex.png"
	step "installed $_icon_dir/ludex.png"
	write_desktop "$_apps_dir/ludex.desktop" "$_bin_dir/ludex" "$_icon_dir/ludex.png"
	step "installed $_apps_dir/ludex.desktop"
	if command -v update-desktop-database >/dev/null 2>&1; then
		update-desktop-database "$_apps_dir" >/dev/null 2>&1 || true
	fi
	write_record appimage "$(printf '%s\n%s\n%s' "$_bin_dir/ludex" "$_apps_dir/ludex.desktop" "$_icon_dir/ludex.png")"
}

install_deb() {
	step "installing $DEB with dpkg"
	if ! as_root dpkg -i "$TMP/$DEB" >"$TMP/dpkg.log" 2>&1; then
		cat "$TMP/dpkg.log" >&2
		die 1 "dpkg -i failed; run 'sudo apt-get install -f' and retry"
	fi
	step "installed the ludex package"
	write_record deb ""
}

do_uninstall() {
	[ -f "$RECORD" ] || die 1 "no install recorded at $RECORD"
	_u_prefix=$(record_field "$RECORD" prefix)
	_u_method=$(record_field "$RECORD" method)
	_u_version=$(record_field "$RECORD" version)
	if [ -z "$_u_prefix" ] || [ -z "$_u_method" ]; then
		die 1 "the install record is incomplete: $RECORD"
	fi
	if [ "$_u_method" = "deb" ]; then
		command -v dpkg >/dev/null 2>&1 || die 1 "dpkg is required to remove the .deb install"
		step "removing the ludex package with dpkg"
		as_root dpkg -r ludex || die 1 "dpkg -r failed"
	else
		_u_files=$(record_files "$RECORD")
		_u_old_ifs=$IFS
		IFS='
'
		for _u_file in $_u_files; do
			case $_u_file in
				"${_u_prefix%/}"/*) ;;
				*)
					step "skipped (outside the recorded prefix): $_u_file"
					continue
					;;
			esac
			if [ -e "$_u_file" ]; then
				rm -f "$_u_file" || die 1 "could not remove $_u_file"
				step "removed $_u_file"
			fi
		done
		IFS=$_u_old_ifs
	fi
	if [ "$PURGE" = 1 ]; then
		case $DATA_DIR in
			*/ludex) rm -rf "$DATA_DIR" ;;
			*) die 1 "refusing to purge an unexpected data directory: $DATA_DIR" ;;
		esac
		step "removed user data $DATA_DIR"
	else
		rm -f "$RECORD"
		rmdir "$DATA_DIR" 2>/dev/null || true
		step "kept user data in $DATA_DIR (use --uninstall --purge to delete it)"
	fi
	step "uninstalled ludex $_u_version"
}

# --- arguments -------------------------------------------------------------

while [ $# -gt 0 ]; do
	case $1 in
		--version)
			[ $# -ge 2 ] || die 3 "usage error: --version needs a tag (see --help)"
			WANT_TAG=$(normalize_tag "$2")
			shift 2
			;;
		--version=*)
			WANT_TAG=$(normalize_tag "${1#*=}")
			shift
			;;
		--channel)
			[ $# -ge 2 ] || die 3 "usage error: --channel needs stable or edge (see --help)"
			CHANNEL=$2
			CHANNEL_GIVEN=1
			shift 2
			;;
		--channel=*)
			CHANNEL=${1#*=}
			CHANNEL_GIVEN=1
			shift
			;;
		--dir)
			[ $# -ge 2 ] || die 3 "usage error: --dir needs a path (see --help)"
			PREFIX=$2
			shift 2
			;;
		--dir=*)
			PREFIX=${1#*=}
			shift
			;;
		--deb)
			USE_DEB=1
			shift
			;;
		--uninstall)
			UNINSTALL=1
			shift
			;;
		--purge)
			PURGE=1
			shift
			;;
		--force)
			FORCE=1
			shift
			;;
		--help | -h)
			usage
			exit 0
			;;
		*) die 3 "usage error: unknown argument '$1' (see --help)" ;;
	esac
done

# --- validation ------------------------------------------------------------

case $CHANNEL in
	stable | edge) ;;
	*) die 3 "usage error: --channel must be stable or edge" ;;
esac
if [ -n "$WANT_TAG" ] && [ "$CHANNEL_GIVEN" = 1 ]; then
	die 3 "usage error: pass either --version or --channel, not both"
fi
if [ "$UNINSTALL" = 1 ]; then
	[ "$USE_DEB" = 0 ] || die 3 "usage error: --uninstall cannot be combined with --deb"
	[ -z "$WANT_TAG" ] || die 3 "usage error: --uninstall cannot be combined with --version"
	[ -z "$PREFIX" ] || die 3 "usage error: --uninstall cannot be combined with --dir"
	[ "$CHANNEL_GIVEN" = 0 ] || die 3 "usage error: --uninstall cannot be combined with --channel"
else
	[ "$PURGE" = 0 ] || die 3 "usage error: --purge only applies with --uninstall"
	if [ "$USE_DEB" = 1 ] && [ -n "$PREFIX" ]; then
		die 3 "usage error: --dir applies to the AppImage install only"
	fi
fi
if [ -n "$PREFIX" ]; then
	case $PREFIX in
		/*) ;;
		*) die 3 "usage error: --dir must be an absolute path" ;;
	esac
	case $PREFIX in
		*\"* | *\\*) die 3 "usage error: the install prefix cannot contain quotes or backslashes" ;;
	esac
fi

if [ "$UNINSTALL" = 1 ]; then
	need sed
	need awk
	do_uninstall
	exit 0
fi

# --- preflight -------------------------------------------------------------

_arch=${LUDEX_ARCH_OVERRIDE:-$(uname -m)}
case $_arch in
	x86_64 | amd64) ;;
	*)
		die 2 "unsupported architecture: $_arch — Ludex publishes x86_64 (amd64) Linux builds only"
		;;
esac

need curl
need base64
need sha256sum
need awk
need sed
need grep
need mktemp
need date

if [ -z "$PREFIX" ] && [ -f "$RECORD" ]; then
	PREFIX=$(record_field "$RECORD" prefix)
	[ -n "$PREFIX" ] || die 1 "the install record has no prefix: $RECORD"
	step "upgrading the install recorded in $RECORD (prefix $PREFIX)"
fi
[ -n "$PREFIX" ] || PREFIX="$HOME/.local"
if [ "$USE_DEB" = 1 ]; then
	# dpkg owns /usr/bin/ludex and the desktop entry.
	PREFIX="/usr"
fi

if [ "$USE_DEB" = 1 ]; then
	command -v dpkg >/dev/null 2>&1 || die 1 "the .deb path needs dpkg; rerun without --deb for the AppImage"
	command -v apt-get >/dev/null 2>&1 || die 1 "the .deb path needs apt; rerun without --deb for the AppImage"
	if [ "$(id -u)" -ne 0 ]; then
		command -v sudo >/dev/null 2>&1 || die 1 "the .deb path needs root or sudo"
	fi
fi

TMP=$(mktemp -d "${TMPDIR:-/tmp}/ludex-install.XXXXXX")
trap 'rm -rf "$TMP"' EXIT

if ! printf '%s' "$PUBKEY_B64" | base64 -d >"$TMP/ludex.pub"; then
	die 1 "the pinned minisign key is not valid base64"
fi
PUB_KEY=$(sed -n '2p' "$TMP/ludex.pub")
case $PUB_KEY in
	RW*) ;;
	*) die 1 "the pinned minisign key is malformed" ;;
esac

TAG=$(resolve_tag)
if [ "$TAG" = "edge" ]; then
	CHANNEL="edge"
	VERSION=$(resolve_edge_version)
else
	VERSION=${TAG#v}
fi
ASSET="Ludex_${VERSION}_amd64.AppImage"
DEB="Ludex_${VERSION}_amd64.deb"
TAG_URL="$BASE_URL/releases/download/$TAG"
step "channel $CHANNEL, tag $TAG, version $VERSION"

if ! command -v minisign >/dev/null 2>&1; then
	step "minisign is not installed: refusing a checksum-only install"
	print_manual_verification "$TAG" "$VERSION"
	die 1 "install minisign and rerun (apt: minisign, pacman: minisign, dnf: minisign)"
fi

if ! webkit_present; then
	step "runtime library libwebkit2gtk-4.1 is missing"
	step "install it with: $(webkit_hint)"
	[ "$FORCE" = 1 ] || die 1 "libwebkit2gtk-4.1 is missing; rerun with --force to install anyway"
	step "--force: continuing without libwebkit2gtk-4.1 (the app will not start)"
fi

if [ "$USE_DEB" = 1 ]; then
	WANT_ASSET="$DEB"
else
	WANT_ASSET="$ASSET"
fi

# --- verify and install ----------------------------------------------------

if [ "$CHANNEL" = "stable" ]; then
	# Verify the signed digest list before the artifact is downloaded, then the
	# artifact against it. A tampered SHA256SUMS.sig fails with nothing on disk.
	download "$TAG_URL/SHA256SUMS" "$TMP/SHA256SUMS" "SHA256SUMS"
	download "$TAG_URL/SHA256SUMS.sig" "$TMP/SHA256SUMS.sig" "SHA256SUMS.sig"
	decode_signature "$TMP/SHA256SUMS.sig" "$TMP/SHA256SUMS.minisig"
	verify_minisign "$TMP/SHA256SUMS" "$TMP/SHA256SUMS.minisig" "SHA256SUMS"
	download "$TAG_URL/$WANT_ASSET" "$TMP/$WANT_ASSET" "$WANT_ASSET"
	verify_asset_sha "$TMP/$WANT_ASSET" "$WANT_ASSET"
else
	# The edge prerelease publishes no SHA256SUMS; the per-asset minisign
	# signature covers the exact bytes instead (same key).
	download "$TAG_URL/$WANT_ASSET" "$TMP/$WANT_ASSET" "$WANT_ASSET"
	download "$TAG_URL/$WANT_ASSET.sig" "$TMP/$WANT_ASSET.sig" "$WANT_ASSET.sig"
	decode_signature "$TMP/$WANT_ASSET.sig" "$TMP/$WANT_ASSET.minisig"
	verify_minisign "$TMP/$WANT_ASSET" "$TMP/$WANT_ASSET.minisig" "$WANT_ASSET"
fi

if [ "$USE_DEB" = 1 ]; then
	install_deb
	BIN_PATH="/usr/bin/ludex"
	BIN_DIR="/usr/bin"
else
	install_appimage
	BIN_PATH="$PREFIX/bin/ludex"
	BIN_DIR="$PREFIX/bin"
fi

printf 'ludex %s installed at %s\n' "$VERSION" "$BIN_PATH"
case ":${PATH:-}:" in
	*":$BIN_DIR:"*) printf 'next: ludex\n' ;;
	*) printf 'next: %s (add %s to PATH)\n' "$BIN_PATH" "$BIN_DIR" ;;
esac
