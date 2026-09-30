# MASTERPROMPT: `ipgen` — Resumable IPv4 Range Generator in Rust (library + CLI + MCP server + agent skill + cross-platform build scripts)

You are a senior Rust engineer. Build the project described below **end to end**, and do not stop until every item in the "Definition of Done" section passes on your machine with real command output. You must actually run the build and the tests. Do not claim something compiles unless you ran it. If something fails, read the error, fix it, and rerun. Work in the stages listed, and pass each stage's gate before starting the next.

---

## 1. Purpose

`ipgen` generates IPv4 addresses, ranges, and CIDR blocks **algorithmically**, with no pre-generated lists on disk. Other programs and AI agents call it to get the next batch of addresses for network surveying, mapping, asset inventory, and research on networks the caller is authorized to examine.

Key requirements:

1. **Algorithmic generation** of the full IPv4 space (2^32 addresses), with categories for public and private/special ranges.
2. **Pause / resume**: the entire generator state must be a few integers, so a session can stop at any moment and continue exactly where it left off, even after a process restart or crash.
3. **Easy API**: a Rust library, a CLI with machine-friendly output, an MCP server, and an agent skill.
4. **Compiles the first time, everywhere**: a minimal dependency set, pinned toolchain, and ready-made build scripts for every major OS.

### Non-goals (strict)
`ipgen` **only enumerates addresses**. It must contain **no networking code**: no sockets, no probing, no port scanning, no DNS. Callers do whatever they want with the addresses. Put this statement in the README, along with a one-line reminder that users must only survey networks they own or are authorized to test.

---

## 2. Core algorithm design (follow this; do not substitute something weaker)

### 2.1 Address model
- Represent an address as `u32` (big-endian numeric value of the dotted quad). Use `std::net::Ipv4Addr` only at the edges.
- Everything is expressed in terms of a **sorted, non-overlapping, merged list of inclusive `u32` intervals** (`IntervalSet`). This is the single source of truth for what is "allowed" in a session.

### 2.2 Range classification (hard-code from the IANA special-purpose registry, RFC 6890 and related)
Provide an enum `Category` and a const table. At minimum:

| CIDR | Category |
|---|---|
| 0.0.0.0/8 | `ThisNetwork` |
| 10.0.0.0/8 | `Private` |
| 100.64.0.0/10 | `SharedCgnat` |
| 127.0.0.0/8 | `Loopback` |
| 169.254.0.0/16 | `LinkLocal` |
| 172.16.0.0/12 | `Private` |
| 192.0.0.0/24 | `IetfProtocol` |
| 192.0.2.0/24 | `Documentation` |
| 192.88.99.0/24 | `Deprecated6to4` |
| 192.168.0.0/16 | `Private` |
| 198.18.0.0/15 | `Benchmarking` |
| 198.51.100.0/24 | `Documentation` |
| 203.0.113.0/24 | `Documentation` |
| 224.0.0.0/4 | `Multicast` |
| 240.0.0.0/4 | `Reserved` |
| 255.255.255.255/32 | `Broadcast` |
| everything else | `Public` |

Expose `classify(Ipv4Addr) -> Category` and `ranges_for(&[Category]) -> IntervalSet`. Users pick which categories to **include** and/or **exclude** (presets: `public`, `private`, `all`, `public+private`). Also accept user-supplied extra include/exclude CIDRs or `a.b.c.d-e.f.g.h` ranges (for example, an exclusion list). Verify with a test that the categories partition the whole 2^32 space exactly (sizes sum to 4,294,967,296, with no overlaps).

### 2.3 Index → address mapping (this is what makes pause/resume trivial)
Let `N` = total number of allowed addresses (`u64`, can be up to 2^32).

- Build a prefix-sum array over the intervals. `nth(i: u64) -> u32` maps an index `i in [0, N)` to the i-th allowed address with a binary search over the prefix sums (O(log k)).
- **The generator state is just a cursor `i`.** Next address = `order(i)` mapped through `nth`. Pausing means saving `i`. Resuming means setting `i`.

