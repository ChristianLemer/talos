@echo off
REM setup.bat - cold first-run, shown in a VISIBLE window on purpose.
REM
REM Installs the two things the panel needs before it can open: Node itself,
REM and the panel's npm deps (node-pty, ws, xterm). Called by start.bat ONLY
REM when one of them is missing. The user sees this work, once.
REM
REM ASCII-only, no BOM.

setlocal
set DIR=%~dp0

echo.
echo   Setting up your environment (first run only)
echo   =============================================
echo.

where node >nul 2>&1
if %errorlevel%==0 goto haveNode

echo   Installing Node.js...
echo.
winget install --id OpenJS.NodeJS -e --accept-source-agreements --accept-package-agreements

REM winget updated the registry but not THIS process's PATH. Re-read it so the
REM npm call below sees the freshly installed Node.
for /f "tokens=2,*" %%a in ('reg query "HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Environment" /v Path 2^>nul ^| findstr /i "Path"') do set "MACHPATH=%%b"
for /f "tokens=2,*" %%a in ('reg query "HKCU\Environment" /v Path 2^>nul ^| findstr /i "Path"') do set "USERPATH=%%b"
set "PATH=%MACHPATH%;%USERPATH%"

:haveNode

if exist "%DIR%node_modules" goto done
echo.
echo   Installing panel components...
echo.
pushd "%DIR%"
call npm install
popd

:done
echo.
echo   Done. Opening the panel...
timeout /t 1 /nobreak >nul
endlocal
