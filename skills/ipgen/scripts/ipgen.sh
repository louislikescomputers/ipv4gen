#!/usr/bin/env sh
# ipgen.sh — wrapper that locates the ipgen binary and forwards all
# arguments. Resolution order: $IPGEN_BIN, ./dist/ipgen, then $PATH.
#
# Usage:
#   IPGEN_BIN=/opt/ipgen/bin/ipgen ./ipgen.sh generate --preset public --count 5
#   ./ipgen.sh categories
#   ./ipgen.sh --help
#
# This wrapper is intentionally minimal: it never modifies arguments
# and never prints anything of its own (unless the binary cannot be
# found). The goal is to give agent skills a stable invocation surface
# regardless of how the underlying binary was installed on the host.

set -eu

# 1. Explicit override via environment.
if [ -n "${IPGEN_BIN:-}" ] && [ -x "$IPGEN_BIN" ]; then
    exec "$IPGEN_BIN" "$@"
fi

# 2. ./dist/ipgen relative to this script's directory.
SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
# Walk up two directories (skills/ipgen/scripts/ -> repo root).
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/../.." && pwd)
DIST_BIN="$REPO_ROOT/dist/ipgen"
if [ -x "$DIST_BIN" ]; then
    exec "$DIST_BIN" "$@"
fi

# 3. ./bin/<platform>/ipgen relative to repo root (compile.sh output).
for plat in linux darwin windows; do
    plat_bin="$REPO_ROOT/bin/$plat/ipgen"
    if [ -x "$plat_bin" ]; then
        exec "$plat_bin" "$@"
    fi
    # Windows .exe variant (will not be executable on Unix, but check
    # anyway in case the wrapper is invoked via WSL or Git Bash).
    plat_bin_exe="$REPO_ROOT/bin/$plat/ipgen.exe"
    if [ -x "$plat_bin_exe" ]; then
        exec "$plat_bin_exe" "$@"
    fi
done

# 4. Target-dir release build (cargo build --release output).
RELEASE_BIN="$REPO_ROOT/target/release/ipgen"
if [ -x "$RELEASE_BIN" ]; then
    exec "$RELEASE_BIN" "$@"
fi

# 5. PATH lookup as a last resort.
if command -v ipgen >/dev/null 2>&1; then
    exec ipgen "$@"
fi

# 6. Could not find the binary; print a helpful error.
cat >&2 <<EOF
ipgen.sh: could not locate the ipgen binary.

Tried, in order:
  1. \$IPGEN_BIN (not set or not executable)
  2. $DIST_BIN
  3. $REPO_ROOT/bin/<platform>/ipgen
  4. $RELEASE_BIN
  5. PATH lookup for 'ipgen'

To fix this, either:
  - Set \$IPGEN_BIN to the absolute path of the ipgen binary, or
  - Build with './compile.sh' from the repository root, or
  - Install ipgen on your PATH.
EOF
exit 127
