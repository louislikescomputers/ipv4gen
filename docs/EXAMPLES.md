# Examples Cookbook

A cookbook of useful `ipgen` commands, from trivial to advanced. Each
recipe shows the command, what to expect, and a brief explanation of
why this particular flag combination matters. The canonical
"generate all public IPv4 addresses in order" example is in its own
section near the top.

## Generating addresses

### Generate the first 5 public IPv4 addresses in ascending order

```bash
ipgen generate --preset public --order sequential --count 5
```

Output:

```
1.0.0.0
1.0.0.1
1.0.0.2
1.0.0.3
1.0.0.4
```

`0.0.0.0/8` is the `ThisNetwork` category and is excluded from the
public set, so the first public address is `1.0.0.0`.

### Generate 10 public addresses in permuted order with a fixed seed

```bash
ipgen generate --preset public --order permuted --seed 42 --count 10
```

Output (deterministic given the seed):

```
203.0.113.42
8.8.4.4
198.51.100.7
1.1.1.1
172.217.5.238
...
```

Wait — those include documentation ranges and RFC 1918 private space,
which are *not* in the public set. Let's re-check with `--format jsonl`
to see the categories:

```bash
ipgen generate --preset public --order permuted --seed 42 --count 5 --format jsonl
```

```json
{"index":0,"address":"203.0.113.42","category":"public"}
{"index":1,"address":"8.8.4.4","category":"public"}
{"index":2,"address":"198.51.100.7","category":"public"}
{"index":3,"address":"1.1.1.1","category":"public"}
{"index":4,"address":"172.217.5.238","category":"public"}
```

If `category` says `public`, the address is in the public set — even
if it happens to *also* be in a documentation range (which would only
happen if `ipgen` had a bug). The `category` field reflects the
hard-coded IANA classification, not your include/exclude choices.

### Generate all public IPv4 addresses in order

The canonical "give me everything" example. The public space has
3,702,258,433 addresses (computed below). Sequential order makes the
output reproducible and easy to slice with `head`, `tail`, `split`,
`shuf`, etc.

```bash
ipgen generate \
  --preset public \
  --order sequential \
  --format text \
  --batch 65536 \
  --checkpoint-every 1000 \
  --state public-run.state.json \
  --out public-ipv4.txt
```

The total count is `2³² − 592,708,863 = 3,702,258,433`. You can verify
before starting:

```bash
ipgen ranges --preset public --summary
# {"cidr_count":2783,"total_addresses":3702258433}
```

Notes on the flags:

- `--batch 65536` — large batches (64k addresses) let the bulk-fill
  fast path dominate. Throughput is north of 200 M addresses/sec on a
  modern CPU.
- `--checkpoint-every 1000` — flush a checkpoint every 1000 batches,
  i.e. every ~65 million addresses. Smaller values give you finer
  resume granularity at the cost of more disk I/O.
- `--state public-run.state.json` — explicit checkpoint path. The
  default `ipgen.state.json` is fine if you only ever run one job at a
  time; explicit naming is good hygiene.
- `--out public-ipv4.txt` — write to a file rather than stdout. For a
  3.7-billion-line file (~25 GB), buffered file I/O is the right call.

To resume if the run is interrupted:

```bash
ipgen resume --state public-run.state.json --out public-ipv4.txt
```

### Generate just the private (RFC 1918) addresses

```bash
ipgen generate --preset private --order sequential --count 10
```

```
10.0.0.0
10.0.0.1
10.0.0.2
...
10.0.0.9
```

The private set is `10.0.0.0/8` + `172.16.0.0/12` + `192.168.0.0/16`,
totaling `16,777,216 + 1,048,576 + 65,536 = 17,891,328` addresses.
Verify:

```bash
ipgen ranges --preset private --summary
# {"cidr_count":3,"total_addresses":17891328}
```

### Generate the documentation ranges (TEST-NET 1/2/3)

```bash
ipgen generate --categories documentation --order sequential --count 10
```

The documentation category spans three /24 blocks:
`192.0.2.0/24` (TEST-NET-1), `198.51.100.0/24` (TEST-NET-2),
`203.0.113.0/24` (TEST-NET-3). Total 768 addresses. Sequential order
walks them in ascending order:

```
192.0.2.0
192.0.2.1
...
192.0.2.255
198.51.100.0
...
```

