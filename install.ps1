# One-line installer for kimi-statusline (Windows):
#
#   irm https://raw.githubusercontent.com/Demogorgon314/kimi-statusline/main/install.ps1 | iex
#
# Downloads the prebuilt binary into %USERPROFILE%\.local\bin (or
# $env:KIMI_STATUSLINE_BIN_DIR) and sets it as Kimi Code's status line command.
$ErrorActionPreference = 'Stop'
$repo = 'Demogorgon314/kimi-statusline'
$binDir = if ($env:KIMI_STATUSLINE_BIN_DIR) { $env:KIMI_STATUSLINE_BIN_DIR } else { Join-Path $HOME '.local\bin' }
$version = if ($env:KIMI_STATUSLINE_VERSION) { $env:KIMI_STATUSLINE_VERSION } else { 'latest' }
$arch = if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') { 'arm64' } else { 'x64' }
$asset = "kimi-statusline-windows-$arch.zip"
$url = if ($version -eq 'latest') {
  "https://github.com/$repo/releases/latest/download/$asset"
} else {
  "https://github.com/$repo/releases/download/$version/$asset"
}

$tmp = Join-Path $env:TEMP ("kimi-statusline-" + [guid]::NewGuid())
New-Item -ItemType Directory -Force $tmp | Out-Null
try {
  Write-Host "Downloading $url"
  Invoke-WebRequest -UseBasicParsing $url -OutFile (Join-Path $tmp $asset)
  Expand-Archive -Force (Join-Path $tmp $asset) $tmp
  New-Item -ItemType Directory -Force $binDir | Out-Null
  $exe = Join-Path $binDir 'kimi-statusline.exe'
  Move-Item -Force (Join-Path $tmp 'kimi-statusline.exe') $exe
  Write-Host "Installed $exe"
  & $exe install
  if ($LASTEXITCODE -ne 0) {
    Write-Host "Kimi Code already has another status line command; to replace it run:"
    Write-Host "  $exe install --force"
  }
  Write-Host "Run /reload-tui in Kimi Code to see it. Configure with: kimi-statusline config"
} finally {
  Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
