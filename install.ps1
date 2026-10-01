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
$base = if ($version -eq 'latest') {
  "https://github.com/$repo/releases/latest/download"
} else {
  "https://github.com/$repo/releases/download/$version"
}

$tmp = Join-Path $env:TEMP ("kimi-statusline-" + [guid]::NewGuid())
New-Item -ItemType Directory -Force $tmp | Out-Null
try {
  Write-Host "Downloading $base/$asset"
  $archive = Join-Path $tmp $asset
  Invoke-WebRequest -UseBasicParsing "$base/$asset" -OutFile $archive
  $sums = (Invoke-WebRequest -UseBasicParsing "$base/SHA256SUMS").Content
  if ($sums -is [byte[]]) { $sums = [Text.Encoding]::UTF8.GetString($sums) }
  $expected = $sums -split "`n" | ForEach-Object {
    $f = $_.Trim() -split '\s+'
    if ($f.Count -ge 2 -and $f[1].TrimStart('*') -eq $asset) { $f[0] }
  } | Select-Object -First 1
  if (-not $expected) { throw "$asset is not listed in SHA256SUMS" }
  $actual = (Get-FileHash -Algorithm SHA256 $archive).Hash
  if ($actual -ne $expected) { throw "checksum mismatch for $asset (expected $expected, got $actual)" }
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
