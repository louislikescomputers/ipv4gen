# compile.ps1 - build ipgen release binaries on Windows (PowerShell)
# Produces: bin\windows\ipgen.exe and bin\windows\ipgen-mcp.exe
# Works on Linux/macOS PowerShell too (outputs to bin/<os>/).
[CmdletBinding()]
param(
    [switch]$AllTargets
)

$ErrorActionPreference = "Stop"
Set-Location -Path $PSScriptRoot

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Error "cargo not found. Install Rust from https://rustup.rs"
    exit 1
}

$isWin = $env:OS -eq "Windows_NT"
$outDir = if ($isWin) { "bin\windows" } else { "bin/linux" }
$ext = if ($isWin) { ".exe" } else { "" }

Write-Host "==> Building ipgen (release)"
cargo build --release -p ipgen-cli -p ipgen-mcp
if ($LASTEXITCODE -ne 0) {
    Write-Error "cargo build failed"
    exit 1
}

New-Item -ItemType Directory -Force -Path $outDir | Out-Null

foreach ($name in @("ipgen", "ipgen-mcp")) {
    $src = Join-Path "target\release" "$name$ext"
    if (-not (Test-Path $src)) {
        Write-Error "expected binary not found: $src"
        exit 1
    }
    Copy-Item -Force $src (Join-Path $outDir "$name$ext")
    Write-Host "    -> $outDir/$name$ext"
}

if ($AllTargets -and $isWin) {
    # Best-effort cross-compiles from Windows to linux/darwin when toolchains exist.
    foreach ($t in @("x86_64-unknown-linux-gnu", "x86_64-apple-darwin")) {
        Write-Host "==> Cross-compiling for $t (best effort)"
        rustup target add $t 2>$null | Out-Null
        cargo build --release -p ipgen-cli -p ipgen-mcp --target $t 2>$null
        if ($LASTEXITCODE -eq 0) {
            $d = if ($t -like "*linux*") { "bin/linux" } else { "bin/darwin" }
            New-Item -ItemType Directory -Force -Path $d | Out-Null
            foreach ($name in @("ipgen", "ipgen-mcp")) {
                Copy-Item -Force (Join-Path "target\$t\release" $name) (Join-Path $d $name)
            }
            Write-Host "    -> $d/ipgen, $d/ipgen-mcp"
        } else {
            Write-Warning "    (skipped: missing linker/toolchain for $t)"
        }
    }
}

Write-Host "==> Done. Binaries are in $outDir/"
