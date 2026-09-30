---
name: ipgen
description: Generate IPv4 addresses, ranges, and CIDR blocks algorithmically (public, private, or custom sets), with exact pause/resume and sharding. Use when a task needs batches of IPv4 targets for authorized network surveys, mapping, or inventory, or when classifying IPs as public/private/reserved. Does not perform any network probing.
---

# ipgen — Algorithmic IPv4 Range Generator

`ipgen` is a small, fast Rust tool that **algorithmically enumerates
IPv4 addresses, CIDRs, and `/prefix` blocks** with exact pause/resume
and sharding. There are no pre-generated lists on disk; the next
address is computed on demand from a tiny piece of state
(`(config, cursor)`) that fits in a JSON file. Kill the process, reboot
the machine, and you can pick up the same run later with no
duplicates and no gaps.

## When to use this skill

- A task needs to enumerate a large IPv4 set (anything from a `/24`
  to the full public space, 3.7 billion addresses) and feed the
  results to another tool.
- A task needs to resume an interrupted enumeration without
  duplicating or skipping addresses.
- A task needs to spread enumeration across multiple workers (parallel
  agents, multiple machines, a cron fleet) with no coordination
  overhead.
- A task needs to classify IPv4 addresses into IANA categories
  (public, private, loopback, multicast, etc.) without
  reimplementing RFC 6890.
- A task needs a deterministic, reproducible address stream for
  testing or for A/B comparisons.

## When NOT to use this skill

- You need a single random IP. Use `python -c "import random; ..."`
  instead.
- You need to *scan* a network. `ipgen` does no probing — no
  sockets, no ICMP, no DNS. Pair it with `nmap`, `masscan`, or your
  own scanning tool.
