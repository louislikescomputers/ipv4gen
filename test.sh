#!/usr/bin/env bash
# test.sh — build (if needed) and smoke-test the ipgen binaries by printing
# IPv4 addresses to the command line. Runs on Linux and macOS; on Windows use
# test.bat or test.ps1.
set -euo pipefail

cd "$(dirname "$0")"

if ! command -v cargo >/dev/null 2>&1; then
    echo "error: cargo not found. Install Rust from https://rustup.rs" >&2
    exit 1
fi

echo "==> Building release binaries"
cargo build --release -p ipgen-cli -p ipgen-mcp

BIN="target/release/ipgen"
MCP="target/release/ipgen-mcp"
COUNT="${IPGEN_TEST_COUNT:-20}"
FAIL=0

banner() { printf '\n=== %s ===\n' "$*"; }

run_case() {
    local desc="$1"; shift
    banner "$desc"
    if "$@"; then
        echo "[PASS] $desc"
    else
        echo "[FAIL] $desc" >&2
        FAIL=$((FAIL + 1))
    fi
}

# --- version / help -----------------------------------------------------
run_case "ipgen --version"   "$BIN" --version
run_case "ipgen --help"      "$BIN" --help > /dev/null

# --- categories table ---------------------------------------------------
run_case "ipgen categories"  "$BIN" categories

# --- print IP addresses to the command line -----------------------------
run_case "generate 20 public IPs (permuted)" \
    "$BIN" generate --preset public --order permuted --seed 42 --count "$COUNT"

run_case "generate 20 public IPs (sequential)" \
    "$BIN" generate --preset public --order sequential --count "$COUNT"

run_case "generate 20 private IPs" \
    "$BIN" generate --preset private --order permuted --seed 7 --count "$COUNT"

run_case "generate 20 'all' IPs" \
    "$BIN" generate --preset all --order permuted --seed 1 --count "$COUNT"

run_case "generate with custom include/exclude" \
    "$BIN" generate --include 10.0.0.0/8 --exclude 10.16.0.0/12 \
                    --order sequential --count "$COUNT"

run_case "generate JSONL output" \
    "$BIN" generate --preset public --format jsonl --count 5

run_case "generate CSV output" \
    "$BIN" generate --preset public --format csv --count 5

run_case "generate blocks (/24 CIDRs)" \
    "$BIN" generate --preset public --order blocks --prefix-len 24 --count 5

run_case "shard 1 of 3" \
    "$BIN" generate --preset public --shard 1/3 --count "$COUNT"

# --- checkpoint / resume -------------------------------------------------
TMPSTATE="$(mktemp)"
trap 'rm -f "$TMPSTATE"' EXIT
banner "checkpoint + resume"
if "$BIN" generate --preset public --count 10 --state "$TMPSTATE" --checkpoint-every 1 > /dev/null \
   && "$BIN" resume --state "$TMPSTATE" --count 10 \
   && "$BIN" status --state "$TMPSTATE"; then
    echo "[PASS] checkpoint/resume round-trip"
else
    echo "[FAIL] checkpoint/resume round-trip" >&2
    FAIL=$((FAIL + 1))
fi

# --- sanity check: every emitted line is a valid IPv4 address ------------
banner "validating generated addresses"
if "$BIN" generate --preset all --order permuted --seed 99 --count 200 \
   | grep -Eqv '^([0-9]{1,3}\.){3}[0-9]{1,3}$'; then
    echo "[FAIL] non-IPv4 output detected" >&2
    FAIL=$((FAIL + 1))
else
    echo "[PASS] all 200 sampled outputs are valid IPv4 addresses"
fi

# --- MCP server binary smoke test ---------------------------------------
# The MCP server speaks stdio JSON-RPC and waits for input, so we feed it an
# initialize request (and EOF) with a hard timeout to prove it starts up.
banner "ipgen-mcp stdio handshake"
mcp_out="$(printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test.sh","version":"0"}}}' \
          | timeout 10 "$MCP" 2>/dev/null | head -c 400 || true)"
if printf '%s' "$mcp_out" | grep -q '"result"'; then
    echo "[PASS] ipgen-mcp responded to initialize over stdio"
else
    echo "[FAIL] ipgen-mcp did not respond over stdio" >&2
    FAIL=$((FAIL + 1))
fi

# --- full unit/integration test suite ------------------------------------
banner "cargo test"
if cargo test --release --workspace; then
    echo "[PASS] cargo test"
else
    echo "[FAIL] cargo test" >&2
    FAIL=$((FAIL + 1))
fi

printf '\n=========================================\n'
if [ "$FAIL" -eq 0 ]; then
    echo "ALL TESTS PASSED"
else
    echo "$FAIL TEST(S) FAILED"
    exit 1
fi
