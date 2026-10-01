#!/bin/sh
# kimi-statusline plugin: SessionStart hook.
#
# Runs with cwd = plugin root. Finds or fetches the kimi-statusline binary
# and points tui.toml's [status_line].command at it. Prints nothing: hook
# stdout may be appended to model context. Never fails the session.
#
# Binary lookup order:
#   1. bin/kimi-statusline in the plugin root (fetched earlier, or shipped)
#   2. the prebuilt binary for this platform from the GitHub release that
#      matches kimi.plugin.json's version, checked against its SHA256SUMS
#   3. kimi-statusline on PATH (cargo install / install.sh)
#   4. build from source with cargo, in the background (hooks time out long
#      before a release build finishes); the next session picks it up

repo="Demogorgon314/kimi-statusline"
root="${KIMI_PLUGIN_ROOT:-$(cd "$(dirname "$0")/.." && pwd)}"
home="${KIMI_CODE_HOME:-$HOME/.kimi-code}"
log="$home/kimi-statusline-debug.log"

debug() {
  if [ -n "${KIMI_STATUSLINE_DEBUG:-}" ] || [ -e "$home/kimi-statusline-debug" ]; then
    echo "$(date '+%Y-%m-%d %H:%M:%S') setup: $*" >>"$log" 2>/dev/null
  fi
}

platform() {
  os="$(uname -s 2>/dev/null)"
  arch="$(uname -m 2>/dev/null)"
  case "$arch" in
    arm64 | aarch64) arch=arm64 ;;
    x86_64 | amd64) arch=x64 ;;
    *) return 1 ;;
  esac
  case "$os" in
    Darwin) echo "darwin-$arch" ;;
    Linux) echo "linux-$arch" ;;
    MINGW* | MSYS* | CYGWIN*) echo "windows-$arch" ;;
    *) return 1 ;;
  esac
}

exe=""
case "$(uname -s 2>/dev/null)" in
  MINGW* | MSYS* | CYGWIN*) exe=".exe" ;;
esac
bin="$root/bin/kimi-statusline$exe"

get() {
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL --max-time 15 -o "$2" "$1"
  elif command -v wget >/dev/null 2>&1; then
    wget -q -T 15 -O "$2" "$1"
  else
    debug "neither curl nor wget"
    return 1
  fi
}

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | cut -d' ' -f1
  elif command -v certutil >/dev/null 2>&1; then
    certutil -hashfile "$1" SHA256 | sed -n 2p | tr -d ' \r' | tr 'A-F' 'a-f'
  fi
}

fetch() {
  plat="$(platform)" || { debug "unsupported platform $(uname -sm)"; return 1; }
  version="$(sed -n 's/.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$root/kimi.plugin.json" | head -n 1)"
  [ -n "$version" ] || return 1
  if [ -n "$exe" ]; then asset="kimi-statusline-$plat.zip"; else asset="kimi-statusline-$plat.tar.gz"; fi
  base="https://github.com/$repo/releases/download/v$version"
  tmp="$root/bin/.download"
  rm -rf "$tmp" && mkdir -p "$tmp" || return 1
  debug "downloading $base/$asset"
  get "$base/$asset" "$tmp/$asset" && get "$base/SHA256SUMS" "$tmp/SHA256SUMS" ||
    { debug "download failed"; return 1; }
  expected="$(awk -v a="$asset" '$2 == a || $2 == "*" a { print $1 }' "$tmp/SHA256SUMS")"
  actual="$(sha256 "$tmp/$asset")"
  if [ -z "$expected" ] || [ "$actual" != "$expected" ]; then
    debug "checksum mismatch for $asset: expected '$expected', got '$actual'"
    rm -rf "$tmp"
    return 1
  fi
  if [ -n "$exe" ]; then
    (cd "$tmp" && unzip -q "$asset") 2>/dev/null ||
      powershell -NoProfile -Command "Expand-Archive -Force '$tmp/$asset' '$tmp'" >/dev/null 2>&1 ||
      return 1
  else
    tar -xzf "$tmp/$asset" -C "$tmp" || return 1
  fi
  mv -f "$tmp/kimi-statusline$exe" "$bin" && chmod +x "$bin" && rm -rf "$tmp"
}

if [ ! -x "$bin" ] && ! fetch; then
  if command -v kimi-statusline >/dev/null 2>&1; then
    bin="$(command -v kimi-statusline)"
  elif command -v cargo >/dev/null 2>&1 && [ -f "$root/Cargo.toml" ]; then
    lock="$root/.building"
    if [ ! -e "$lock" ]; then
      debug "building from source"
      : >"$lock"
      (
        cargo build --release --manifest-path "$root/Cargo.toml" --target-dir "$root/target" \
          >>"$log" 2>&1 &&
          mkdir -p "$root/bin" &&
          cp "$root/target/release/kimi-statusline$exe" "$bin" &&
          "$bin" install --plugin
        rm -f "$lock"
      ) </dev/null >/dev/null 2>&1 &
    fi
    exit 0
  else
    debug "no binary available"
    exit 0
  fi
fi

"$bin" install --plugin >/dev/null 2>&1 || debug "install --plugin failed"
exit 0
