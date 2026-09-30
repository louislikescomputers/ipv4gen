# ipv4gen — Algorithmic IPv4 Range Generator

`ipgen` is a small, fast, dependency-light Rust tool that **algorithmically
enumerates IPv4 addresses, CIDRs, and `/prefix` blocks**. There are no
pre-generated lists on disk, no databases, no network I/O of any kind — the
generator computes the next address on demand from a tiny piece of state
that fits in a JSON file. That state is `(config, cursor)`: a description of
what you wanted, plus a single integer that says where you are. Pause at any
moment, kill the process, reboot the machine, and you can pick up the same
run later with no duplicates and no gaps.

The project is organised as a Cargo workspace with three crates:

| Crate        | Binary       | What it is                                                       |
|--------------|--------------|------------------------------------------------------------------|
| `ipgen-core` | (library)    | The generator logic — intervals, ordering, sharding, checkpoints.|
| `ipgen-cli`  | `ipgen`      | The command-line tool most users will run.                       |
| `ipgen-mcp`  | `ipgen-mcp`  | A stdio Model Context Protocol (MCP) server for AI agents.       |

> **Strict non-goal.** `ipgen` only enumerates addresses. It contains no
> sockets, no probing, no port scanning, no DNS. Use the output only against
> networks you own or are explicitly authorised to examine.

---

## What it gives you

- **Coverage of the full IPv4 unicast space** (all 2³² addresses, partitioned
  into 13 IANA categories per RFC 6890 and friends).
- **Three ordering modes**:
  - `sequential` — ascending order, fastest path.
  - `permuted` (default) — a keyed pseudo-random bijection over the allowed
    set, so a survey spreads across the whole internet instead of hammering
    one subnet at a time. Backed by a cycle-walking Feistel network.
  - `blocks` — iterate aligned `/N` blocks (default `/24`) in permuted
    order; useful for block-level mapping.
- **Sharding** (`--shard K/M`) so multiple workers can split a run with no
  overlap and no missed addresses.
- **Exact pause / resume** via an atomically-written JSON checkpoint that
  detects tampering through a content hash.
- **Three output formats**: plain text (one address per line), JSON Lines,
  and CSV. Plus a CIDR-only mode for `blocks` and `ranges`.
- **A hand-rolled MCP server** with no async runtime — one JSON-RPC message
  per line on stdin/stdout, perfect for plugging into Claude Desktop, Cursor,
  or any other MCP-aware agent.

---

## 60-second quick start

### Build

The repository ships with `compile.sh` (Linux/macOS), `compile.bat` (Windows
cmd), and `compile.ps1` (Windows PowerShell). Pick the one for your platform
and run it from the repository root:

```bash
# Linux or macOS
./compile.sh

# Or, with cargo directly:
cargo build --release -p ipgen-cli -p ipgen-mcp
# Binaries land in target/release/ipgen and target/release/ipgen-mcp
```

On Windows, double-click `compile.bat` or run `./compile.ps1` in PowerShell.
You will need the Rust toolchain (1.75 or newer); install it from
<https://rustup.rs> if you do not have it.

### First run

```bash
# Print the 13 built-in categories and their sizes (they sum to 2^32).
ipgen categories

# Generate the first 5 public IPv4 addresses in permuted order.
ipgen generate --preset public --count 5

# Generate the first 5 in ascending order (1.0.0.0, 1.0.0.1, ...).
ipgen generate --preset public --order sequential --count 5

# Pipe to a file with JSON-Lines output and periodic checkpoints.
ipgen generate --preset public --order permuted --seed 42 \
  --format jsonl --out addrs.jsonl \
  --checkpoint-every 1000 --state run.state.json
```

### Pause and resume

`Ctrl-C` at any time flushes a checkpoint to `ipgen.state.json` (or wherever
`--state` points). Resume with the `resume` subcommand:

```bash
# 1. Start a long run, then hit Ctrl-C partway through.
ipgen generate --preset public --order permuted --out addrs.txt

# 2. See where you stopped.
ipgen status --state ipgen.state.json

# 3. Continue from that exact position — no duplicates, no gaps.
ipgen resume --state ipgen.state.json --out addrs.txt
```

### Generate all public IPv4 addresses in order

This is the canonical "give me everything" example. Sequential order makes
the output reproducible and easy to slice with `head` / `tail` / `split`:

```bash
# 3,702,258,433 addresses, ascending. Pipe to a file or stream consumer.
# Default batch size 4096; --checkpoint-every 1000 batches writes a
# checkpoint roughly every 4 million addresses.
ipgen generate \
  --preset public \
  --order sequential \
  --format text \
  --batch 65536 \
  --checkpoint-every 1000 \
  --state public-run.state.json \
  --out public-ipv4.txt
```

The total number of public addresses is `2³² − 592,708,863 = 3,702,258,433`.
You can verify with:

```bash
ipgen ranges --preset public --summary
# {"cidr_count":2783,"total_addresses":3702258433}
```

---

## Library quick start

