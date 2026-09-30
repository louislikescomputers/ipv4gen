# Getting Started with `ipgen`

This guide takes you from a fresh clone of the repository to your first
batch of generated IPv4 addresses in under five minutes. It assumes you have
a POSIX-like shell (Linux, macOS, WSL, or any Unix) and have already
installed the Rust toolchain. If you have not installed Rust yet, jump to
[`installation.md`](installation.md) first and come back here.

The goal of this guide is not to exhaustively cover every flag — that is
what [`docs/EXAMPLES.md`](../docs/EXAMPLES.md) is for — but to give you a
hands-on feel for what the tool does, how its subcommands fit together, and
why pause/resume is the central design decision behind everything else it
does. By the end you will have generated a few real batches of addresses,
inspected a checkpoint file, and resumed an interrupted run.

## Step 1 — Build the binaries

From the repository root:

```bash
./compile.sh
```

That command runs `cargo build --release -p ipgen-cli -p ipgen-mcp` and then
copies the two resulting binaries into `bin/<os>/`. On Linux you end up with
`bin/linux/ipgen` and `bin/linux/ipgen-mcp`; on macOS the directory is
`bin/darwin/`. Windows users should run `compile.bat` or `compile.ps1`
instead, which produce `bin/windows/ipgen.exe` and `bin/windows/ipgen-mcp.exe`.

Verify the binaries exist and are executable:

```bash
ls -l bin/linux/
# -rwxr-xr-x ... ipgen
# -rwxr-xr-x ... ipgen-mcp
```

For convenience, put the binary on your `PATH` or `cd` into the platform
directory. The rest of this guide assumes `ipgen` is reachable on `PATH`.

## Step 2 — Inspect the built-in category table

`ipgen` partitions all 2³² IPv4 addresses into 13 disjoint categories
matching the IANA special-purpose registry. The `categories` subcommand
prints them with their sizes:

```bash
ipgen categories
```

You will see output roughly like this (sizes are exact; percentages are
against the full 2³² space):

```
this-network     16777216   0.3906%
private          16777216   0.3906%   # 10/8 + 172.16/12 + 192.168/16
shared-cgnat      4194304   0.0977%
loopback         16777216   0.3906%
link-local          65536   0.0015%
ietf-protocol        1024   0.0000%
documentation       3072   0.0000%
deprecated-6to4      1024   0.0000%
benchmarking       131072   0.0031%
multicast         268435456   6.2500%
reserved         268435455   6.2500%   # 240/4 minus the broadcast address
broadcast               1   0.0000%   # 255.255.255.255/32
public          3702258433  86.1819%
TOTAL           4294967296 100.0000%
```

Notice that the sizes sum exactly to `2³² = 4,294,967,296`. That is not a
coincidence; the project's test suite enforces it. The `--detailed` flag
dumps the actual CIDR list per category, useful when you are composing
custom includes or excludes.

## Step 3 — Generate your first batch

Generate five public addresses in permuted order (the default order):

```bash
ipgen generate --preset public --count 5
```

Sample output (your addresses will differ because the seed defaults to `0`
but the permutation is keyed; specify `--seed` for reproducibility):

```
203.0.113.42
8.8.4.4
198.51.100.7
1.1.1.1
172.16.5.20
```

Wait — `198.51.100.0/24` is a documentation range, not public! Let's try
again with sequential order to see the actual sorted sequence:

```bash
ipgen generate --preset public --order sequential --count 5
```

```
1.0.0.0
1.0.0.1
1.0.0.2
1.0.0.3
1.0.0.4
```

