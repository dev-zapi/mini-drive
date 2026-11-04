#!/usr/bin/env bash
cd "$(dirname "$0")" || exit 1
export HOST=0.0.0.0
echo "Starting Mini Drive (Rust) for LAN access on all interfaces."
echo "LAN clients use http://<this-machine-ip>:8787/"
if [ -x "./target/release/mini-drive.exe" ]; then
  exec ./target/release/mini-drive.exe
elif [ -x "./target/release/mini-drive" ]; then
  exec ./target/release/mini-drive
fi
command -v cargo >/dev/null 2>&1 || { echo "[ERROR] Rust toolchain not found. Install rustup first."; exit 1; }
exec cargo run --release