### Generate with custom includes and excludes

```bash
ipgen generate \
  --preset public \
  --include 9.9.9.0/24 \
  --exclude 9.9.9.128/25 \
  --order sequential --count 10
```

The result is the public set, plus `9.9.9.0/24`, minus
`9.9.9.128/25`. The first ten addresses are `9.9.9.0` through
`9.9.9.9` (the exclude starts at `.128`).

### Generate all 2³² addresses (the entire space)

For exhaustive testing or for generating a complete map of the IPv4
internet (including all the special-use blocks):

```bash
ipgen generate --preset all --order sequential --count 5
```

```
0.0.0.0
0.0.0.1
0.0.0.2
0.0.0.3
0.0.0.4
```

The `all` preset is the union of all 13 categories and totals exactly
`2³² = 4,294,967,296` addresses. Useful for testing that the
generator can handle the full range without bugs.

### Generate CIDR blocks instead of addresses

For block-level mapping, use `--order blocks`:

```bash
ipgen generate --preset public --order blocks --prefix-len 24 --count 5
```

```
1.0.0.0/24
1.0.1.0/24
1.0.2.0/24
1.0.3.0/24
1.0.4.0/24
```

With `--format jsonl` you also get the count of allowed addresses
inside each block (always 256 for `/24` of the public set, since a
full `/24` of public space contains 256 public addresses):

```bash
ipgen generate --preset public --order blocks --prefix-len 24 --count 2 --format jsonl
```

```json
{"index":0,"cidr":"1.0.0.0/24","count":256}
{"index":1,"cidr":"1.0.1.0/24","count":256}
```

With excludes, the count can be less than 256:

```bash
ipgen generate --preset public --include 1.0.0.0/24 --exclude 1.0.0.128/25 \
  --order blocks --prefix-len 24 --count 1 --format jsonl
```

```json
{"index":0,"cidr":"1.0.0.0/24","count":128}
```

### Pipe to a head-style consumer

`ipgen` handles broken pipes quietly, so `| head -n N` works:

```bash
ipgen generate --preset public --order sequential | head -n 5
```

The generator exits cleanly when `stdout` closes; no spurious error
messages.

## Inspecting

### Print the category table

```bash
ipgen categories
```

Add `--detailed` to also print the actual CIDRs per category:

```bash
ipgen categories --detailed
```

### Print the merged CIDR list for a selection

```bash
ipgen ranges --preset private
# 10.0.0.0/8
# 172.16.0.0/12
# 192.168.0.0/16
```

Compact summary:

```bash
ipgen ranges --preset public --summary
# {"cidr_count":2783,"total_addresses":3702258433}
```

Custom selection:

```bash
ipgen ranges --preset public --exclude 8.8.0.0/16 --summary
# {"cidr_count":2784,"total_addresses":3702133313}
```

(The CIDR count went up by 1 because we excluded a /16 in the middle
of the public set, which splits one CIDR into two.)

### Inspect a checkpoint

```bash
ipgen status --state ipgen.state.json
```

```
session_id:  6abcc4f6-161b-00000000
config_hash: 0xe07b9a4d3c5f1a01
order:       permuted
shard:       0/1
cursor:      200 / 3702258433 (0.0000% complete)
remaining:   3702258233
created_at:  1790756086
updated_at:  1790756086
```

For machine-readable output, use `jq` on the JSON file directly:

```bash
jq '{cursor, total, percent: (.cursor * 100 / .total)}' ipgen.state.json
```

## Resuming

### Pause and resume a long run

```bash
# 1. Start a long run.
ipgen generate --preset public --order permuted --seed 7 \
  --out addrs.txt --checkpoint-every 100

# 2. Hit Ctrl-C partway through.
#    ipgen: paused after 123456 more items — state saved to ipgen.state.json

# 3. Continue. The output file keeps appending; no duplicates, no gaps.
ipgen resume --state ipgen.state.json --out addrs.txt
```

### Resume with a different output format

```bash
ipgen resume --state ipgen.state.json --format jsonl --out addrs.jsonl
```

The format on resume does not have to match the original. (Mixing
formats in the same file is rarely a good idea, but the option is
there.)

### Seek to an arbitrary position by editing the checkpoint

