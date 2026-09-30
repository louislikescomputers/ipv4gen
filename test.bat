REM test.bat - build and smoke-test ipgen binaries on Windows (cmd.exe)
REM Prints generated IPv4 addresses to the command line and runs cargo test.
@echo off
setlocal enabledelayedexpansion

cd /d "%~dp0"

where cargo >nul 2>nul
if errorlevel 1 (
    echo error: cargo not found. Install Rust from https://rustup.rs 1>&2
    exit /b 1
)

if "%IPGEN_TEST_COUNT%"=="" set COUNT=20
if not "%IPGEN_TEST_COUNT%"=="" set COUNT=%IPGEN_TEST_COUNT%
set FAIL=0

echo ^=^>^ Building release binaries
cargo build --release -p ipgen-cli -p ipgen-mcp
if errorlevel 1 (echo error: build failed 1>&2 & exit /b 1)

set BIN=target\release\ipgen.exe
set MCP=target\release\ipgen-mcp.exe

call :case "ipgen --version"        "%BIN%" --version
call :case "ipgen categories"       "%BIN%" categories >nul
call :case "public permuted IPs"    "%BIN%" generate --preset public  --order permuted --seed 42 --count %COUNT%
call :case "public sequential IPs"  "%BIN%" generate --preset public  --order sequential --count %COUNT%
call :case "private permuted IPs"   "%BIN%" generate --preset private --order permuted --seed 7 --count %COUNT%
call :case "all preset IPs"         "%BIN%" generate --preset all     --order permuted --seed 1 --count %COUNT%
call :case "custom include/exclude" "%BIN%" generate --include 10.0.0.0/8 --exclude 10.16.0.0/12 --order sequential --count %COUNT%
call :case "jsonl format"           "%BIN%" generate --preset public --format jsonl --count 5
call :case "csv format"             "%BIN%" generate --preset public --format csv --count 5
call :case "blocks /24 CIDRs"       "%BIN%" generate --preset public --order blocks --prefix-len 24 --count 5
call :case "shard 1 of 3"           "%BIN%" generate --preset public --shard 1/3 --count %COUNT%

echo.
echo === checkpoint + resume ===
"%BIN%" generate --preset public --count 10 --state %TEMP%\ipgen.state.json --checkpoint-every 1 >nul
if errorlevel 1 (echo [FAIL] checkpoint write & set /a FAIL+=1) else (
    "%BIN%" resume --state %TEMP%\ipgen.state.json --count 10
    if errorlevel 1 (echo [FAIL] resume & set /a FAIL+=1) else (echo [PASS] checkpoint/resume round-trip)
    "%BIN%" status --state %TEMP%\ipgen.state.json
    del "%TEMP%\ipgen.state.json" >nul 2>nul
)

echo.
echo === mcp server stdio handshake ===
set MCPOUT=%TEMP%\ipgen_mcp_test.out
echo {"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test.bat","version":"0"}}}> "%TEMP%\ipgen_mcp_in.json"
"%MCP%" < "%TEMP%\ipgen_mcp_in.json" > "%MCPOUT%" 2>nul
findstr /c:"\"result\"" "%MCPOUT%" >nul 2>nul
if errorlevel 1 (echo [FAIL] ipgen-mcp did not respond over stdio & set /a FAIL+=1) else (echo [PASS] ipgen-mcp responded to initialize over stdio)
del "%TEMP%\ipgen_mcp_in.json" "%MCPOUT%" >nul 2>nul

echo.
echo === cargo test ===
cargo test --release --workspace
if errorlevel 1 (echo [FAIL] cargo test & set /a FAIL+=1) else (echo [PASS] cargo test)

echo.
echo =========================================
if %FAIL%==0 (
    echo ALL TESTS PASSED
    endlocal
    exit /b 0
)
echo %FAIL% TEST^(S^) FAILED
endlocal
exit /b 1

:case
echo.
echo === %~1 ===
shift
%*
if errorlevel 1 (
    echo [FAIL] %~1
    set /a FAIL+=1
) else (
    echo [PASS] %~1
)
exit /b 0
