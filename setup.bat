@echo off
REM Astro-Telemetry dependency checker/installer.
REM Double-click it, or run from a terminal:
REM   setup.bat                         check + install everything
REM   setup.bat -Role computer1 -Build  Computer 1 only, then compile
REM   setup.bat -CheckOnly              report only
setlocal EnableDelayedExpansion
cd /d "%~dp0"
set "SELF=%~nx0"
set "CMDLINE=%cmdcmdline:"=%"

echo.
echo ============================================================
echo  Astro-Telemetry setup     (a log is saved to setup.log)
echo ============================================================

where powershell >nul 2>nul
if errorlevel 1 (
    echo PowerShell was not found on this PC. It ships with Windows 10/11, so check your PATH.
    set "RC=9009"
    goto :finish
)

powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0setup.ps1" %*
set "RC=%ERRORLEVEL%"

:finish
echo.
if "%RC%"=="0" (
    echo Setup finished OK.
) else (
    echo Setup finished with problems. Read the red lines above.
    echo If you ask for help, send the file setup.log from this folder.
)

REM Keep the window open only when launched by double-click (cmd /c "...setup.bat").
if /i not "!CMDLINE:%SELF%=!"=="!CMDLINE!" pause
exit /b %RC%
