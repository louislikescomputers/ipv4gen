# ipgen.ps1 — PowerShell wrapper that locates the ipgen binary and
# forwards all arguments. Resolution order: $env:IPGEN_BIN, .\dist\ipgen.exe,
# .\bin\windows\ipgen.exe, then PATH.
#
# Usage:
#   $env:IPGEN_BIN = "C:\bin\ipgen.exe"
#   .\ipgen.ps1 generate --preset public --count 5
#
#   .\ipgen.ps1 categories
#   .\ipgen.ps1 --help
#
# This wrapper is intentionally minimal: it never modifies arguments
# and never prints anything of its own (unless the binary cannot be
# found). The goal is to give agent skills a stable invocation surface
# regardless of how the underlying binary was installed on the host.

[CmdletBinding()]
param(
    [Parameter(Position = 0, ValueFromRemainingArguments = $true)]
    [string[]]$Args
)

$ErrorActionPreference = "Stop"

# 1. Explicit override via environment.
if ($env:IPGEN_BIN -and (Test-Path -Path $env:IPGEN_BIN -PathType Leaf)) {
    $exe = $env:IPGEN_BIN
    if ($exe -notmatch '\.exe$') { $exe += '.exe' }
    if (Test-Path -Path $exe -PathType Leaf) {
        & $exe @Args
        exit $LASTEXITCODE
    }
}

# 2. .\dist\ipgen.exe relative to the repository root.
$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
# Walk up two directories: skills\ipgen\scripts\ -> repo root.
$repoRoot = Resolve-Path (Join-Path $scriptDir '..\..') | Select-Object -ExpandProperty Path
$distBin  = Join-Path $repoRoot 'dist\ipgen.exe'
if (Test-Path -Path $distBin -PathType Leaf) {
    & $distBin @Args
    exit $LASTEXITCODE
}

# 3. .\bin\windows\ipgen.exe (compile.ps1 output).
$windowsBin = Join-Path $repoRoot 'bin\windows\ipgen.exe'
if (Test-Path -Path $windowsBin -PathType Leaf) {
    & $windowsBin @Args
    exit $LASTEXITCODE
}

# 4. .\target\release\ipgen.exe (cargo build --release output).
$releaseBin = Join-Path $repoRoot 'target\release\ipgen.exe'
if (Test-Path -Path $releaseBin -PathType Leaf) {
    & $releaseBin @Args
    exit $LASTEXITCODE
}

# 5. PATH lookup as a last resort.
$found = Get-Command ipgen -ErrorAction SilentlyContinue
if ($found) {
    & $found.Source @Args
    exit $LASTEXITCODE
}

# 6. Could not find the binary; print a helpful error.
Write-Error @"
ipgen.ps1: could not locate the ipgen binary.

Tried, in order:
  1. `$env:IPGEN_BIN (not set or not found)
  2. $distBin
  3. $windowsBin
  4. $releaseBin
  5. PATH lookup for 'ipgen'

To fix this, either:
  - Set `$env:IPGEN_BIN to the absolute path of ipgen.exe, or
  - Build with '.\compile.ps1' from the repository root, or
  - Install ipgen.exe on your PATH.
"@
exit 127
