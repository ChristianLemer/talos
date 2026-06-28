@echo off
REM start.bat - hidden orchestrator (launched by ..\Start.vbs with no window).
REM
REM The only thing that blocks the panel from opening is Node (the panel runs
REM on it) and the panel's own deps. Everything else is checked/installed FROM
REM INSIDE the panel, shown live.
REM
REM Fast path (Node + deps present): no window at all - just opens the panel.
REM Cold path (something missing): spawns a VISIBLE setup window (terminal only
REM if necessary), waits for it, then opens the panel.
REM
REM ASCII-only, no BOM (cmd.exe chokes on a BOM).

setlocal
set PORT=7682
set DIR=%~dp0

REM Self-heal: kill any process ALREADY listening on our port, surgically by
REM PID (never "all node"). Covers a stale server whose auto-shutdown failed,
REM and guarantees every launch starts fresh on the latest code. If the port
REM is free this is a no-op.
for /f "tokens=5" %%p in ('netstat -ano ^| findstr /r /c:":%PORT% .*LISTENING"') do (
  taskkill /F /PID %%p >nul 2>&1
)

REM Decide if cold setup is needed (Node missing, or panel deps missing).
set NEED=
where node >nul 2>&1 || set NEED=1
if not exist "%DIR%node_modules" set NEED=1

REM Run cold setup in a VISIBLE window so the user sees it work - once.
if defined NEED (
  start "Environment setup" /wait cmd /c ""%DIR%setup.bat""
  REM setup.bat installed Node, but its PATH refresh died with that child
  REM process. Re-read PATH HERE from the registry so THIS process can find
  REM the freshly installed node - otherwise the launch below fails silently
  REM and the panel never opens on first run.
  for /f "tokens=2,*" %%a in ('reg query "HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Environment" /v Path 2^>nul ^| findstr /i "Path"') do set "MACHPATH=%%b"
  for /f "tokens=2,*" %%a in ('reg query "HKCU\Environment" /v Path 2^>nul ^| findstr /i "Path"') do set "USERPATH=%%b"
  call set "PATH=%%MACHPATH%%;%%USERPATH%%"
)

REM Launch node HIDDEN (no console), then open the panel as an app window.
cscript //nologo "%DIR%node-hidden.vbs" "node ""%DIR%src\server.js"""

:wait
powershell -NoProfile -Command "try{ (New-Object Net.Sockets.TcpClient).Connect('127.0.0.1',%PORT%); exit 0 }catch{ exit 1 }" >nul 2>&1
if errorlevel 1 (
  timeout /t 1 /nobreak >nul
  goto wait
)

REM Edge ships with Windows; --app gives a clean window (no URL bar, no tabs).
start "" msedge --app=http://localhost:%PORT%

endlocal