### 2.4 Ordering modes (`--order`)
1. `sequential`: `order(i) = i`. Fastest; ascending order. Also provide a fast path that iterates interval by interval with no binary search per address.
2. `permuted` (default for survey use): a **keyed pseudo-random bijection over `[0, N)`** so addresses are spread across the whole internet instead of hammering one subnet at a time. Implement it as a **cycle-walking Feistel network**:
   - Choose the smallest even bit-width `2w` with `2^(2w) >= N` (minimum 2).
   - Balanced Feistel with 4 to 8 rounds over `w`-bit halves, round function = a cheap keyed integer mixer (for example a splitmix/murmur-style finalizer with a round key derived from the seed). Not cryptographic; document that.
   - `order(i)`: apply the Feistel permutation to `i`; while the result is `>= N`, apply it again (cycle walking). This yields an exact bijection on `[0, N)`.
   - It must be **random-access and stateless**: `order(i)` depends only on `(seed, N, i)`. No large tables, O(1) memory.
   - Same `(seed, config)` always yields the same sequence (reproducible runs).
3. Optional `blocks` mode: iterate `/24` (configurable prefix length) blocks in permuted order, yielding each block as a CIDR or range. Useful for mapping.

### 2.5 Sharding (for multiple workers)
`--shard K/M`: worker K of M processes indices `i` where `i % M == K`. Implement it as a stride on the cursor. Shards must be disjoint and together cover everything exactly once (test this).

### 2.6 Checkpoints
- `Checkpoint` struct serialized as JSON: `{ version, session_id, config (categories, extra include/exclude, order, seed, prefix_len, shard), config_hash, total, cursor, emitted, created_at, updated_at }`.
- **Atomic writes**: write to a temp file in the same directory, `fsync`, then rename over the target. Never leave a half-written checkpoint.
- On resume, recompute `config_hash` from the stored config and **refuse to resume if it does not match** the stored hash (corruption or tampering). Use a simple, dependency-free hash (for example FNV-1a 64 or SipHash from std) and say so in the docs.
- The CLI saves a checkpoint every N addresses or T seconds (configurable), on SIGINT/Ctrl-C (use the `ctrlc` crate, which supports Windows/macOS/Linux), and on clean exit.

---

## 3. Project layout (Cargo workspace)

```
ipgen/
├── Cargo.toml                 # [workspace], resolver = "2"
├── Cargo.lock                 # COMMIT THIS
├── rust-toolchain.toml        # channel = "stable", components = ["clippy","rustfmt"]
├── README.md
├── LICENSE                    # MIT OR Apache-2.0
├── crates/
│   ├── ipgen-core/            # library: all logic, ZERO heavy deps
│   ├── ipgen-cli/             # binary `ipgen`
│   └── ipgen-mcp/             # binary `ipgen-mcp` (MCP server, stdio)
├── skills/
│   └── ipgen/
│       ├── SKILL.md           # agent skill
│       └── scripts/           # tiny helper wrappers (see §7)
├── scripts/                   # build + test scripts (see §8)
├── docs/
│   ├── API.md
│   ├── MCP.md
│   └── ALGORITHM.md
└── .github/workflows/ci.yml   # build + test matrix: ubuntu, macos, windows
```

**Dependency policy (critical for first-try compilation):**
- `ipgen-core`: `serde` (derive) and `serde_json` only. Everything else is std.
- `ipgen-cli`: `ipgen-core`, `clap` (derive, v4), `ctrlc`, `serde_json`.
- `ipgen-mcp`: `ipgen-core`, `serde`, `serde_json`. **Hand-roll the minimal MCP JSON-RPC 2.0 stdio loop** (see §6) rather than depending on a fast-moving MCP SDK crate. Do not add `tokio` or any async runtime. A blocking stdin/stdout loop is enough.
- Dev: `proptest` (optional) and `criterion` (optional; put the benches behind a non-default feature so they never break a plain build).
- Set `edition = "2021"` and `rust-version = "1.75"`. Check every dependency you add against that MSRV, and pin exact-compatible versions in `Cargo.lock`.
- Gate: `cargo build --workspace --locked` must succeed on a clean checkout with no warnings-as-errors surprises. Use `#![forbid(unsafe_code)]` in all crates.

