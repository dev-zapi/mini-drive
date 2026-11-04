@echo off
cd /d "%~dp0"
set HOST=0.0.0.0
echo Starting Mini Drive (Rust) for LAN access on all interfaces.
echo LAN clients use http://^<this-machine-ip^>:8787/
echo Press Ctrl+C to stop. Use start.bat for localhost-only mode.
if exist "target\release\mini-drive.exe" (
  "target\release\mini-drive.exe"
) else (
  where cargo >nul 2>nul
  if errorlevel 1 (
    echo [ERROR] Rust toolchain not found. Install rustup, then run: cargo build --release
    pause
    exit /b 1
  )
  cargo run --release
)
pause