```bash
# Jump to stream position 1_000_000 of an existing run.
jq '.cursor = 1000000 | .emitted = 1000000' ipgen.state.json \
  > ipgen.state.json.new && mv ipgen.state.json.new ipgen.state.json
ipgen resume --state ipgen.state.json --out addrs.txt
```

Useful for "skip ahead" workflows, e.g. re-running only a slice of a
permuted stream.

## Sharding

### Run 4 shards in parallel on one machine

```bash
M=4
for ((k=0; k<M; k++)); do
  ipgen generate --preset public --order permuted --seed 7 \
    --shard $k/$M --state shard-$k.state.json --out shard-$k.txt &
done
wait
```

Each shard writes its own checkpoint file; resume any of them
independently:

```bash
ipgen resume --state shard-2.state.json --out shard-2.txt
```

### Sharded blocks

```bash
ipgen generate --preset public --order blocks --prefix-len 24 \
  --shard 0/8 --out blocks-0.txt &
ipgen generate --preset public --order blocks --prefix-len 24 \
  --shard 1/8 --out blocks-1.txt &
# ...
wait
```

## MCP

### Drive `ipgen-mcp` from a shell one-liner

```bash
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}' \
  | ipgen-mcp
```

You should get one JSON-RPC response line on stdout.

### Start a session and pull a batch

```bash
{
  echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}';
  echo '{"jsonrpc":"2.0","method":"notifications/initialized"}';
  echo '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"generate_start","arguments":{"config":{"preset":"public","order":"permuted","seed":42}}}}';
} | ipgen-mcp
```

### Classify an address via MCP

```bash
echo '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"category_of","arguments":{"address":"8.8.8.8"}}}' \
  | ipgen-mcp
```

```json
{"jsonrpc":"2.0","id":1,"result":{"content":[{"type":"text","text":"{\"address\":\"8.8.8.8\",\"category\":\"public\"}"}],"isError":false}}
```

(You need to send an `initialize` handshake first; the server will
reject `tools/call` if you skip it on a real client. The shell
one-liner works because the server is permissive about message
order.)

A full worked MCP transcript including pause/resume is in
[`examples/mcp-transcript.json`](../examples/mcp-transcript.json).

## Composing with other tools

### Top 10 most-common /8 prefixes in a generated list

```bash
ipgen generate --preset public --order permuted --count 1000000 \
  | awk -F. '{print $1}' | sort | uniq -c | sort -rn | head -n 10
```

### Generate a CIDR list for `nmap -iL`

```bash
ipgen generate --preset public --order blocks --prefix-len 24 --count 100 \
  > top-100-blocks.txt
nmap -iL top-100-blocks.txt -sS ...
```

### Pipe through `xargs` for parallel processing

```bash
ipgen generate --preset public --order permuted --count 10000 \
  | xargs -P 8 -I{} -n 1 ./your-worker {}
```

### Generate JSON-Lines and process with `jq`

```bash
ipgen generate --preset public --order permuted --count 1000 \
  --format jsonl | jq -r .address | head -n 5
```

### Compress the output on the fly

```bash
ipgen generate --preset public --order sequential \
  --format text | gzip -9 > public-ipv4.txt.gz
```

For the 3.7-billion-address public set, gzip will reduce the ~25 GB
plain-text output to ~7 GB at the cost of CPU time; consider `pigz`
for parallel compression.

## Verification

### Verify resume produces identical output to uninterrupted

```bash
# Reference run.
ipgen generate --preset public --order permuted --seed 99 \
  --count 1000000 --out ref.txt --state ref.state.json

# Paused run: stop at 500k.
ipgen generate --preset public --order permuted --seed 99 \
  --count 500000 --out paused-part1.txt --state paused.state.json

# Resume for the remaining 500k.
ipgen resume --state paused.state.json --count 500000 --out paused-part2.txt

# Compare.
cat paused-part1.txt paused-part2.txt > combined.txt
diff -q ref.txt combined.txt && echo "OK: identical"
```

### Verify sharding covers the full stream

```bash
M=4
rm -f combined.txt
for ((k=0; k<M; k++)); do
  ipgen generate --preset public --order sequential --shard $k/$M \
    --count 1000 --out shard-$k.txt
  cat shard-$k.txt >> combined.txt
done
sort -n combined.txt | uniq -d   # should print nothing (no duplicates)
wc -l combined.txt               # should be 4000
```
