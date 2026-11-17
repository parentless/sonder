#!/usr/bin/env bash
# Keeps `target/` from eating the disk. Drops what rebuilds on its own
# (incremental caches, test screenshots) and, past a size limit, everything.
# Usage: scripts/trim-target.sh [limit-in-GB, default 15]
set -euo pipefail
cd "$(dirname "$0")/.."
limit_gb="${1:-15}"
[ -d target ] || exit 0
rm -rf target/debug/incremental target/ui-artifacts
used_gb=$(( $(du -sk target | cut -f1) / 1048576 ))
if [ "$used_gb" -gt "$limit_gb" ]; then
  echo "target/ is ${used_gb} GB (limit ${limit_gb}); running cargo clean"
  cargo clean
else
  echo "target/ is ${used_gb} GB (limit ${limit_gb}); fine"
fi
