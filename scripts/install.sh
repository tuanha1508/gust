#!/usr/bin/env bash
# Install the latest Gust release binary into ~/.local/bin (or DEST).
#
#   curl -fsSL https://raw.githubusercontent.com/tuanha1508/gust/main/scripts/install.sh | bash
#
# Override install location: DEST=/usr/local/bin bash install.sh
set -euo pipefail

REPO="tuanha1508/gust"
DEST="${DEST:-$HOME/.local/bin}"

uname_s="$(uname -s)"
uname_m="$(uname -m)"

case "$uname_s/$uname_m" in
  Darwin/arm64|Darwin/aarch64) target="aarch64-apple-darwin" ;;
  Darwin/x86_64)               target="x86_64-apple-darwin" ;;
  Linux/x86_64|Linux/amd64)    target="x86_64-unknown-linux-gnu" ;;
  Linux/aarch64|Linux/arm64)   target="aarch64-unknown-linux-gnu" ;;
  *)
    echo "unsupported platform: $uname_s/$uname_m" >&2
    echo "build from source: cargo install --git https://github.com/${REPO}.git --locked gust" >&2
    exit 1
    ;;
esac

archive="gust-${target}.tar.gz"
url="https://github.com/${REPO}/releases/latest/download/${archive}"

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

echo "downloading ${url}"
curl -fsSL "$url" -o "${tmpdir}/${archive}"
tar -xzf "${tmpdir}/${archive}" -C "$tmpdir"

mkdir -p "$DEST"
install -m 755 "${tmpdir}/gust-${target}/gust" "${DEST}/gust"

echo "installed: ${DEST}/gust"
if ! command -v gust >/dev/null 2>&1; then
  echo "add to PATH:  export PATH=\"${DEST}:\$PATH\""
fi
"${DEST}/gust" --version
