# Agent Guide: Using `ipgen` from AI agents

This document is for AI agents (LLM-driven coding assistants, autonomous
scanners, CI/CD bots, etc.) that need to enumerate IPv4 addresses as
part of their workflow. It is a working reference for the agent-facing
interface: when to reach for `ipgen`, when to use the CLI vs the MCP
server, how to choose batches and orders, how to handle very long
runs, and how to integrate pause/resume into an agentic loop that may
be killed and restarted at any moment.

The short version: `ipgen` is a pure enumeration utility. It produces
IPv4 addresses, CIDRs, and `/prefix` blocks, in deterministic or
pseudo-random order, with exact pause/resume. It does no networking.
Use it when you need a stream of addresses to feed into something
else — a scanner, a profiler, a research script, a database populator.

## When to use `ipgen`

Use `ipgen` when your agent needs to:

- **Enumerate a large IPv4 set** (anything from a `/24` to the full
  public space, 3.7 billion addresses) and feed the results to a
  downstream tool.
- **Resume an interrupted enumeration** without duplicating or
  skipping addresses. The state is one JSON file; load it and continue.
- **Spread work across multiple workers** (parallel agents, multiple
  machines, a cron-driven fleet) with no coordination overhead.
- **Classify addresses** into IANA categories (public, private,
  loopback, etc.) without reimplementing RFC 6890.
- **Produce a deterministic, reproducible stream** for testing or for
  A/B comparisons of downstream tools.

Do **not** use `ipgen` if:

- You need a single random address. `python -c "import random; print('.'.join(str(random.randint(0,255)) for _ in range(4)))"`
  is simpler.
- You need to *scan* a network. `ipgen` only enumerates; it does
  no probing, no sockets, no ICMP. Pair it with `nmap`, `masscan`,
  `zmap`, or your own scanning tool.
- You need IPv6. `ipgen` is IPv4-only by design; an IPv6 version
  would need a different address model (`u128` instead of `u32`).
