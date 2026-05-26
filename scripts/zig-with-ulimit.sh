#!/bin/sh
set -eu

# Zig 0.14+ links many objects in parallel. On macOS the default file descriptor
# limit is often too low for large cross-link steps, so raise it before invoking Zig.
ulimit -n 8192 >/dev/null 2>&1 || true

exec zig "$@"