---

## 4. Library API (`ipgen-core`)

Design for ergonomics. A caller should get going in five lines.

```rust
use ipgen_core::{Generator, Config, Preset, Order};

let cfg = Config::builder()
    .preset(Preset::Public)          // or .categories(&[...]), .include("10.0.0.0/8"), .exclude("10.1.0.0/16")
    .order(Order::Permuted { seed: 42 })
    .build()?;

let mut g = Generator::new(cfg)?;
while let Some(batch) = g.next_batch(10_000) {   // Vec<Ipv4Addr> or fill a caller-provided &mut [u32]
    for ip in batch { /* ... */ }
    if should_pause() {
        let ckpt = g.checkpoint();               // serde-serializable
        ckpt.save_atomic("state.json")?;
        break;
    }
}
// later, even in another process:
let mut g = Generator::resume(Checkpoint::load("state.json")?)?;
```

Required public surface:
- `Config` + `ConfigBuilder`, `Preset`, `Category`, `Order`, `Shard`.
- `IntervalSet` (parse from CIDR and `a-b` strings, union, subtract, `len() -> u64`, `nth(u64)`, `contains`, `iter_intervals`).
- `Generator`: `new`, `resume`, `next()`, `next_batch(n)`, `next_batch_into(&mut [u32]) -> usize`, `next_cidr_blocks(n)` (for blocks mode), `checkpoint()`, `seek(index)`, `remaining()`, `total()`, `progress() -> f64`, and `impl Iterator<Item = Ipv4Addr>`.
- `Checkpoint`: `save_atomic`, `load`, `validate`.
- Clean `Error` enum implementing `std::error::Error` (no `anyhow` in the library).
- Doc comments and at least one runnable doctest on each major type.

**Performance targets (single thread, release build):** sequential ≥ 200 M addr/s batch-filling; permuted ≥ 50 M addr/s. Add a `--bench` subcommand (or a small example) that prints measured throughput. The targets guide the design; report actual numbers, do not fake them.

---

## 5. CLI (`ipgen`)

Subcommands:
- `ipgen generate [--preset public|private|all] [--categories ...] [--include CIDR|RANGE]... [--exclude CIDR|RANGE]... [--exclude-file PATH] [--order sequential|permuted] [--seed N] [--shard K/M] [--limit N] [--format ip|cidr|range|json|jsonl|u32] [--blocks PREFIX_LEN] [--checkpoint PATH] [--checkpoint-every N] [--resume]`
- `ipgen resume --checkpoint PATH [--limit N] [--format ...]`
- `ipgen status --checkpoint PATH` (prints progress, cursor, total, percent, remaining)
- `ipgen count [same filters as generate]` (prints the number of addresses)
- `ipgen classify <IP>...`
- `ipgen ranges [--preset ...]` (prints the merged CIDR or range list for the selection)
- `ipgen bench`

Behavior rules:
- Output goes to **stdout**, logs and progress to **stderr**. Output must be buffered (`BufWriter`, large buffer) and must handle `BrokenPipe` quietly (so `ipgen generate | head` works).
- Exit codes: 0 success, 1 runtime error, 2 usage error, 130 on Ctrl-C (after checkpoint flush).
- `--format jsonl` emits one JSON object per line. Document the schema in `docs/API.md`.
- Ctrl-C flushes a checkpoint, then exits. Running the same command with `--resume` continues with **no duplicated and no skipped addresses** (this is a tested requirement; see §9).

---

## 6. MCP server (`ipgen-mcp`)