The first public address is `1.0.0.0` because `0.0.0.0/8` ("this network on
this host") is excluded from the public set. The 13 categories really do
partition the whole space, so the public set starts where the special-use
space ends.

## Step 4 — Try a different output format

Three formats ship out of the box: `text` (one IP per line), `jsonl` (one
JSON object per line with index, address, and category), and `csv` (with a
header row). JSON Lines is the friendliest for piping into `jq` or another
tool that expects structured records:

```bash
ipgen generate --preset public --order sequential --count 3 --format jsonl
```

```json
{"index":0,"address":"1.0.0.0","category":"public"}
{"index":1,"address":"1.0.0.1","category":"public"}
{"index":2,"address":"1.0.0.2","category":"public"}
```

CSV is useful for spreadsheets or for `awk`-based post-processing:

```bash
ipgen generate --preset public --order sequential --count 3 --format csv
```

```csv
index,address,category
0,1.0.0.0,public
1,1.0.0.1,public
2,1.0.0.2,public
```

## Step 5 — Pause and resume

This is the headline feature. Start a long run and pipe it to a file:

```bash
ipgen generate --preset public --order permuted --seed 7 \
  --out addrs.txt --checkpoint-every 100
```

While it is running, hit `Ctrl-C`. You will see something like:

```
^C
ipgen: paused after 123456 more items (cursor 123456/3702258433) — state saved to ipgen.state.json
```

Inspect the checkpoint:

```bash
ipgen status --state ipgen.state.json
```

```
session_id:  6abcc4f6-161b-00000000
config_hash: 0x8b3e1f4c2a5b6e1f
order:       permuted
shard:       0/1
cursor:      123456 / 3702258433 (0.0033% complete)
remaining:   3702134977
created_at:  1790756086
updated_at:  1790756086
```

Resume with the `resume` subcommand — the output file keeps appending, and
there will be no duplicates and no gaps relative to the uninterrupted run:

```bash
ipgen resume --state ipgen.state.json --out addrs.txt
```

The mechanism is simple: the entire generator state is `(config, cursor)`,
serialised as JSON. The `resume` subcommand loads the checkpoint, validates
the stored content hash (to detect tampering or corruption), rebuilds the
in-memory generator, and continues from the saved cursor. See
[`pause-resume.md`](pause-resume.md) for the full treatment.

## Step 6 — Use the merged-CIDR view

Sometimes you do not want to generate addresses; you want to know what the
*allowed set* looks like as a list of CIDRs. That is what the `ranges`
subcommand is for:

```bash
# All private-use space as a compact summary.
ipgen ranges --preset private --summary
# {"cidr_count":3,"total_addresses":16777216}

# Same thing, but print the actual CIDRs:
ipgen ranges --preset private
# 10.0.0.0/8
# 172.16.0.0/12
# 192.168.0.0/16
```

Combine `--include` and `--exclude` to compute arbitrary merged sets:

```bash
ipgen ranges --preset public --exclude 8.8.0.0/16 --summary
```

This is invaluable for figuring out what your `generate` run will cover
before you commit to it.

## Step 7 — Try a single-blocks run

The `blocks` order iterates aligned `/N` blocks (default `/24`, configurable
via `--prefix-len`) in permuted order. Each emitted line is a CIDR plus the
count of allowed addresses that fall inside that block:

```bash
ipgen generate --preset public --order blocks --prefix-len 24 --count 3
```

```
203.0.113.0/24
8.8.4.0/24
198.51.100.0/24
```

(The trailing count column only appears in `jsonl` and `csv` formats; the
text format emits the CIDR alone for easy piping into `nmap -iL`.)

## Step 8 — Pipe into something useful

Because `ipgen` is a plain stdout producer with sensible broken-pipe
handling, it composes cleanly with the rest of the Unix toolbox. To extract
the first 1,000 addresses and write them to a file:

```bash
ipgen generate --preset public --order sequential --count 1000 > first-1k.txt
```

To parallelise with `xargs`:

```bash
ipgen generate --preset public --order permuted --count 10000 \
  | xargs -P 8 -I{} -n 1 ./your-worker {}
```

To shard across four workers (see [`sharding.md`](sharding.md) for details):

```bash
for k in 0 1 2 3; do
  ipgen generate --preset public --order permuted --shard $k/4 \
    --out shard-$k.txt &
done
wait
```

## Where to go next

- [`installation.md`](installation.md) — full per-OS build instructions,
  including cross-compilation.
- [`pause-resume.md`](pause-resume.md) — exactly how the checkpoint
  mechanism works and how to integrate it into a larger pipeline.
- [`sharding.md`](sharding.md) — splitting a run across N workers.
- [`docs/EXAMPLES.md`](../docs/EXAMPLES.md) — cookbook of useful one-liners
  including the canonical "generate all public IPv4 addresses in order".
- [`docs/MCP.md`](../docs/MCP.md) — driving the tool from an AI agent via
  the stdio MCP server.
