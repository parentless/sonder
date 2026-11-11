#!/bin/sh
# Rebuilds the app icons (ico, icns and the sidebar mark) from scripts/build-icon.py.
set -eu
exec python3 "$(dirname "$0")/build-icon.py"
