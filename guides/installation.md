# Installation

`ipgen` is written in Rust and builds with `cargo`. There are no native
dependencies beyond the Rust standard library and a handful of well-maintained
crates (`clap`, `ctrlc`, `serde`, `serde_json`), so a clean build from source
takes a couple of minutes on any modern machine. This guide walks through the
prerequisites, the three shipping build scripts, where the binaries land,
and how to verify the install.

If you just want to try the tool quickly, the "Linux/macOS quick path"
below is the shortest route from zero to a working binary. The other
sections cover Windows, cross-compilation, and how to integrate the
binaries into your `PATH` so subsequent guides work without prefixing
every command.

## Prerequisites

You need the Rust toolchain, version 1.75 or newer (the workspace pins
this in `rust-toolchain.toml`). The easiest way to install Rust is
[`rustup`](https://rustup.rs), which gives you `cargo`, `rustc`, and a
profile that works for both development and release builds. On Debian and
Ubuntu you can also `apt install cargo`, but the distro version is often
stale; `rustup` is strongly recommended.

You also need a C toolchain if you ever pull in crates that link against C
libraries — `ipgen` itself does not, but `ctrlc` on certain platforms can
end up needing the platform's libc headers. On Linux that means `gcc` and
`libc6-dev`; on macOS the Xcode Command Line Tools (`xcode-select --install`)
are enough; on Windows the MSVC build tools or a MinGW toolchain work.

Verify your install before continuing:

```bash
cargo --version       # should print cargo 1.75.0 (or newer)
rustc --version       # should print rustc 1.75.0 (or newer)
```

## Linux / macOS quick path

From a fresh clone of the repository:

```bash
git clone https://github.com/louislikescomputers/ipv4gen.git
cd ipv4gen
./compile.sh
```

`compile.sh` is intentionally simple: it checks that `cargo` is on `PATH`,
runs `cargo build --release -p ipgen-cli -p ipgen-mcp`, then copies the two
resulting binaries into `bin/linux/` (on Linux) or `bin/darwin/` (on macOS).
The script is `set -euo pipefail`, so any failure stops the build with a
non-zero exit code — no half-built state to clean up.

When it finishes you should see:

```
==> Building ipgen (release) for linux/x86_64
    Compiling ipgen-core v0.1.0
    Compiling ipgen-mcp v0.1.0
    Compiling ipgen-cli v0.1.0
    -> bin/linux/ipgen
    -> bin/linux/ipgen-mcp
==> Done. Binaries are in bin/linux/
```

Verify the binaries actually run:

```bash
./bin/linux/ipgen --version
./bin/linux/ipgen categories
./bin/linux/ipgen-mcp --help 2>&1 || true   # the MCP server has no --help; that is fine
```

## Cross-compiling for the other desktop OS

`compile.sh` accepts an optional `--all-targets` flag that attempts to
cross-compile for the other desktop platforms when the required Rust
targets and linkers are installed. On a Linux host, for example, it will
try to also build for `x86_64-apple-darwin` and `x86_64-pc-windows-gnu`.
The script uses `rustup target add` on demand and prints a clear message
when a target has to be skipped because its toolchain is missing.

```bash
./compile.sh --all-targets
```

Cross-compilation from Linux to Apple Silicon requires `cargo-zigbuild`
or `cross`; the build script does not invoke them automatically, but you
can run them yourself once the targets are installed. See the
"Cross-compilation" section below.

## Windows quick path

Two scripts ship for Windows: `compile.bat` for `cmd.exe` and `compile.ps1`
for PowerShell 5.1 and 7+. They do the same thing — check that `cargo` is
available, run the release build, and copy the binaries into `bin/windows/`.

From a PowerShell prompt:

```powershell
git clone https://github.com/louislikescomputers/ipv4gen.git
cd ipv4gen
.\compile.ps1
```

Or from `cmd.exe`:

```cmd
git clone https://github.com/louislikescomputers/ipv4gen.git
cd ipv4gen
compile.bat
```

Either way the result is `bin\windows\ipgen.exe` and
`bin\windows\ipgen-mcp.exe`. Verify they run:

```powershell
.\bin\windows\ipgen.exe --version
.\bin\windows\ipgen.exe categories
```

## Putting the binaries on your PATH

The rest of the documentation assumes `ipgen` is on your `PATH`. The
simplest way to arrange that is to symlink or copy the binary into
`/usr/local/bin` (Linux/macOS) or to add the platform's `bin/` directory
to your `PATH` (any OS).

Linux / macOS:

```bash
sudo cp bin/linux/ipgen bin/linux/ipgen-mcp /usr/local/bin/
ipgen --version    # should now work from anywhere
```

Windows (PowerShell, user-scope PATH so no admin needed):

```powershell
[Environment]::SetEnvironmentVariable(
  "Path",
  $env:Path + ";$pwd\bin\windows",
  "User")
# Open a new shell; ipgen and ipgen-mcp should now be on PATH.
```

The MCP server (`ipgen-mcp`) does not need to be on `PATH` for an MCP
client like Claude Desktop to find it — you can give the client the
absolute path to the binary in its config file. See
[`mcp-clients.md`](mcp-clients.md) for exact snippets.

## Building with cargo directly

If you prefer to skip the wrapper scripts and call `cargo` yourself, the
canonical command is:

```bash
cargo build --release --locked -p ipgen-cli -p ipgen-mcp
```

The `--locked` flag respects `Cargo.lock`, so the build is reproducible from
the pinned dependency versions. The resulting binaries live in
`target/release/ipgen` and `target/release/ipgen-mcp`; you can copy them
wherever you like.

For a debug build (faster compile, slower runtime):

```bash
cargo build -p ipgen-cli -p ipgen-mcp
# Binaries in target/debug/{ipgen, ipgen-mcp}
```

The release build is strongly recommended for any real run — `sequential`
order can sustain hundreds of millions of addresses per second in release
mode but only a few million in debug mode, because the bulk-fill fast path
is heavily optimised and the optimiser matters.

## Cross-compilation

For cross-compiling to a target your host does not natively support, the
easiest route is [`cargo-zigbuild`](https://github.com/rust-cross/cargo-zigbuild),
which uses Zig as the linker and works on Linux, macOS, and Windows hosts.

Install `cargo-zigbuild` and the Zig toolchain once:

```bash
cargo install cargo-zigbuild
# Install Zig from https://ziglang.org or your package manager
```

Then build for any target the project supports:

```bash
rustup target add x86_64-unknown-linux-musl
cargo zigbuild --release --target x86_64-unknown-linux-musl \
  -p ipgen-cli -p ipgen-mcp
# Result: target/x86_64-unknown-linux-musl/release/{ipgen, ipgen-mcp}
```

Common targets you might want:

- `x86_64-unknown-linux-gnu` — Linux on x86-64 (default on a Linux host).
- `x86_64-unknown-linux-musl` — fully static Linux binary, works on any
  glibc version.
- `aarch64-unknown-linux-gnu` — Linux on ARM64 (Raspberry Pi 4/5, Graviton).
- `x86_64-apple-darwin` — macOS on Intel.
- `aarch64-apple-darwin` — macOS on Apple Silicon.
- `x86_64-pc-windows-gnu` — Windows on x86-64 via MinGW.

## Verifying the install

Run the four-command smoke test below; if all four produce sensible output,
the install is good:

```bash
ipgen --version          # prints version string
ipgen categories         # prints the 13 categories with sizes
ipgen generate --preset public --count 3   # emits 3 addresses
ipgen ranges --preset public --summary     # prints JSON {"cidr_count":2783,...}
```

If `ipgen categories` exits with a non-zero code or prints an error, the
binary is corrupt or the platform is unsupported. Re-run `compile.sh` and
watch for warnings.

If `ipgen generate --preset public --count 3` produces three addresses
that are all in the public category (i.e. none of them is in `0.0.0.0/8`,
`10/8`, etc.), the generator and the classification table are both correct.
You can confirm with:

```bash
ipgen generate --preset public --count 3 --format jsonl \
  | jq -r .category   # should print "public" three times
```

## Uninstall

There is no `make install` step — just delete the binaries:

```bash
rm /usr/local/bin/ipgen /usr/local/bin/ipgen-mcp
```

Or, if you used the `bin/<platform>/` layout:

```bash
rm -rf ipv4gen/
```

No config files, caches, or state directories are written by `ipgen` itself
unless you ask for a checkpoint with `--state`; the MCP server similarly
keeps sessions in memory and forgets them on exit.

## Troubleshooting

**`cargo: command not found`** — install Rust via <https://rustup.rs>.

**`error: failed to run custom build command for openssl-sys`** —
`ipgen` does not depend on OpenSSL. If you see this, you are probably
building something else in the same workspace; check your `Cargo.toml`.

**`error: linker 'cc' not found`** — install the platform's C toolchain
(`build-essential` on Debian/Ubuntu, the Xcode Command Line Tools on
macOS, MSVC build tools or MinGW on Windows).

**`ipgen: paused after 0 more items`** — the checkpoint was saved with
the cursor already at the end. Start a fresh run with `ipgen generate`
rather than `ipgen resume`.

**Permission denied writing to `ipgen.state.json`** — the directory
containing the checkpoint file is not writable by the current user. Use
`--state /path/you/can/write/to.state.json` to point elsewhere.
