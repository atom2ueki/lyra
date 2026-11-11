#!/bin/sh
# Install the latest Lyra binary into ~/.local/bin:
#   curl -fsSL https://raw.githubusercontent.com/atom2ueki/lyra/main/install.sh | sh
#
# Optional:
#   LYRA_VERSION=0.1.0          install a specific release tag
#   LYRA_INSTALL_DIR=/usr/local/bin
#   LYRA_REPO=owner/repo
#   LYRA_RELEASES_URL=...       override the release asset base URL
#   LYRA_GITHUB_TOKEN=...       authenticate against a private repository
set -eu

repo="${LYRA_REPO:-atom2ueki/lyra}"
dest="${LYRA_INSTALL_DIR:-$HOME/.local/bin}"
version="${LYRA_VERSION:-}"
token="${LYRA_GITHUB_TOKEN:-${GITHUB_TOKEN:-${GH_TOKEN:-}}}"

case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) target=aarch64-apple-darwin ;;
  Linux-x86_64) target=x86_64-unknown-linux-gnu ;;
  Linux-aarch64|Linux-arm64) target=aarch64-unknown-linux-gnu ;;
  *) echo "lyra installs on Apple Silicon macOS and Linux (x86_64, arm64) only" >&2; exit 1 ;;
esac

asset="lyra-$target.tar.gz"
base="${LYRA_RELEASES_URL:-}"
if [ -z "$base" ]; then
  if [ -n "$version" ]; then
    base="https://github.com/$repo/releases/download/$version"
  else
    base="https://github.com/$repo/releases/latest/download"
  fi
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

curl_fetch() {
  curl -fsSL --retry 3 --retry-all-errors --retry-delay 1 \
    --connect-timeout 10 --max-time 300 "$1" -o "$2"
}

fetch_private_asset() {
  if ! command -v gh >/dev/null 2>&1; then
    echo "private repository install needs gh; install gh or set LYRA_RELEASES_URL" >&2
    exit 1
  fi
  if [ -n "$version" ]; then
    GH_TOKEN="$token" gh release download "$version" --repo "$repo" --pattern "$1" --dir "$tmp" --clobber
  else
    GH_TOKEN="$token" gh release download --repo "$repo" --pattern "$1" --dir "$tmp" --clobber
  fi
}

if [ -n "$token" ] && [ -z "${LYRA_RELEASES_URL:-}" ]; then
  fetch_private_asset "$asset"
  fetch_private_asset "SHA256SUMS"
else
  curl_fetch "$base/$asset" "$tmp/$asset"
  curl_fetch "$base/SHA256SUMS" "$tmp/SHA256SUMS"
fi
expected=$(awk -v f="$asset" '$2 == f || $2 == "*" f { print $1; exit }' "$tmp/SHA256SUMS")
if [ -z "$expected" ]; then
  echo "no checksum for $asset in SHA256SUMS; not installing" >&2
  exit 1
fi
if command -v sha256sum >/dev/null 2>&1; then
  actual=$(sha256sum "$tmp/$asset" | awk '{ print $1 }')
else
  actual=$(shasum -a 256 "$tmp/$asset" | awk '{ print $1 }')
fi
if [ "$expected" != "$actual" ]; then
  echo "checksum mismatch for $asset; not installing" >&2
  exit 1
fi

tar -xzf "$tmp/$asset" -C "$tmp"
[ -f "$tmp/lyra" ] || { echo "archive did not contain lyra" >&2; exit 1; }

mkdir -p "$dest"
if command -v install >/dev/null 2>&1; then
  install -m 755 "$tmp/lyra" "$dest/lyra"
else
  cp "$tmp/lyra" "$dest/lyra"
  chmod 755 "$dest/lyra"
fi

echo "Installed $("$dest/lyra" --version) to $dest/lyra"
case ":$PATH:" in
  *":$dest:"*) ;;
  *)
    case "${SHELL:-}" in
      *zsh*) rc='~/.zshrc' ;;
      *) rc='~/.bashrc' ;;
    esac
    echo "Add it to your PATH: echo 'export PATH=\"$dest:\$PATH\"' >> $rc"
    ;;
esac
