@echo off
rem kimi-statusline plugin: SessionStart hook for Windows when `sh` is not on
rem PATH (hooks run under cmd.exe). Fetches the release binary that matches
rem kimi.plugin.json's version, checks it against the release's SHA256SUMS,
rem and wires it into tui.toml. Silent; never fails.
setlocal
set "ROOT=%KIMI_PLUGIN_ROOT%"
if "%ROOT%"=="" set "ROOT=%~dp0.."
set "BIN=%ROOT%\bin\kimi-statusline.exe"
if not exist "%BIN%" (
  powershell -NoProfile -ExecutionPolicy Bypass -Command ^
    "$ErrorActionPreference='Stop';" ^
    "$v=(Get-Content -Raw '%ROOT%\kimi.plugin.json' | ConvertFrom-Json).version;" ^
    "$a=if($env:PROCESSOR_ARCHITECTURE -eq 'ARM64'){'arm64'}else{'x64'};" ^
    "$n=\"kimi-statusline-windows-$a.zip\";" ^
    "$b=\"https://github.com/Demogorgon314/kimi-statusline/releases/download/v$v\";" ^
    "$t=Join-Path $env:TEMP ('ksl-'+[guid]::NewGuid());" ^
    "New-Item -ItemType Directory -Force $t | Out-Null;" ^
    "Invoke-WebRequest -UseBasicParsing -TimeoutSec 15 \"$b/$n\" -OutFile \"$t\ksl.zip\";" ^
    "Invoke-WebRequest -UseBasicParsing -TimeoutSec 15 \"$b/SHA256SUMS\" -OutFile \"$t\sums\";" ^
    "$e=Get-Content \"$t\sums\" | ForEach-Object { $f=$_.Trim() -split '\s+'; if($f.Count -ge 2 -and $f[1].TrimStart('*') -eq $n){$f[0]} } | Select-Object -First 1;" ^
    "if(-not $e -or (Get-FileHash -Algorithm SHA256 \"$t\ksl.zip\").Hash -ne $e){Remove-Item -Recurse -Force $t; throw 'checksum mismatch'};" ^
    "Expand-Archive -Force \"$t\ksl.zip\" $t;" ^
    "New-Item -ItemType Directory -Force '%ROOT%\bin' | Out-Null;" ^
    "Move-Item -Force \"$t\kimi-statusline.exe\" '%BIN%';" ^
    "Remove-Item -Recurse -Force $t" >nul 2>&1
)
if exist "%BIN%" "%BIN%" install --plugin >nul 2>&1
exit /b 0
