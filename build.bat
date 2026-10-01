@echo off
setlocal
cd /d "%~dp0"

pushd "%~dp0asus-fan-control-rs"
cargo build --release
set "RC=%ERRORLEVEL%"
popd
if not "%RC%"=="0" exit /b %RC%

rem Prefer the target-dir that asus-fan-control-rs\.cargo\config.toml pins -
rem on CI that path is not under %USERPROFILE%, and guessing it wrong is how
rem the v0.1.0 release ended up shipping a ZIP with no executables in it.
set "TARGET="
if defined CARGO_TARGET_DIR if exist "%CARGO_TARGET_DIR%\release\asus-fan-app.exe" set "TARGET=%CARGO_TARGET_DIR%\release"
if not defined TARGET for /f "tokens=2 delims== " %%A in ('findstr /c:"target-dir" "%~dp0asus-fan-control-rs\.cargo\config.toml" 2^>nul') do (
    if not defined TARGETRAW set "TARGETRAW=%%~A"
)
if defined TARGETRAW (
    set "TARGETRAW=%TARGETRAW:/=\%"
    if exist "%TARGETRAW%\release\asus-fan-app.exe" set "TARGET=%TARGETRAW%\release"
)
if not defined TARGET if exist "%~dp0asus-fan-control-rs\target\release\asus-fan-app.exe" set "TARGET=%~dp0asus-fan-control-rs\target\release"
if not defined TARGET if exist "%USERPROFILE%\.cargo_target\asus_fan_control\release\asus-fan-app.exe" set "TARGET=%USERPROFILE%\.cargo_target\asus_fan_control\release"

if not defined TARGET (
    echo [ERROR] Built binary not found - run cargo build --release first.
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
