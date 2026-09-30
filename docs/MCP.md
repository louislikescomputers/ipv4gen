# `ipgen-mcp` — MCP Server Reference

`ipgen-mcp` is a Model Context Protocol server over stdio. It reads
JSON-RPC 2.0 messages from stdin (one per line) and writes responses
to stdout (one per line). Logs go to stderr only — never on stdout —
so any MCP-compatible client can parse the responses cleanly.

This document is the protocol reference: the supported methods, the
tool catalogue, the schemas for inputs and outputs, and the error
codes. For client-side wiring (Claude Desktop, Cursor, generic
`mcpServers` JSON, hand-rolled client), see
[`guides/mcp-clients.md`](../guides/mcp-clients.md). For a worked
transcript, see [`examples/mcp-transcript.json`](../examples/mcp-transcript.json).

## Protocol details

- **Transport:** stdio (newline-delimited JSON-RPC 2.0).
- **Protocol version:** `2024-11-05` (the server accepts any recent
  version in `initialize` and responds with `2024-11-05`).
- **Server info:** `{"name": "ipgen-mcp", "version": "<crate version>"}`.
- **Capabilities:** `{"tools": {}}`. Resources and prompts are not
  implemented.
- **Sessions:** in-memory `HashMap<String, Session>`. Each session
  holds a live `Generator` plus its last `Checkpoint`. Sessions vanish
  when the server process exits.
- **No async runtime.** The server is a blocking `stdin.lock().lines()`
  loop; one OS thread handles all requests. This is enough for the
  expected workload (an agent calls a tool every few seconds).

## Methods

| Method                       | Has response? | Purpose                              |
|------------------------------|---------------|--------------------------------------|
| `initialize`                 | yes           | Handshake: exchange versions and capabilities. |
| `notifications/initialized` | no            | Client signals end of handshake. No response. |
| `ping`                       | yes           | Empty-result heartbeat.              |
| `tools/list`                 | yes           | Return the tool catalogue.           |
| `tools/call`                 | yes           | Invoke a tool.                       |

Unknown methods return a JSON-RPC error with code `-32601`
("method not found"). Malformed JSON returns `-32700` ("parse error")
with `id: null`. Invalid params (missing required field, wrong type)
return `-32602` ("invalid params"). Tool execution errors are returned
as `tools/call` results with `isError: true`, not as JSON-RPC errors —
this preserves the MCP distinction between protocol-level problems and
tool-level problems.

## Tool catalogue

Eight tools are exposed. All inputs and outputs are JSON. `tools/call`
wraps tool output as `{"content": [{"type": "text", "text": "<json>"}], "isError": <bool>}`,
where `<json>` is a JSON string of the tool's actual result. Clients
must `JSON.parse` (or equivalent) the `text` field to get the
structured payload.

### 1. `generate_start`

Start a generation session from a config object. Returns the session
id and totals. The session lives in memory until the server exits or
`generate_close` is called.

**Input schema:**

```json
{
  "config": {
    "preset": "public|private|all|public+private",
    "categories": ["string", ...],          // overrides preset
    "include": ["CIDR|RANGE|ADDR", ...],
    "exclude": ["CIDR|RANGE|ADDR", ...],
    "order": "sequential|permuted|blocks",
    "seed": 0,                              // integer
    "prefix_len": 24,                       // 8..=32, for blocks order
    "shard": "K/M"                          // pattern ^[0-9]+/[0-9]+$
  }
}
```

**Output:**

```json
{
  "session_id": "6abcc4f6-161b-00000000",
  "total_addresses": 3702258433,
  "shard_total": 3702258433,
  "order": "permuted",
  "config_hash": "0xe07b9a4d3c5f1a01"
}
```

### 2. `generate_next`

Pull the next batch of items (addresses for `sequential`/`permuted`,
CIDR blocks for `blocks`). The count must be in `[1, 100000]`.

**Input:**

```json
{
  "session_id": "...",
  "count": 1000
}
```

**Output (sequential or permuted):**

```json
{
  "items": [
    {"index": 0, "address": "1.0.0.0", "category": "public"},
    {"index": 1, "address": "1.0.0.1", "category": "public"}
  ],
  "cursor": 2,
  "total": 3702258433
}
```

