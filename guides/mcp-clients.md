# Wiring `ipgen-mcp` into MCP clients

The Model Context Protocol (MCP) is a JSON-RPC 2.0 protocol that lets AI
agents invoke tools over a stdio or HTTP transport. The `ipgen-mcp`
binary is a stdio MCP server: it reads JSON-RPC messages from stdin
(one per line) and writes responses to stdout (one per line), keeping all
session state in memory. Logs go to stderr only — never stdout — so
clients can parse stdout cleanly.

This guide shows how to register `ipgen-mcp` with the most common MCP
clients: Claude Desktop, generic `mcpServers` JSON config, Cursor, and a
hand-rolled client in any language. The full tool catalogue is documented
in [`docs/MCP.md`](../docs/MCP.md); this guide is the "how do I plug it
in" companion.

## What an MCP client needs to provide

Every MCP client needs to know three things:

1. The **absolute path** to the `ipgen-mcp` binary. (Or a name that is on
   the user's `PATH`, but absolute paths are safer — they avoid
   `PATH`-lookup surprises in non-interactive shells.)
2. An **empty argument list**, unless you want to override defaults.
   `ipgen-mcp` does not currently accept any CLI arguments; all session
   state is in-memory and per-client.
3. An **environment** if you want to influence logging. `RUST_LOG` is
   respected; otherwise the server is silent by default.

That is it. No port, no URL, no API key, no auth — the server is local
to the client process.

## Claude Desktop (macOS)

`Claude Desktop` reads its configuration from
`~/Library/Application Support/Claude/claude_desktop_config.json`.
Open it (create it if it does not exist) and add an `ipgen` entry to
`mcpServers`:

```json
{
  "mcpServers": {
    "ipgen": {
      "command": "/Users/yourname/bin/ipgen-mcp",
      "args": []
    }
  }
}
```

Save, fully quit Claude (`Cmd-Q`), and reopen. In any conversation you
should now be able to ask Claude things like:

> Use the ipgen tool to generate 5 public IPv4 addresses, then classify
> each one.

Claude will go through the `initialize` → `tools/list` → `tools/call`
handshake on your behalf. The first call may take a second because
Claude has to fetch the tool catalogue; subsequent calls are
sub-millisecond.

To verify the server is actually loaded, ask Claude "what MCP tools are
available?" and look for `generate_start`, `generate_next`,
`generate_status`, etc. in the response.

## Claude Desktop (Windows)

The config file lives at
`%APPDATA%\Claude\claude_desktop_config.json`. Edit it with Notepad or
VS Code (not a rich-text editor — JSON does not survive word processors):

```json
{
  "mcpServers": {
    "ipgen": {
      "command": "C:\\Users\\yourname\\bin\\ipgen-mcp.exe",
      "args": []
    }
  }
}
```

Note the double backslashes — JSON requires escaping them. Save, fully
quit Claude (right-click the tray icon → Quit), reopen, and verify the
tools are loaded as above.

## Generic `mcpServers` JSON

Any client that follows the standard `mcpServers` schema (Cursor,
Continue, Cline, etc.) accepts the same shape. The only difference is
where the config file lives; consult the client's docs. Example:

```json
{
  "mcpServers": {
    "ipgen": {
      "command": "/absolute/path/to/ipgen-mcp",
      "args": [],
      "env": {
        "RUST_LOG": "info"
      }
    }
  }
}
```

The optional `env` block sets environment variables for the server
process. `RUST_LOG=info` enables informational logging on stderr (which
the client typically surfaces in its own log pane). Useful when you are
debugging a misbehaving session.

## Cursor

Cursor reads MCP servers from `.cursor/mcp.json` in the workspace root
or from `~/.cursor/mcp.json` for global configuration. The schema is
the same:

```json
{
  "mcpServers": {
    "ipgen": {
      "command": "/usr/local/bin/ipgen-mcp",
      "args": []
    }
  }
}
```

After saving, restart Cursor and open a chat. You should see "ipgen"
listed under the available MCP servers in the chat's tool palette.

## Cline (VS Code extension)

Cline reads `cline_mcp_settings.json` in its settings directory (the
exact path varies by OS; the extension will tell you on first run). The
schema is the same `mcpServers` shape. Add the `ipgen` block and reload
the VS Code window.

## A hand-rolled client in 20 lines of Python

If your client is a custom agent or you want to drive `ipgen-mcp` from
a script, the protocol is small enough to implement directly. Here is a
minimal Python client:

