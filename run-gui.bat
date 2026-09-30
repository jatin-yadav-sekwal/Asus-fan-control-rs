@echo off
setlocal
cd /d "%~dp0"

rem \\.\AsusSAIO only answers to NT AUTHORITY\SYSTEM. Everything else gets -1
rem from every HealthyTable_* export, so PsExec is mandatory - never fall back
rem to launching the GUI as a normal user.

net session >nul 2>&1
if %ERRORLEVEL% NEQ 0 (
    echo Requesting administrative privileges...
    powershell -Command "Start-Process -FilePath 'cmd.exe' -ArgumentList '/c \"\"%~f0\"\"' -Verb RunAs"
    exit /b
)

set "PSEXEC=%~dp0PsExec.exe"
if not exist "%PSEXEC%" set "PSEXEC=%~dp0bin\PsExec.exe"
if not exist "%PSEXEC%" (
    echo [ERROR] PsExec.exe not found next to this script or in bin\.
    echo         Launching as a normal user cannot reach the ASUS driver.
    pause
    exit /b 1
)

set "APP=%~dp0bin\asus-fan-app.exe"
if not exist "%APP%" (
    echo [ERROR] %APP% not found.
    pause
    exit /b 1
)

echo Launching as NT AUTHORITY\SYSTEM: "%APP%"
"%PSEXEC%" -accepteula -w "%~dp0bin" -i -s -d "%APP%"
if %ERRORLEVEL% NEQ 0 (
    echo [ERROR] PsExec failed with exit code %ERRORLEVEL%.
    pause
    exit /b %ERRORLEVEL%
)
exit /b 0