**Output (blocks):**

```json
{
  "items": [
    {"index": 0, "cidr": "1.0.0.0/24", "count": 256},
    {"index": 1, "cidr": "8.8.4.0/24", "count": 256}
  ],
  "cursor": 2,
  "total": 14481479
}
```

After every `generate_next`, the server updates the session's
checkpoint in memory so a `generate_pause` immediately afterwards
returns a checkpoint at the latest cursor.

### 3. `generate_status`

Get progress without pulling items.

**Input:** `{"session_id": "..."}`

**Output:**

```json
{
  "session_id": "...",
  "cursor": 200,
  "total": 3702258433,
  "remaining": 3702258233,
  "percent_complete": 0.0000054,
  "order": "permuted",
  "config_hash": "0xe07b9a4d3c5f1a01"
}
```

### 4. `generate_pause`

Snapshot the session as a `Checkpoint` and return it. The session stays
in memory; you can continue calling `generate_next` afterwards. Useful
when you want to persist a session externally (e.g. to a database) so
it can be resumed later even after the server exits.

**Input:** `{"session_id": "..."}`

**Output:** the full `Checkpoint` object as JSON (same shape as the
file written by the CLI's `--state` option).

### 5. `generate_resume`

Resume a session from a checkpoint previously returned by
`generate_pause` or written by the CLI.

**Input:** `{"checkpoint": { ... checkpoint object ... }}`

**Output:**

```json
{
  "session_id": "...",
  "cursor": 200,
  "total": 3702258433
}
```

If the checkpoint's config hash does not match the recomputed hash, the
call returns `isError: true` with a message about hash mismatch. This
is the same tamper detection the CLI uses on `ipgen resume`.

### 6. `ranges_list`

List the CIDR blocks covering the requested categories. Useful for
figuring out what a `generate` run will cover before committing to it.

**Input:**

```json
{
  "categories": ["public", "private"]
}
```

If `categories` is omitted or empty, all 13 categories are listed.

**Output:**

```json
{
  "cidrs": ["10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16", ...],
  "total_addresses": 3702258433
}
```

### 7. `category_of`

Classify a single IPv4 address.

**Input:** `{"address": "8.8.8.8"}`

**Output:** `{"address": "8.8.8.8", "category": "public"}`

Returns `isError: true` with `BadAddress` if the address is not a valid
dotted-quad IPv4 string.

### 8. `generate_close`

Drop a session from the server's memory.

**Input:** `{"session_id": "..."}`

**Output:** `{"closed": "..."}`

After this call the session id is no longer valid; a subsequent
`generate_next` for the same id will fail with "unknown session".

## Tool input schema reference

The config schema for `generate_start` (also returned by `tools/list`
as the `inputSchema` of the tool):

```json
{
  "type": "object",
  "properties": {
    "preset":      {"type": "string", "enum": ["public","private","all","public+private"]},
    "categories":  {"type": "array", "items": {"type": "string"}},
    "include":     {"type": "array", "items": {"type": "string"}},
    "exclude":     {"type": "array", "items": {"type": "string"}},
    "order":       {"type": "string", "enum": ["sequential","permuted","blocks"]},
    "seed":        {"type": "integer"},
    "prefix_len":  {"type": "integer", "minimum": 8, "maximum": 32},
    "shard":       {"type": "string", "pattern": "^[0-9]+/[0-9]+$"}
  }
}
```

Only `config` itself is required for `generate_start`. All fields of
`config` are optional and have sensible defaults (preset `public`,
order `permuted`, seed `0`, shard `0/1`, prefix_len `24` for blocks).

## Error codes

JSON-RPC defines the error codes the server uses:

- `-32700` — parse error (the line was not valid JSON).
- `-32601` — method not found.
- `-32602` — invalid params (missing required field, wrong type, etc.).
- `-32603` — internal error (the server bug; should not happen).

Tool-level errors are **not** JSON-RPC errors; they are `tools/call`
results with `isError: true` and a `text` field describing the
problem. Examples:

- `unknown tool: <name>` — `tools/call` with a `name` the server does
  not know.
- `missing session_id` — calling `generate_next`, `generate_status`,
  `generate_pause`, or `generate_close` without a `session_id`.
- `unknown session: <sid>` — calling a session-scoped tool with an id
  that does not exist (was never created or was closed).
- `invalid CIDR notation: '<spec>'` — bad include/exclude spec in
  `generate_start`.
- `the selection contains no addresses` — empty selection (e.g.
  excluding everything).
- `config hash mismatch` — `generate_resume` with a tampered checkpoint.
- `index 0 out of range for 100000 addresses` — `generate_next` with
  `count` outside `[1, 100000]`.

## A minimal session, end-to-end

```text
→ {"jsonrpc":"2.0","id":1,"method":"initialize",
   "params":{"protocolVersion":"2024-11-05","capabilities":{},
             "clientInfo":{"name":"demo","version":"0.0.1"}}}
← {"jsonrpc":"2.0","id":1,
   "result":{"protocolVersion":"2024-11-05",
             "capabilities":{"tools":{}},
             "serverInfo":{"name":"ipgen-mcp","version":"0.1.0"}}}

→ {"jsonrpc":"2.0","method":"notifications/initialized"}
   (no response)

→ {"jsonrpc":"2.0","id":2,"method":"tools/call",
   "params":{"name":"generate_start",
             "arguments":{"config":{"preset":"public","order":"permuted","seed":42}}}}
← {"jsonrpc":"2.0","id":2,
   "result":{"content":[{"type":"text","text":"{\"session_id\":\"...\",\"total_addresses\":3702258433,...}"}],
             "isError":false}}

→ {"jsonrpc":"2.0","id":3,"method":"tools/call",
   "params":{"name":"generate_next",
             "arguments":{"session_id":"...","count":5}}}
← {"jsonrpc":"2.0","id":3,
   "result":{"content":[{"type":"text","text":"{\"items\":[...],\"cursor\":5,\"total\":3702258433}"}],
             "isError":false}}

→ {"jsonrpc":"2.0","id":4,"method":"tools/call",
   "params":{"name":"generate_close","arguments":{"session_id":"..."}}}
← {"jsonrpc":"2.0","id":4,
   "result":{"content":[{"type":"text","text":"{\"closed\":\"...\"}"}],
             "isError":false}}
```

A full worked transcript including pause, resume, and an error case
lives in [`examples/mcp-transcript.json`](../examples/mcp-transcript.json).

## Design notes

- **No persistence.** Sessions live in memory only. If you need to
  survive a server restart, call `generate_pause`, stash the
  returned checkpoint somewhere durable (file, database, KV store),
  and call `generate_resume` on the next start.
- **No concurrency.** The server is single-threaded. Long-running
  `generate_next` calls block the stdin loop; keep batch sizes
  reasonable (10k–100k addresses max) so the server stays responsive.
- **No streaming.** Each `tools/call` returns the full batch in one
  response. If you need millions of addresses, call `generate_next`
  repeatedly in a loop.
- **No `--state-dir`.** The MCP server does not currently expose a
  filesystem-based session store. If you need that, use the CLI; if
  you need it from an agent, write a wrapper that calls the CLI via
  `tools/call` on a generic shell-exec tool.

## Differences from the original spec

The original masterprompt (see `ipgen-masterprompt.md` in the source
tree) listed a slightly different tool set: `ipgen_create_session`,
`ipgen_next`, `ipgen_pause`, `ipgen_resume`, `ipgen_status`,
`ipgen_seek`, `ipgen_list_sessions`, `ipgen_close`, `ipgen_classify`,
`ipgen_count`, `ipgen_ranges`. The shipped binary implements a
slightly simplified version: `generate_start`, `generate_next`,
`generate_status`, `generate_pause`, `generate_resume`,
`generate_close`, `ranges_list`, `category_of`. The differences are:

- Names use `generate_*` instead of `ipgen_*` for brevity.
- `ipgen_seek` is not exposed (you can achieve the same by editing the
  checkpoint JSON before calling `generate_resume`).
- `ipgen_list_sessions` is not exposed (sessions are in-memory and
  ephemeral; a caller knows what it created).
- `ipgen_count` is not exposed (the count is part of the
  `generate_start` and `generate_status` responses).

These differences are intentional simplifications for the v0.1 release.