Implement a **Model Context Protocol server over stdio**: newline-delimited JSON-RPC 2.0 on stdin/stdout, **nothing but protocol messages on stdout** (logs go to stderr). Before coding, look up the current MCP specification (modelcontextprotocol.io, "Transports: stdio" and "Tools") and follow its current revision for the `initialize` handshake, `protocolVersion` negotiation, capabilities, and message shapes. Handle at least:

- `initialize` → respond with server info (`ipgen-mcp`, version) and `capabilities: { tools: {} }`; accept the client's `protocolVersion` and echo a supported one.
- `notifications/initialized` → no response.
- `ping` → empty result.
- `tools/list` → the tool catalogue below, each with a precise JSON Schema `inputSchema`.
- `tools/call` → execute the tool; return `{ content: [{type:"text", text:"<JSON string>"}], isError: bool }`. On bad input, return `isError: true` with a helpful message; reserve JSON-RPC errors for protocol-level problems (unknown method `-32601`, invalid params `-32602`, parse error `-32700`).

**Tools:**
| Tool | Purpose |
|---|---|
| `ipgen_create_session` | Create a session from a config (preset/categories/include/exclude/order/seed/shard/prefix_len). Returns `session_id`, `total`. |
| `ipgen_next` | `{session_id, count (max 100000, default 1000), format}` → next batch plus `cursor`, `remaining`, `done`. |
| `ipgen_pause` | Persist a checkpoint to the session store; returns the checkpoint path. |
| `ipgen_resume` | Load a session from a checkpoint path or `session_id`. |
| `ipgen_status` | Progress, cursor, total, percent, config summary. |
| `ipgen_seek` | Set the cursor to a specific index. |
| `ipgen_list_sessions` | List saved sessions. |
| `ipgen_close` | Drop a session (optionally delete its checkpoint). |
| `ipgen_classify` | Classify one or more IPs. |
| `ipgen_count` | Count the addresses for a config without creating a session. |
| `ipgen_ranges` | Return the merged CIDR list for a config. |

Session store: directory from `--state-dir` or env `IPGEN_STATE_DIR`, default to the OS data dir via `std::env` (no extra crate: `%LOCALAPPDATA%\ipgen` on Windows, `$XDG_DATA_HOME/ipgen` or `~/.local/share/ipgen` on Linux, `~/Library/Application Support/ipgen` on macOS). Auto-checkpoint after every `ipgen_next`, so a killed server loses nothing. Cap batch sizes so an agent cannot blow its own context window, and return a hint in the response when `done` is true.

Write `docs/MCP.md` with copy-paste client config snippets (Claude Desktop / generic `mcpServers` JSON using the absolute path of the binary) and an example transcript. Provide a **test script** that pipes a scripted `initialize` → `tools/list` → `ipgen_create_session` → `ipgen_next` → `ipgen_pause` → `ipgen_resume` → `ipgen_next` sequence into the binary and asserts the responses (see §9).

---

## 7. Agent skill (`skills/ipgen/`)

Create a skill in the standard `SKILL.md` format with YAML frontmatter:

```markdown
---
name: ipgen
description: Generate IPv4 addresses, ranges, and CIDR blocks algorithmically (public, private, or custom sets), with pause/resume. Use when a task needs batches of IPv4 targets for authorized network surveys, mapping, or inventory, or when classifying IPs as public/private/reserved. Does not perform any network probing.
---
```

Body must contain: when to use it, when **not** to use it, the quick-start commands, how to resume, how to choose presets and ordering, output-format guidance, batch-size guidance for agents, a worked example, and a reminder to only survey authorized networks. Keep it under ~150 lines. Put tiny wrapper scripts in `skills/ipgen/scripts/` (`ipgen.sh` and `ipgen.ps1`) that locate the binary (env `IPGEN_BIN`, then `./dist`, then `PATH`) and forward arguments.

---

## 8. Build, test, and packaging scripts (mandatory, in `scripts/`)

