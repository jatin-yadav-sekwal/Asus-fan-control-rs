@echo off
setlocal
cd /d "%~dp0"

pushd "%~dp0asus-fan-control-rs"
cargo build --release
set "RC=%ERRORLEVEL%"
popd
if not "%RC%"=="0" exit /b %RC%

set "TARGET=%USERPROFILE%\.cargo_target\asus_fan_control\release"
if not exist "%TARGET%" set "TARGET=%~dp0asus-fan-control-rs\target\release"

if not exist "%TARGET%\asus-fan-app.exe" (
    echo [ERROR] Built binary not found in "%TARGET%".
    exit /b 1
)

if not exist "%~dp0bin" mkdir "%~dp0bin"

for %%F in (asus-fan-app.exe asus-driver-cli.exe hardware-probe.exe) do (
    if exist "%TARGET%\%%F" (
        copy /y "%TARGET%\%%F" "%~dp0bin\%%F" >nul
        if errorlevel 1 (
            echo [ERROR] Could not copy %%F - is the app running?
            exit /b 1
        )
    )
)

echo [OK] Binaries installed into "%~dp0bin".
exit /b 0