```rust
use ipgen_core::{Config, Generator, Order, Preset};

let cfg = Config::builder()
    .preset(Preset::Public)
    .order(Order::Permuted { seed: 42 })
    .build()?;

let mut g = Generator::new(cfg)?;
while let Some(batch) = g.next_batch(10_000) {
    for ip in batch {
        // ... your code here
    }
    if should_pause() {
        g.checkpoint().save_atomic("state.json")?;
        break;
    }
}

// Later — even in another process — pick up exactly where you stopped:
let mut g = Generator::resume(ipgen_core::Checkpoint::load("state.json")?)?;
```

See [`docs/API.md`](docs/API.md) for the full library surface.

---

## MCP quick start

`ipgen-mcp` speaks JSON-RPC 2.0 over stdio, one message per line. The
handshake is `initialize` → `notifications/initialized` → `tools/list` →
`tools/call`. The full tool catalogue is documented in
[`docs/MCP.md`](docs/MCP.md).

For Claude Desktop, drop this into `claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "ipgen": {
      "command": "/absolute/path/to/ipgen-mcp",
      "args": []
    }
  }
}
```

A complete worked transcript (create session → pull batches → pause → resume
→ close) lives in [`examples/mcp-transcript.json`](examples/mcp-transcript.json).

---

## Agent skill

This repo ships a ready-to-use agent skill under
[`skills/ipgen/`](skills/ipgen/). Drop the folder into your skill loader's
search path, point `IPGEN_BIN` at the `ipgen` binary, and the agent can:

- Generate batches of IPv4 addresses with exact pause/resume.
- Classify single addresses into the 13 IANA categories.
- List the merged CIDR set for any combination of categories.
- Pick sharded work for parallel agents.

The skill file format follows the standard `SKILL.md` + YAML frontmatter
convention; see [`skills/ipgen/SKILL.md`](skills/ipgen/SKILL.md) and the
agent-facing guide [`docs/AGENTS.md`](docs/AGENTS.md).

---

## Repository layout

```
ipv4gen-docs/
├── README.md                  # this file
├── docs/
│   ├── ALGORITHM.md           # how the generator actually works
│   ├── API.md                 # Rust library API reference
│   ├── MCP.md                 # MCP server reference + client snippets
│   ├── EXAMPLES.md            # cookbook of CLI & MCP examples
│   └── AGENTS.md              # how AI agents should use the tool
├── guides/
│   ├── getting-started.md     # 0 to first batch in 5 minutes
│   ├── installation.md        # build, install, verify on each OS
│   ├── pause-resume.md        # exact pause & resume semantics
│   ├── sharding.md            # splitting work across N workers
│   └── mcp-clients.md        # wiring into Claude Desktop / Cursor / generic
├── skills/
│   └── ipgen/
│       ├── SKILL.md           # agent skill description
│       └── scripts/
│           ├── ipgen.sh       # POSIX wrapper
│           └── ipgen.ps1      # PowerShell wrapper
├── man/
│   └── ipgen.1                # man page (groff/troff)
└── examples/
    ├── all-public-in-order.txt   # example transcript for the canonical run
    └── mcp-transcript.json       # example MCP session transcript
```

---

## Performance

Measured on commodity hardware (single thread, release build):

| Order       | Throughput (addresses/sec) | Notes                                     |
|-------------|----------------------------|-------------------------------------------|
| `sequential`| ≥ 200 M addr/s             | Bulk-fill fast path, no per-addr binary search. |
| `permuted`  | ≥ 50 M addr/s              | One Feistel call per address.             |
| `blocks`    | ~10–20 M blocks/sec        | Smaller batches, more bookkeeping per item.|

The stateless Feistel permutation means memory is `O(1)` regardless of how
many addresses a run will eventually emit — the generator does not hold a
permutation table.

---

## Safety & ethics

`ipgen` enumerates addresses and nothing else. It does **not**:

- open sockets,
- send ICMP,
- do DNS lookups,
- probe ports,
- fingerprint stacks,
- touch the network at all.

That is by design. Anyone using the output to scan networks they do not own
or are not authorised to test is responsible for their own conduct; the tool
itself is a pure enumeration utility. The authors and maintainers disclaim
any responsibility for misuse.

---

## License
```
            DO WHAT THE FUCK YOU WANT TO PUBLIC LICENSE
                    Version 2, December 2004

 Copyright (C) 2004 Sam Hocevar <sam@hocevar.net>

 Everyone is permitted to copy and distribute verbatim or modified
 copies of this license document, and changing it is allowed as long
 as the name is changed.

            DO WHAT THE FUCK YOU WANT TO PUBLIC LICENSE
   TERMS AND CONDITIONS FOR COPYING, DISTRIBUTION AND MODIFICATION

  0. You just DO WHAT THE FUCK YOU WANT TO.
```
## Contributing

Contributions are welcome via pull request. Please run
`cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
and `cargo test --workspace` before submitting. The project uses
`#![forbid(unsafe_code)]` in every crate — please do not introduce `unsafe`.
