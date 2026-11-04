@echo off
cd /d "%~dp0"
echo Starting Mini Drive (Rust) on http://127.0.0.1:8787/
echo Press Ctrl+C to stop.
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
