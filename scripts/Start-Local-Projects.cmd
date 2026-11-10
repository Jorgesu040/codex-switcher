@echo off
setlocal
rem The experimental behavior is enabled only for this process and its children.
set "CODEX_SWITCHER_REBIND_LOCAL_PROJECTS=1"
start "Codex Switcher Local Projects" "%~dp0codex-switcher-local-projects.exe"
