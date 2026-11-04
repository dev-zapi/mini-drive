@echo off
cd /d "%~dp0"
echo Starting Mini Drive on http://127.0.0.1:8787/
echo Press Ctrl+C to stop.
node server.js
pause