- You need *filtered* enumeration (e.g. "all addresses that respond to
  ping"). That requires actual network I/O, which `ipgen` does not do.

## CLI vs MCP: which to use

`ipgen` ships two interfaces: the `ipgen` CLI and the `ipgen-mcp`
stdio server. Each is appropriate for different agent contexts.

### Use the CLI when

- Your agent is **running in a shell** (Bash, Zsh, PowerShell). The
  CLI is just a subprocess; you spawn it, you read its stdout, you
  handle its exit code.
- You need **long-running enumerations** that may exceed the agent's
  own session lifetime. Start `ipgen generate` in the background,
  write the checkpoint, kill the agent, restart it, call `ipgen resume`.
- You need **integration with Unix tools** — `awk`, `sort`, `jq`,
  `gzip`, `xargs`, `parallel`. The CLI is a stdin/stdout pipe citizen.
- You are **debugging**. The CLI has helpful stderr output and clean
  exit codes; the MCP server is silent on stdout (only JSON-RPC
  responses belong there).

### Use the MCP server when

- Your agent is an **MCP-aware client** (Claude Desktop, Cursor,
  Cline, or a custom agent that speaks MCP). The tool catalogue is
  directly callable as `tools/call` with structured arguments and
  structured results.
- You need **structured output** (not lines of text) and the MCP
  client's tool-result parsing is more convenient than parsing CSV or
  JSONL.
- You want the agent to manage **session lifecycle** itself — start,
  pull batches, pause, resume, close — without leaving the agent's
  own process.
- You want **concurrent sessions** in the same server process
  (each session has a unique id and lives in memory).

A reasonable rule of thumb: if your agent is in a shell, use the CLI;
if your agent is an MCP client, use the MCP server. Both expose the
same generator and the same categories.

## Choosing the order

| Order        | Use when                                                       |
|--------------|----------------------------------------------------------------|
| `sequential` | You want ascending order, or you want maximum throughput, or you want the output to be reproducible without specifying a seed. |
| `permuted`   | You want the spread of pseudo-random order without the cost of materialising a full permutation, or you want different workers (via `--seed`) to walk the same space differently. |
| `blocks`     | You want aligned `/N` blocks (default `/24`) rather than individual addresses, e.g. for block-level mapping or to feed into `nmap -iL`. |

For survey-style workloads, the **default `permuted` order** is almost
always right. Sequential order hammers one subnet at a time, which
concentrates load on a single network and is generally considered rude
(even when you own the network). Permuted order spreads the addresses
across the whole space, which is both more polite and more
representative of "real" internet traffic.

## Choosing the batch size

The `--batch` flag (CLI) and `count` argument (MCP `generate_next`)
control how many addresses are computed per internal call. Larger
batches are more efficient but require more memory.

- **CLI: `--batch 4096` (default)** is a reasonable starting point
  for most workloads. It is small enough that the buffer fits in L2
  cache and the checkpoint cadence stays fine-grained.
- **CLI: `--batch 65536` or larger** for high-throughput runs to a
  file. The bulk-fill fast path scales linearly with batch size.
- **MCP: `count` in `[1, 100000]`** — pick `1000` to `10000` for
  agent-driven iterations; you want the response to fit comfortably
  in the agent's context window.
- **One-at-a-time (`--batch 1`)** only when you need
  per-address checkpoint granularity and you do not care about
  throughput.

## Pause and resume in an agentic loop

The headline feature. Here is the pattern for an agent that may be
killed at any moment:

1. **Check for an existing checkpoint.** If it exists, call `ipgen
   resume --state <path>`. If not, call `ipgen generate ...` with
   `--state <path>` and `--checkpoint-every N`.
2. **Run for a bounded time.** Either set `--count` to a finite
   number, or wrap the run in a `timeout` invocation, or have the
   agent's parent process kill the run after a wall-clock budget.
3. **When the run stops** (Ctrl-C, timeout, normal exit), the
   checkpoint is already on disk. Read it with `ipgen status` to
   see the cursor and total.
4. **Repeat.** On the next iteration, the agent notices the
   checkpoint, calls `ipgen resume`, and continues from the saved
   cursor.

In pseudocode:

```python
import subprocess, json, os

STATE = "/var/run/myagent/ipgen.state.json"
OUT   = "/var/run/myagent/addresses.txt"

while True:
    if os.path.exists(STATE):
        cmd = ["ipgen", "resume", "--state", STATE, "--out", OUT,
               "--count", "100000", "--format", "jsonl"]
    else:
        cmd = ["ipgen", "generate", "--preset", "public",
               "--order", "permuted", "--seed", "42",
               "--count", "100000", "--format", "jsonl",
               "--state", STATE, "--out", OUT]
    subprocess.run(cmd, check=True, timeout=300)

    status = subprocess.run(["ipgen", "status", "--state", STATE],
                            capture_output=True, text=True)
    # parse status, decide if done
    if done:
        break
```

This loop is safe to kill at any point — between iterations, during a
`generate` call, during a `resume` call, even during a `status` call.
The worst case is that you lose the last few addresses of an
in-flight batch, which the next resume will re-emit.

## Sharding for parallel agents

If your workflow involves multiple agents (or multiple processes),
sharding lets them split the work without any coordination. Each
agent gets a `--shard K/M` argument; together the agents cover the
full stream with no overlaps.

```python
# Agent 0 of 4
subprocess.run(["ipgen", "generate", "--preset", "public",
                "--order", "permuted", "--seed", "42",
                "--shard", "0/4", "--out", "shard-0.txt",
                "--state", "shard-0.state.json"])

# Agent 1 of 4 (in a different process, possibly on a different machine)
subprocess.run(["ipgen", "generate", "--preset", "public",
                "--order", "permuted", "--seed", "42",
                "--shard", "1/4", "--out", "shard-1.txt",
                "--state", "shard-1.state.json"])
# ...
```

Each shard has its own checkpoint and can be resumed independently.
The shards are deterministic given the seed, so two runs with the same
seed and the same `M` produce the same partition (though the
within-shard order depends on `K`).

## Batch-size guidance for agents

If you are an AI agent calling `ipgen` from a tool-execution loop,
the batch size you pick has a direct effect on your own context
window. Each emitted address in JSON-Lines format is about 70 bytes
(`{"index":0,"address":"1.2.3.4","category":"public"}`). At 1000
addresses per batch, that is ~70 KB of tool output per call — usually
fine. At 100000 addresses per batch, it is ~7 MB, which will
saturate most agents' context.

Recommended cadence:

- **Initial exploration**: `--count 10` to `--count 100`. Just to
  see what the stream looks like.
- **Production enumeration**: `--count 1000` to `--count 10000`. Big
  enough to amortise per-call overhead, small enough to fit in
  context.
- **Bulk transfer to a file**: `--count 100000` and write to a file
  with `--out`. You will not see the output in your context; you
  will see the cursor advance in the checkpoint.

## Output-format guidance

- **`text`** for piping into other Unix tools (`awk`, `sort`, `xargs`)
  or for human inspection. One address per line, no metadata.
- **`jsonl`** for structured processing in `jq`, Python, JavaScript,
  or your agent's own JSON parser. One JSON object per line with
  `index`, `address`, `category`.
- **`csv`** for spreadsheets, databases, and `awk`-style post-
  processing where column structure matters. Has a header row
  (`index,address,category`).
- **`blocks` order + `text` format** for `nmap -iL` and other tools
  that accept a CIDR list.

## Common pitfalls for agents

- **Forgetting `--state`** — the default checkpoint file is
  `ipgen.state.json` in the current working directory. If your
  agent runs in different working directories across invocations,
  you will lose track of the checkpoint. Always pass `--state` with
  an absolute path.
- **Forgetting `--out` on resume** — if you used `--out addrs.txt`
  on the original run, you must use the same `--out addrs.txt` on
  resume, otherwise the file will be created but the existing output
  will not be appended to.
- **Mismatched seed across runs** — the `--seed` is part of the
  config; changing it invalidates the checkpoint. The `resume`
  subcommand refuses to load a checkpoint whose config hash does
  not match (this is intentional). If you want a different stream,
  start a fresh `generate` with a different `--state` path.
- **Mixing formats on resume** — you *can* pass `--format jsonl` on
  `resume` even if the original was `--format text`. The output file
  will then have a mixed format, which is rarely what you want.
  Stick with the same format across `generate` and `resume`.
- **Reaching for `rand` yourself** — do not write your own
  "generate a random IP" code. `ipgen`'s permuted order is a
  bijection: every address appears exactly once. A naive `rand`
  approach will produce duplicates and miss addresses, which ruins
  surveys.
- **Assuming `ipgen` does network I/O** — it does not. If you find
  yourself writing "use `ipgen` to scan X", rewrite as "use `ipgen`
  to enumerate X, then pipe into `<scanner>`".

## Worked example: a 4-agent parallel survey

Goal: enumerate the public IPv4 space in permuted order across four
parallel agents, each writing to its own file. If any agent dies,
it should be resumable.

```bash
# Coordinator: spawn four shards.
for k in 0 1 2 3; do
  ipgen generate \
    --preset public \
    --order permuted --seed 42 \
    --shard $k/4 \
    --state /var/run/survey/shard-$k.state.json \
    --out /var/run/survey/shard-$k.txt \
    --batch 65536 \
    --checkpoint-every 1000 \
    &
done
wait
```

If shard 2 dies:

```bash
ipgen resume --state /var/run/survey/shard-2.state.json \
  --out /var/run/survey/shard-2.txt \
  --batch 65536 --checkpoint-every 1000
```

The other shards are unaffected. When all four finish, the union of
the four output files is the complete public IPv4 permuted stream
(3,702,258,433 addresses, no duplicates, no gaps).

## Worked example: a chat-driven enumeration

Goal: an agent in Claude Desktop enumerates a small set of addresses,
classifies them, and reports to the user.

The agent's plan:

1. Call `ipgen categories` to enumerate the available categories.
2. Call `ipgen generate --preset public --count 5 --format jsonl` to
   get a sample.
3. Call `ipgen classify <addr>` (or just inspect the `category` field
   in the JSONL output) to label each address.
4. Summarise: "Here are 5 public addresses: 1.1.1.1 (public),
   8.8.8.8 (public), …"

For larger jobs, the agent would switch to the MCP server, call
`generate_start`, then loop on `generate_next` until the cursor
reaches the total.

## Worked example: an MCP-driven loop

Goal: an MCP-aware agent pulls 1000 addresses at a time until the
stream is exhausted, pausing and resuming across server restarts.

```python
# Pseudocode — assuming an MCP client library.
client = MCPClient(command="/path/to/ipgen-mcp")
client.initialize()

ckpt = load_checkpoint_from_db()  # your persistence layer
if ckpt:
    sid = client.call("generate_resume", {"checkpoint": ckpt})["session_id"]
else:
    r = client.call("generate_start",
                    {"config": {"preset": "public", "order": "permuted",
                                "seed": 42}})
    sid = r["session_id"]

while True:
    r = client.call("generate_next", {"session_id": sid, "count": 1000})
    items = r["items"]
    if not items:
        break
    for it in items:
        process(it["address"])  # your downstream work

    # Persist the checkpoint after every batch, so a server restart
    # loses at most one batch.
    ckpt = client.call("generate_pause", {"session_id": sid})
    save_checkpoint_to_db(ckpt)

client.call("generate_close", {"session_id": sid})
```

The MCP server keeps the session in memory; the `generate_pause` call
returns the checkpoint JSON which you persist externally. On the next
start, `generate_resume` rehydrates the session from the checkpoint.

## Ethics reminder

`ipgen` enumerates addresses; what your agent *does* with them is your
responsibility. Use the output only against networks you own or are
explicitly authorised to examine. Network scanning without
authorisation is illegal in most jurisdictions and is a violation of
the trust the maintainers extend by publishing this tool. If you are
an autonomous agent and you are not sure whether you are authorised,
assume you are not.
