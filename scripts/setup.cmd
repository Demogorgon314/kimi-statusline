@echo off
rem kimi-statusline plugin: SessionStart hook for Windows when `sh` is not on
rem PATH (hooks run under cmd.exe). Fetches the release binary that matches
rem kimi.plugin.json's version and wires it into tui.toml. Silent; never fails.
setlocal
set "ROOT=%KIMI_PLUGIN_ROOT%"
if "%ROOT%"=="" set "ROOT=%~dp0.."
set "BIN=%ROOT%\bin\kimi-statusline.exe"
if not exist "%BIN%" (
  powershell -NoProfile -ExecutionPolicy Bypass -Command ^
    "$ErrorActionPreference='Stop';" ^
    "$v=(Get-Content -Raw '%ROOT%\kimi.plugin.json' | ConvertFrom-Json).version;" ^
    "$a=if($env:PROCESSOR_ARCHITECTURE -eq 'ARM64'){'arm64'}else{'x64'};" ^
    "$u=\"https://github.com/Demogorgon314/kimi-statusline/releases/download/v$v/kimi-statusline-windows-$a.zip\";" ^
    "$t=Join-Path $env:TEMP ('ksl-'+[guid]::NewGuid());" ^
    "New-Item -ItemType Directory -Force $t | Out-Null;" ^
    "Invoke-WebRequest -UseBasicParsing -TimeoutSec 15 $u -OutFile \"$t\ksl.zip\";" ^
    "Expand-Archive -Force \"$t\ksl.zip\" $t;" ^
    "New-Item -ItemType Directory -Force '%ROOT%\bin' | Out-Null;" ^
    "Move-Item -Force \"$t\kimi-statusline.exe\" '%BIN%';" ^
    "Remove-Item -Recurse -Force $t" >nul 2>&1
)
if exist "%BIN%" "%BIN%" install --plugin >nul 2>&1
exit /b 0
