REM compile.bat - build ipgen release binaries on Windows (cmd.exe)
REM Produces: bin\windows\ipgen.exe and bin\windows\ipgen-mcp.exe
@echo off
setlocal

cd /d "%~dp0"

where cargo >nul 2>nul
if errorlevel 1 (
    echo error: cargo not found. Install Rust from https://rustup.rs 1>&2
    exit /b 1
)

echo ^=^>^ Building ipgen (release) for windows
cargo build --release -p ipgen-cli -p ipgen-mcp
if errorlevel 1 (
    echo error: cargo build failed 1>&2
    exit /b 1
)

if not exist "bin\windows" mkdir "bin\windows"

copy /y "target\release\ipgen.exe"     "bin\windows\ipgen.exe"     >nul
if errorlevel 1 (echo error: missing target\release\ipgen.exe 1>&2 & exit /b 1)
copy /y "target\release\ipgen-mcp.exe" "bin\windows\ipgen-mcp.exe" >nul
if errorlevel 1 (echo error: missing target\release\ipgen-mcp.exe 1>&2 & exit /b 1)

echo     -^> bin\windows\ipgen.exe
echo     -^> bin\windows\ipgen-mcp.exe
echo ^=^>^ Done. Binaries are in bin\windows\
endlocal