The user must be able to compile with **one command on any OS** without knowing Cargo. Every script must: `set -euo pipefail` semantics (or the PowerShell/batch equivalent: stop on the first error and return a non-zero exit code), print clear step messages, check for `cargo`/`rustup` and, if missing, print the exact install instructions (and offer to install via rustup with a flag like `--install-rust` / `-InstallRust`; never install silently), use `--locked`, and put finished binaries in `dist/<target>/`.

**Files to create:**

| File | Platform | Does |
|---|---|---|
| `scripts/build.ps1` | Windows (PowerShell 5.1+ and 7+) | Native release build of both binaries, then copy into `dist/`. Params: `-Target`, `-Debug`, `-InstallRust`, `-AllTargets`, `-Clean`. |
| `scripts/build.bat` | Windows (cmd) | Same basics without PowerShell; can call `cargo` directly. Must work when double-clicked (`pause` at the end only if launched interactively). |
| `scripts/build-linux.sh` | Linux | Native release build plus optional `--musl` static build (`x86_64-unknown-linux-musl`) and `--target`. |
| `scripts/build-macos.sh` | macOS | Native build, plus `--universal` producing a universal binary for `x86_64-apple-darwin` and `aarch64-apple-darwin` via `lipo`. |
| `scripts/build-unix.sh` | Any other Unix (FreeBSD, OpenBSD, NetBSD, illumos, Solaris, etc.) | Generic POSIX `sh` script: native build using whichever `cargo` is found. Note in comments which of these have Tier 1/2/3 Rust support. |
| `scripts/build-all.sh` | Linux/macOS host | Cross-compile the full matrix (below) using `cargo-zigbuild` or `cross` (auto-detect, print install instructions if absent), then package archives and checksums. |
| `scripts/build-all.ps1` | Windows host | Same matrix as far as Windows can do it (Windows MSVC/GNU targets natively; others via `cross` with Docker if available, otherwise print what was skipped and why). |
| `scripts/test.sh` / `scripts/test.ps1` / `scripts/test.bat` | all | `cargo fmt --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo test --workspace --locked`, then the MCP smoke test. |
| `scripts/package.sh` / `scripts/package.ps1` | all | Make `.tar.gz` (Unix targets) and `.zip` (Windows) archives in `dist/` with `SHA256SUMS`. |

**Target matrix for `build-all`:**
`x86_64-pc-windows-msvc`, `x86_64-pc-windows-gnu`, `aarch64-pc-windows-msvc`, `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`, `x86_64-apple-darwin`, `aarch64-apple-darwin`, `x86_64-unknown-freebsd`. Targets that cannot be built from the current host must be **skipped with a clear message**, not cause a crash. Finish with a summary table (target, status, binary size).

Also make every `.sh` file executable (`chmod +x`, and record the mode in git with `git update-index --chmod=+x` if you initialize a repo), use `#!/usr/bin/env bash` (except `build-unix.sh`, which uses `#!/bin/sh`), use LF line endings, and add a `.gitattributes` forcing `*.sh text eol=lf` and `*.bat text eol=crlf` and `*.ps1 text eol=crlf`.

Add the same matrix (build + test) to `.github/workflows/ci.yml` using `dtolnay/rust-toolchain@stable`, `Swatinem/rust-cache`, and a runner matrix of `ubuntu-latest`, `macos-latest`, `windows-latest`.

---

## 9. Testing requirements (all must exist and pass)

