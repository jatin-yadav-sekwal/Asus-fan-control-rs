@echo off
setlocal
cd /d "%~dp0"

net session >nul 2>&1
if %ERRORLEVEL% NEQ 0 (
    echo Requesting Administrator privileges for ASUS hardware access...
    powershell -Command "Start-Process -FilePath 'cmd.exe' -ArgumentList '/c \"\"%~f0\"\"' -Verb RunAs"
    exit /b
)

echo ========================================================
echo Running Asus Fan Control Rust Driver Status (SYSTEM)
echo ========================================================
if exist "%~dp0PsExec.exe" (
    "%~dp0PsExec.exe" -s "%~dp0bin\asus-driver-cli.exe" status
) else if exist "%~dp0bin\PsExec.exe" (
    "%~dp0bin\PsExec.exe" -s "%~dp0bin\asus-driver-cli.exe" status
) else (
    "%~dp0bin\asus-driver-cli.exe" status
)
echo.
pause
