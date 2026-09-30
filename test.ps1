# test.ps1 - build and smoke-test ipgen binaries from PowerShell
# (Windows primary; works on Linux/macOS pwsh as well).
# Prints generated IPv4 addresses to the command line and runs cargo test.
[CmdletBinding()]
param(
    [int]$Count = $(if ($env:IPGEN_TEST_COUNT) { [int]$env:IPGEN_TEST_COUNT } else { 20 })
)

$ErrorActionPreference = "Continue"
Set-Location -Path $PSScriptRoot

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Error "cargo not found. Install Rust from https://rustup.rs"
    exit 1
}

$isWin = $env:OS -eq "Windows_NT"
$ext = if ($isWin) { ".exe" } else { "" }

Write-Host "==> Building release binaries"
cargo build --release -p ipgen-cli -p ipgen-mcp
if ($LASTEXITCODE -ne 0) {
    Write-Error "build failed"
    exit 1
}

$BIN = Join-Path "target\release" "ipgen$ext"
$MCP = Join-Path "target\release" "ipgen-mcp$ext"
$script:Fail = 0

function Invoke-Case {
    param([string]$Desc, [scriptblock]$Block)
    Write-Host "`n=== $Desc ==="
    & $Block
    if ($LASTEXITCODE -eq 0) {
        Write-Host "[PASS] $Desc"
    } else {
        Write-Host "[FAIL] $Desc"
        $script:Fail++
    }
}

Invoke-Case "ipgen --version"     { & $BIN --version }
Invoke-Case "ipgen categories"    { & $BIN categories | Out-Null }
Invoke-Case "public permuted IPs" { & $BIN generate --preset public  --order permuted --seed 42 --count $Count }
Invoke-Case "public sequential"   { & $BIN generate --preset public  --order sequential --count $Count }
Invoke-Case "private permuted IPs"{ & $BIN generate --preset private --order permuted --seed 7 --count $Count }
Invoke-Case "all preset IPs"      { & $BIN generate --preset all     --order permuted --seed 1 --count $Count }
Invoke-Case "custom include/exclude" {
    & $BIN generate --include 10.0.0.0/8 --exclude 10.16.0.0/12 --order sequential --count $Count
}
Invoke-Case "jsonl format"        { & $BIN generate --preset public --format jsonl --count 5 }
Invoke-Case "csv format"          { & $BIN generate --preset public --format csv --count 5 }
Invoke-Case "blocks /24 CIDRs"    { & $BIN generate --preset public --order blocks --prefix-len 24 --count 5 }
Invoke-Case "shard 1 of 3"        { & $BIN generate --preset public --shard 1/3 --count $Count }

# --- checkpoint / resume -------------------------------------------------
Write-Host "`n=== checkpoint + resume ==="
$state = Join-Path ([System.IO.Path]::GetTempPath()) "ipgen.state.json"
& $BIN generate --preset public --count 10 --state $state --checkpoint-every 1 | Out-Null
if ($LASTEXITCODE -eq 0) { & $BIN resume --state $state --count 10 }
if ($LASTEXITCODE -eq 0) { & $BIN status --state $state }
if ($LASTEXITCODE -eq 0) {
    Write-Host "[PASS] checkpoint/resume round-trip"
} else {
    Write-Host "[FAIL] checkpoint/resume round-trip"
    $script:Fail++
}
Remove-Item $state -ErrorAction SilentlyContinue

# --- sanity check: emitted lines are valid IPv4 addresses ----------------
Write-Host "`n=== validating generated addresses ==="
$lines = & $BIN generate --preset all --order permuted --seed 99 --count 200
$bad = @($lines | Where-Object { $_ -notmatch '^(\d{1,3}\.){3}\d{1,3}$' })
if ($bad.Count -eq 0 -and $lines.Count -ge 200) {
    Write-Host "[PASS] all $($lines.Count) sampled outputs are valid IPv4 addresses"
} else {
    Write-Host "[FAIL] non-IPv4 output detected ($($bad.Count) bad lines)"
    $script:Fail++
}

# --- MCP server smoke test -----------------------------------------------
# The MCP server speaks stdio JSON-RPC; feed it an initialize request and
# verify it responds (it would otherwise block waiting for input forever).
Write-Host "`n=== ipgen-mcp stdio handshake ==="
$initReq = '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test.ps1","version":"0"}}}'
$mcpJob = Start-Job -ScriptBlock {
    param($bin, $req)
    $req | & $bin 2>$null | Select-Object -First 1
} -ArgumentList $MCP, $initReq
$mcpLine = if (Wait-Job $mcpJob -Timeout 15) { Receive-Job $mcpJob } else { $null }
Remove-Job $mcpJob -Force
if ($mcpLine -match '"result"') {
    Write-Host "[PASS] ipgen-mcp responded to initialize over stdio"
} else {
    Write-Host "[FAIL] ipgen-mcp did not respond over stdio"
    $script:Fail++
}

# --- full unit/integration test suite ------------------------------------
Write-Host "`n=== cargo test ==="
cargo test --release --workspace
if ($LASTEXITCODE -eq 0) {
    Write-Host "[PASS] cargo test"
} else {
    Write-Host "[FAIL] cargo test"
    $script:Fail++
}

Write-Host "`n========================================="
if ($script:Fail -eq 0) {
    Write-Host "ALL TESTS PASSED"
    exit 0
} else {
    Write-Host "$script:Fail TEST(S) FAILED"
    exit 1
}
