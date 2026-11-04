#!/usr/bin/env bash
cd "$(dirname "$0")" || exit 1
export HOST=0.0.0.0
echo "Starting Mini Drive for LAN access (all interfaces)."
echo "LAN clients use http://<this-machine-ip>:8787/"
echo "Press Ctrl+C to stop. Use start.sh for localhost-only mode."
exec node server.js