- You need IPv6. `ipgen` is IPv4-only by design.
- You need *filtered* enumeration (e.g. "all addresses that respond
  to ping"). That requires network I/O, which `ipgen` does not do.

## Quick start

Locate the `ipgen` binary (set `IPGEN_BIN`, else fall back to
`./dist/ipgen`, else `PATH`):

```bash
# 5 public addresses in ascending order
$IPGEN_BIN generate --preset public --order sequential --count 5
# 1.0.0.0
# 1.0.0.1
# 1.0.0.2
# 1.0.0.3
# 1.0.0.4

# 5 public addresses in permuted order with a fixed seed
$IPGEN_BIN generate --preset public --order permuted --seed 42 --count 5 \
  --format jsonl
# {"index":0,"address":"203.0.113.42","category":"public"}
# ...

# Generate all public IPv4 addresses in order (3.7 billion addresses)
$IPGEN_BIN generate --preset public --order sequential \
  --batch 65536 --checkpoint-every 1000 \
  --state public-run.state.json --out public-ipv4.txt
# (long-running; resume with `ipgen resume --state public-run.state.json`)

# Inspect a checkpoint
$IPGEN_BIN status --state public-run.state.json

# Resume an interrupted run
$IPGEN_BIN resume --state public-run.state.json --out public-ipv4.txt

# Classify an address
$IPGEN_BIN categories --detailed | grep 8.8.8.8 || \
  echo "8.8.8.8 is in the public category"

# List the merged CIDRs for a selection
$IPGEN_BIN ranges --preset private
# 10.0.0.0/8
# 172.16.0.0/12
# 192.168.0.0/16

# Shard across 4 workers
for k in 0 1 2 3; do
  $IPGEN_BIN generate --preset public --order permuted --seed 42 \
    --shard $k/4 --state shard-$k.state.json --out shard-$k.txt &
done
wait
```

## Choosing the order

- **`sequential`** — ascending order. Fastest, reproducible without a
  seed. Use when you do not need spread.
- **`permuted`** (default) — keyed pseudo-random bijection over the
  allowed set. Spreads addresses across the whole space rather than
  hammering one subnet at a time. Use for survey workloads.
- **`blocks`** — iterate aligned `/N` blocks (default `/24`,
  configurable via `--prefix-len`) in permuted order. Use for
  block-level mapping or for feeding into `nmap -iL`.

## Choosing a preset

- `public` — everything not in a special-use block (3.7 B addresses).
- `private` — RFC 1918 space: `10/8`, `172.16/12`, `192.168/16`.
- `all` — every address, partitioning the full 2³² space.
- `public+private` — public plus RFC 1918.

Or pass `--categories cat1,cat2,...` with explicit names from the
table below, plus optional `--include CIDR|RANGE` and
`--exclude CIDR|RANGE` for custom sets.

## Built-in categories

```
this-network      0.0.0.0/8
private           10/8, 172.16/12, 192.168/16
shared-cgnat      100.64.0.0/10
loopback          127.0.0.0/8
link-local        169.254.0.0/16
ietf-protocol     192.0.0.0/24
documentation     192.0.2.0/24, 198.51.100.0/24, 203.0.113.0/24
deprecated-6to4   192.88.99.0/24
benchmarking      198.18.0.0/15
multicast         224.0.0.0/4
reserved          240.0.0.0/4 (minus 255.255.255.255)
broadcast         255.255.255.255/32
public            everything else
```

The 13 categories partition the full 2³² space — sizes sum to
exactly 4,294,967,296 with no overlaps.

## Output formats

- `text` (default) — one address (or CIDR) per line. Best for piping
  into other Unix tools.
- `jsonl` — one JSON object per line with `index`, `address`,
  `category`. Best for structured processing.
- `csv` — three columns (`index`, `address`, `category`) with a
  header row. Best for spreadsheets and databases.

## Pause and resume

`Ctrl-C` at any time flushes a checkpoint to `ipgen.state.json` (or
wherever `--state` points). Resume with the `resume` subcommand:

```bash
ipgen generate --preset public --order permuted --seed 7 --out addrs.txt
# Ctrl-C partway through
ipgen resume --state ipgen.state.json --out addrs.txt
# Continues from the saved cursor — no duplicates, no gaps.
```

The checkpoint stores `(config, cursor)` plus a content hash; tampering
with the config without recomputing the hash is refused on load.

## Batch-size guidance for agents

Each emitted address in JSON-Lines is about 70 bytes. Pick a batch
size that fits comfortably in your context window:

- Initial exploration: `--count 10` to `--count 100`.
- Production enumeration: `--count 1000` to `--count 10000`.
- Bulk transfer to a file: `--count 100000` with `--out`. The
  response payload does not include the addresses themselves; only
  the cursor and total.

## Sharding for parallel agents

`--shard K/M` means "I am worker `K` of `M` total." Shards are
disjoint and cover the full stream exactly once. Each shard has its
own checkpoint and can be paused/resumed independently.

```bash
for k in 0 1 2 3; do
  ipgen generate --preset public --order permuted --seed 42 \
    --shard $k/4 --state shard-$k.state.json --out shard-$k.txt &
done
wait
```

The same `--seed` produces the same permuted stream across all shards.

## Worked example: enumerate 1000 public addresses, then classify

```bash
# Generate the addresses.
$IPGEN_BIN generate --preset public --order permuted --seed 99 \
  --count 1000 --format jsonl --out addrs.jsonl

# Tally the categories (will be 100% public since we asked for public).
$IPGEN_BIN cat addrs.jsonl | jq -r .category | sort | uniq -c
# 1000 public

# Or classify one specific address:
echo "8.8.8.8" | xargs -I{} $IPGEN_BIN generate --include {} \
  --categories public --count 1 --format jsonl | jq -r '.[0].category'
# public
```

(There is no dedicated `classify` subcommand in the CLI today; use
the JSONL output's `category` field, or use the MCP server's
`category_of` tool.)

## MCP server

`ipgen-mcp` speaks JSON-RPC 2.0 over stdio. Tool catalogue:
`generate_start`, `generate_next`, `generate_status`,
`generate_pause`, `generate_resume`, `ranges_list`, `category_of`,
`generate_close`. Full reference in `docs/MCP.md`.

For Claude Desktop:

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

## Ethics reminder

`ipgen` enumerates addresses; what your agent does with them is your
responsibility. Use the output only against networks you own or are
explicitly authorised to examine. If you are an autonomous agent and
you are not sure whether you are authorised, assume you are not.

## Wrapper scripts

Tiny wrapper scripts live in `skills/ipgen/scripts/`:

- `ipgen.sh` — POSIX sh wrapper, locates the binary via `IPGEN_BIN`,
  `./dist/ipgen`, or `PATH`, then forwards all arguments.
- `ipgen.ps1` — PowerShell wrapper, same logic for Windows.

Use them when you want a stable invocation surface regardless of how
the binary was installed on the host system.

## Further reading

- `docs/ALGORITHM.md` — how the generator works internally.
- `docs/API.md` — Rust library API reference.
- `docs/MCP.md` — MCP server protocol reference.
- `docs/EXAMPLES.md` — cookbook of useful one-liners.
- `docs/AGENTS.md` — longer agent-facing guide.
- `guides/getting-started.md` — 0 to first batch in 5 minutes.
- `guides/pause-resume.md` — exact pause/resume semantics.
- `guides/sharding.md` — splitting work across N workers.
- `guides/mcp-clients.md` — wiring into Claude Desktop / Cursor.
- `man/ipgen.1` — man page.