**Unit / property tests in `ipgen-core`:**
1. Categories partition the 2^32 space: sizes sum to exactly 4,294,967,296 with no overlap.
2. `IntervalSet` union/subtract/merge correctness against a brute-force model on small random inputs.
3. `nth(i)` is strictly increasing and in-range, and matches naive enumeration on small sets.
4. **Feistel bijection:** for every small `N` in a sweep (1..=5000, plus a few random larger values up to 2^20), `order` over `[0, N)` yields each index exactly once. Also check determinism by seed and that different seeds give different orders.
5. **Resume equivalence:** for random configs, generating K items, checkpointing, serializing to JSON, reloading, and resuming yields a stream identical to an uninterrupted run, for both orders and for shards.
6. **Shard coverage:** for M in {1,2,3,7}, the shards are pairwise disjoint and their union equals the full stream.
7. Checkpoint tamper test: modifying `cursor` is allowed, while modifying the config without fixing the hash is rejected. A truncated/garbage file yields an `Err`, not a panic.
8. Atomic save: simulate a failure between temp write and rename and verify the old checkpoint is intact.
9. An `#[ignore]`d full-space test: generate every address for `Preset::All` in permuted order into a 512 MiB bitset and assert that all 2^32 bits are set exactly once (run with `cargo test --release -- --ignored`). Document how to run it.

**CLI tests** (`assert_cmd`-style or plain `std::process::Command`; no extra deps if possible): `generate --limit`, `head`-style broken pipe handling, `resume` produces no duplicates and no gaps (concatenate two runs and compare to one run), `count` for known presets, `classify` for known addresses.

**MCP smoke test:** a script (`scripts/mcp-smoke.sh` and `.ps1`) or a Rust integration test that drives the server over stdio exactly as described in §6 and asserts on the JSON responses, including an error case (unknown tool, invalid arguments) and verifying that stdout contains **only** valid JSON-RPC lines.

---

## 10. Documentation deliverables

- `README.md`: what it is, the non-goals statement, 60-second quick start for **each OS** using the scripts in §8, CLI examples, library example, MCP config snippet, skill install instructions, pause/resume explanation, benchmark numbers you measured.
- `docs/ALGORITHM.md`: explain the interval set, the prefix-sum `nth`, the cycle-walking Feistel permutation (with the bijection argument), sharding, and why resume needs only `(config, cursor)`.
- `docs/API.md` and `docs/MCP.md` as described above.

---

## 11. Execution plan (follow in order, verify at each gate)

1. **Scaffold** the workspace, toolchain file, `.gitattributes`, empty crates. **Gate:** `cargo build --workspace --locked` succeeds.
2. **Core:** categories, `IntervalSet`, `nth`, sequential generator, checkpoint. **Gate:** unit tests 1 to 3 pass.
3. **Permutation and sharding.** **Gate:** tests 4 to 8 pass.
4. **CLI.** **Gate:** CLI tests pass; manually demonstrate Ctrl-C and resume.
5. **MCP server.** **Gate:** smoke test passes.
6. **Skill and docs.**
7. **Build/test/package scripts.** **Gate:** run the native build script for the OS you are on and show that `dist/` contains working binaries, then run the binaries (`ipgen count --preset public`, `ipgen generate --limit 5`).
8. **Final pass:** `cargo fmt`, `cargo clippy -D warnings`, full test suite, the ignored full-space test in release mode, benchmark numbers.

---

## 12. Definition of Done

Report back with **actual command output** for each:

- [ ] `cargo build --workspace --release --locked` succeeds from a clean clone
- [ ] `cargo test --workspace --locked` passes; the ignored full-space test passes in `--release`
- [ ] `cargo clippy --workspace --all-targets --locked -- -D warnings` is clean, and `cargo fmt --check` is clean
- [ ] The native build script for your current OS produces binaries in `dist/` and they run
- [ ] `ipgen count --preset all` prints `4294967296`
- [ ] Resume demo: generate 1,000,000 addresses in one run and in two interrupted-and-resumed runs, then show that the outputs are byte-identical
- [ ] MCP smoke test passes
- [ ] Every file in §3 and §8 exists; scripts for other OSes are at least syntax-checked (`bash -n`, `shellcheck` if available, PowerShell parser check)
- [ ] A short list of anything you could not verify on your platform (for example, Windows scripts when you are on Linux), so the human knows what to test

If any requirement here is ambiguous or seems impossible, choose the simplest approach that satisfies the intent, note the decision in `docs/ALGORITHM.md`, and continue. Do not ask for clarification unless you are completely blocked.
