@echo off
setlocal
cd /d "%~dp0"

rem The CLI creates its own SYSTEM helper (Task Scheduler + named pipe), so it
rem only needs to be elevated - never run under PsExec.

net session >nul 2>&1
if %ERRORLEVEL% NEQ 0 (
    echo Requesting administrative privileges...
    powershell -Command "Start-Process -FilePath 'cmd.exe' -ArgumentList '/c \"\"%~f0\"\" %*' -Verb RunAs"
    exit /b
)

set "CLI=%~dp0bin\asus-driver-cli.exe"
if not exist "%CLI%" set "CLI=%~dp0asus-driver-cli.exe"
if not exist "%CLI%" (
    echo [ERROR] asus-driver-cli.exe not found next to this script or in bin\.
    echo         Run build.bat first, or extract the release ZIP.
    pause
    exit /b 1
)

echo ========================================================
echo Asus Fan Control - driver test harness
echo ========================================================
"%CLI%" %*
echo.
pause
