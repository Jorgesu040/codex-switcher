@echo off
setlocal
echo Close Codex completely before applying this experiment.
echo This changes local project configuration for the account in auth.json.
echo.
"%~dp0codex-local-projects.exe"
if errorlevel 1 goto done
"%~dp0codex-local-projects.exe" --apply
:done
echo.
pause
