@echo off
setlocal
cd /d "%~dp0"

rem The GUI elevates itself (UAC) and then starts its own SYSTEM helper via
rem Task Scheduler. PsExec is not used anywhere in this project.

set "APP=%~dp0bin\asus-fan-app.exe"
if not exist "%APP%" set "APP=%~dp0asus-fan-app.exe"
if not exist "%APP%" (
    echo [ERROR] asus-fan-app.exe not found next to this script or in bin\.
    echo         Run build.bat first, or extract the release ZIP.
    pause
    exit /b 1
)

start "" "%APP%"
exit /b 0
