@echo off
cd /d "%~dp0"
set HOST=0.0.0.0
echo Starting Mini Drive for LAN access (all interfaces).
echo LAN clients use http://<this-machine-ip>:8787/
echo Press Ctrl+C to stop. Use start.bat for localhost-only mode.
node server.js
pause
