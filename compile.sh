#!/usr/bin/env bash
# compile.sh — build ipgen release binaries on Linux and macOS.
#
# Produces:
#   bin/linux/ipgen,  bin/linux/ipgen-mcp   (when run on Linux)
#   bin/darwin/ipgen, bin/darwin/ipgen-mcp  (when run on macOS)
#
# Usage: ./compile.sh [--all-targets]
#   --all-targets : also cross-compile for the other OS/arch when the
#                   required Rust targets and linkers are installed.
set -euo pipefail

cd "$(dirname "$0")"

if ! command -v cargo >/dev/null 2>&1; then
    echo "error: cargo not found. Install Rust from https://rustup.rs" >&2
    exit 1
fi

HOST_OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
HOST_ARCH="$(uname -m)"
case "$HOST_ARCH" in
    x86_64|amd64)  HOST_ARCH="x86_64" ;;
    arm64|aarch64) HOST_ARCH="aarch64" ;;
esac
case "$HOST_OS" in
    linux*)  OUTDIR="bin/linux";  TARGET_TRIPLE="${HOST_ARCH}-unknown-linux-gnu" ;;
    darwin*) OUTDIR="bin/darwin"; TARGET_TRIPLE="${HOST_ARCH}-apple-darwin" ;;
    *)       OUTDIR="bin/${HOST_OS}"; TARGET_TRIPLE="" ;;
esac

echo "==> Building ipgen (release) for ${HOST_OS}/${HOST_ARCH}"
cargo build --release -p ipgen-cli -p ipgen-mcp

mkdir -p "$OUTDIR"

copy_bin() {
    local name="$1"
    local src="target/release/$name"
    if [[ -f "$src" ]]; then
        cp -f "$src" "$OUTDIR/$name"
        chmod +x "$OUTDIR/$name" || true
        echo "    -> $OUTDIR/$name"
    else
        echo "error: expected binary not found: $src" >&2
        exit 1
    fi
}

copy_bin ipgen
copy_bin ipgen-mcp

# Optional: cross-compile for the other desktop platforms.
if [[ "${1:-}" == "--all-targets" ]]; then
    declare -a EXTRA_TARGETS=()
    case "$HOST_OS" in
        linux)  EXTRA_TARGETS+=("${HOST_ARCH}-apple-darwin") ;;
        darwin) EXTRA_TARGETS+=("${HOST_ARCH}-unknown-linux-gnu") ;;
    esac
    # Windows is always an extra target from any host.
    EXTRA_TARGETS+=("x86_64-pc-windows-gnu")

    for t in "${EXTRA_TARGETS[@]}"; do
        if rustc --print target-list | grep -qx "$t"; then
            echo "==> Cross-compiling for $t (best effort)"
            if rustup target add "$t" >/dev/null 2>&1 && \
               cargo build --release -p ipgen-cli -p ipgen-mcp --target "$t"; then
                ospart="${t##*-}"
                case "$t" in
                    *-linux-gnu)         d="bin/linux" ;;
                    *-apple-darwin)      d="bin/darwin" ;;
                    *-pc-windows-gnu)    d="bin/windows" ;;
                    *)                   d="bin/$ospart" ;;
                esac
                mkdir -p "$d"
                ext=""; [[ "$t" == *windows* ]] && ext=".exe"
                cp -f "target/$t/release/ipgen${ext}" "$d/ipgen${ext}"
                cp -f "target/$t/release/ipgen-mcp${ext}" "$d/ipgen-mcp${ext}"
                echo "    -> $d/ipgen${ext}, $d/ipgen-mcp${ext}"
            else
                echo "    (skipped: missing linker/toolchain for $t)" >&2
            fi
        fi
    done
fi

echo "==> Done. Binaries are in $OUTDIR/"
