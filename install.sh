#!/bin/sh
# One-line installer for kimi-statusline (macOS / Linux):
#
#   curl -fsSL https://raw.githubusercontent.com/Demogorgon314/kimi-statusline/main/install.sh | sh
#
# Downloads the prebuilt binary from GitHub releases into ~/.local/bin (or
# $KIMI_STATUSLINE_BIN_DIR) and sets it as Kimi Code's status line command.
# Pin a version with KIMI_STATUSLINE_VERSION=v0.1.0.
set -eu

repo="Demogorgon314/kimi-statusline"
bin_dir="${KIMI_STATUSLINE_BIN_DIR:-$HOME/.local/bin}"
version="${KIMI_STATUSLINE_VERSION:-latest}"

say() { printf '%s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

case "$(uname -m)" in
  arm64 | aarch64) arch=arm64 ;;
  x86_64 | amd64) arch=x64 ;;
  *) die "unsupported architecture: $(uname -m)" ;;
esac
case "$(uname -s)" in
  Darwin) os=darwin ;;
  Linux) os=linux ;;
  *) die "unsupported OS: $(uname -s) (on Windows use install.ps1)" ;;
esac

asset="kimi-statusline-$os-$arch.tar.gz"
if [ "$version" = latest ]; then
  url="https://github.com/$repo/releases/latest/download/$asset"
else
  url="https://github.com/$repo/releases/download/$version/$asset"
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
say "Downloading $url"
if command -v curl >/dev/null 2>&1; then
  curl -fsSL -o "$tmp/$asset" "$url" || die "download failed"
elif command -v wget >/dev/null 2>&1; then
  wget -q -O "$tmp/$asset" "$url" || die "download failed"
else
  die "need curl or wget"
fi
tar -xzf "$tmp/$asset" -C "$tmp"
mkdir -p "$bin_dir"
mv -f "$tmp/kimi-statusline" "$bin_dir/kimi-statusline"
chmod +x "$bin_dir/kimi-statusline"
say "Installed $bin_dir/kimi-statusline ($("$bin_dir/kimi-statusline" --version))"

if "$bin_dir/kimi-statusline" install; then
  :
else
  say "Kimi Code already has another status line command; to replace it run:"
  say "  $bin_dir/kimi-statusline install --force"
fi

case ":$PATH:" in
  *":$bin_dir:"*) ;;
  *) say "Note: $bin_dir is not on PATH; add it to run 'kimi-statusline config' directly." ;;
esac
say "Run /reload-tui in Kimi Code to see it. Configure with: kimi-statusline config"
