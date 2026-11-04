#!/usr/bin/env bash
cd "$(dirname "$0")" || exit 1
echo "Starting Mini Drive on http://127.0.0.1:8787/"
echo "Press Ctrl+C to stop."
exec node server.js