```python
import json, subprocess, sys

proc = subprocess.Popen(
    ["/absolute/path/to/ipgen-mcp"],
    stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
    text=True, bufsize=1,
)

def rpc(method, params=None, *, want_response=True):
    msg = {"jsonrpc": "2.0", "method": method}
    if params is not None:
        msg["params"] = params
    if want_response:
        msg["id"] = rpc.next_id
        rpc.next_id += 1
    proc.stdin.write(json.dumps(msg) + "\n")
    proc.stdin.flush()
    if not want_response:
        return None
    line = proc.stdout.readline()
    return json.loads(line)

rpc.next_id = 1

# 1. Handshake.
rpc("initialize", {"protocolVersion": "2024-11-05", "capabilities": {},
                    "clientInfo": {"name": "demo", "version": "0.0.1"}})
rpc("notifications/initialized", {}, want_response=False)

# 2. Start a session.
r = rpc("tools/call", {
    "name": "generate_start",
    "arguments": {"config": {"preset": "public", "order": "permuted",
                            "seed": 42}},
})
session_id = json.loads(r["result"]["content"][0]["text"])["session_id"]
print("session:", session_id)

# 3. Pull a batch.
r = rpc("tools/call", {"name": "generate_next",
                       "arguments": {"session_id": session_id, "count": 5}})
items = json.loads(r["result"]["content"][0]["text"])["items"]
for it in items:
    print(it)

# 4. Pause and resume.
r = rpc("tools/call", {"name": "generate_pause",
                       "arguments": {"session_id": session_id}})
ckpt = json.loads(r["result"]["content"][0]["text"])
r = rpc("tools/call", {"name": "generate_resume",
                       "arguments": {"checkpoint": ckpt}})
print("resumed:", r["result"]["content"][0]["text"])

proc.stdin.close()
proc.wait()
```

Run it:

```bash
python client.py
# session: 6abcc4f6-...
# {'index': 0, 'address': '203.0.113.42', 'category': 'public'}
# {'index': 1, 'address': '8.8.4.4', 'category': 'public'}
# ...
# resumed: {"session_id":"6abcc4f6-...","cursor":5,"total":3702258433}
```

This is exactly the protocol Claude Desktop itself speaks; the client
above is just the minimum viable wrapper. See
[`examples/mcp-transcript.json`](../examples/mcp-transcript.json) for a
complete worked transcript.

## Troubleshooting

**"MCP server ipgen failed to start"** — the path in the config file is
wrong, the binary is not executable, or the OS refused to run it for
some reason (Gatekeeper on macOS, SmartScreen on Windows). Run the
binary directly from a shell to verify:

```bash
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}' \
  | /path/to/ipgen-mcp
```

You should see one JSON line back. If you see nothing, the binary did
not start; check file permissions and the path.

**"Method not found" errors in the client log** — your client is using
a method name that the server does not implement. The current method
set is `initialize`, `ping`, `tools/list`, `tools/call`. Resources and
prompts are not implemented.

**"Unknown tool" errors when calling** — you are calling a tool name
the server does not know. The tool names are `generate_start`,
`generate_next`, `generate_status`, `generate_pause`, `generate_resume`,
`ranges_list`, `category_of`, `generate_close`. The original spec used
`ipgen_*` names; the shipped binary uses the `generate_*` family —
check [`docs/MCP.md`](../docs/MCP.md) for the canonical list.

**Server hangs forever on `tools/call`** — you sent a `id` field (so the
server expects to respond) but the tool raised an unexpected error. Kill
the process and file a bug with the input that triggered it; the server
is supposed to always return either a result or an error for any request
with an `id`.

**stderr noise is breaking your client** — `ipgen-mcp` writes nothing
to stderr by default. If you set `RUST_LOG=info`, you will get a few
informational lines per session. If you see stack traces, those are
bugs; please report them.

## Reference: the handshake

For protocol nerds, the canonical MCP handshake is:

1. Client sends `initialize` with `protocolVersion`,
   `capabilities`, `clientInfo`.
2. Server responds with `protocolVersion`, `capabilities`,
   `serverInfo`.
3. Client sends `notifications/initialized` (no `id`, no response).
4. Client can now call `tools/list`, `tools/call`, `ping`, etc.

`ipgen-mcp` accepts any of the recent protocol versions; it always
responds with `"2024-11-05"`, the version it was implemented against.
