# Sharding

Sharding lets you split one logical enumeration across multiple workers
with no overlap and no missed addresses. Each worker is `--shard K/M`,
meaning "I am worker `K` of `M` total workers; emit every `M`-th item
starting at index `K`." The math is trivial, the implementation is a
handful of lines in `crates/ipgen-core/src/order.rs`, and the guarantee
is a property test in `crates/ipgen-core/tests/properties.rs`.

This guide explains when sharding is the right tool, what the guarantee
actually is, how to invoke it from the CLI, and how to wire it into a
parallel pipeline that pauses and resumes cleanly.

## When to shard

Shard when the bottleneck is work-per-address rather than
emit-per-second. A single-threaded `ipgen` process can sustain tens of
millions of addresses per second in `permuted` order (and hundreds of
millions in `sequential`), so for purely I/O-bound consumers a single
process is usually enough. Sharding starts to pay off when each emitted
address triggers an expensive downstream operation — a database lookup, an
HTTP request, a TLS handshake, a DNS resolution — and you want to fan
out across CPU cores or machines.

For example: if your consumer does a 50ms HTTPS handshake per address, a
single `ipgen` instance is bottlenecked at 20 addresses/second. Eight
shards in parallel give you 160 addresses/second, with zero coordination
between them — no central queue, no dedup table, no skip-list.

## The guarantee

For any `M >= 1` and any `K in [0, M)`, the stream produced by
`--shard K/M` is the subsequence of the unsharded stream consisting of
indices `i` where `i % M == K`. Three properties follow:

1. **Disjointness.** Shards `0..M` are pairwise disjoint, because an
   index cannot have two different residues mod `M`.
2. **Coverage.** The union of shards `0..M` is the full stream, because
   every index has exactly one residue mod `M`.
3. **Determinism.** The partition depends only on `M` and `K`, not on
   the order or seed. So you can shard a permuted run, a sequential run,
   or a blocks run, and each shard is a subsequence of the corresponding
   unsharded stream.

The property test `shard_sizes_partition_total` in
`crates/ipgen-core/tests/properties.rs` enforces all three for `M` in
`{1, 2, 3, 7}` against a real configuration with includes and excludes.
The test also exhaustively drains the smaller shards and samples 300k
addresses from each of the larger ones, asserting no overlaps in a
`HashSet`.

## CLI usage

Pass `--shard K/M` to `generate`:

```bash
# Run as the first of four workers.
ipgen generate --preset public --order permuted --shard 0/4 --out shard-0.txt

# Run as the second of four workers (in another terminal, or another machine).
ipgen generate --preset public --order permuted --shard 1/4 --out shard-1.txt
```

The same seed produces the same permuted stream across all shards — that
is, shard `0/4` and shard `1/4` are subsequences of the same unsharded
`--seed 0` run. Change the seed and you get a different (but still
disjoint-and-covering) partition.

A simple parallel fan-out on one machine:

```bash
M=8
for ((k=0; k<M; k++)); do
  ipgen generate --preset public --order permuted \
    --shard $k/$M --out shard-$k.txt &
done
wait
```

On Windows PowerShell:

```powershell
$M = 8
$jobs = 0..($M-1) | ForEach-Object {
  Start-Process -FilePath "ipgen.exe" -NoNewWindow -PassThru `
    -ArgumentList "generate","--preset","public","--order","permuted",
                  "--shard","$_/$M","--out","shard-$_.txt"
}
$jobs | ForEach-Object { $_.WaitForExit() }
```

## Sharding and pause/resume

Each shard has its own checkpoint, with its own cursor expressed in
shard-local positions. A shard can be paused and resumed independently of
the others:

```bash
# Worker 2 of 8 was killed. Resume just that shard.
ipgen resume --state shard-2.state.json --out shard-2.txt
```

The checkpoint stores `shard: {index, count}` so the resumed generator
knows which residue class it belongs to. There is no coordination with
the other shards — each one is a fully independent stream that happens
to be a subsequence of the same conceptual unsharded run.

## Sharding and ordering

Sharding composes with all three ordering modes:

- **`sequential` sharded.** The unsharded stream is just the addresses in
  ascending order; shard `K/M` emits every `M`-th address starting at
  position `K`. So shard `0/4` of the public set emits addresses at
  positions 0, 4, 8, ...; shard `1/4` emits positions 1, 5, 9, ...; and
  so on. Sorting the union of all shards gives the full ascending
  sequence.
- **`permuted` sharded.** The unsharded stream is the Feistel permutation
  of `[0, N)`. Each shard takes a strided subsequence. Because the
  permutation is a bijection, the union of shards is still the full set,
  but the within-shard order is permuted-spread.
- **`blocks` sharded.** Each shard emits every `M`-th `/24` (or other
  prefix length) block in permuted order. Useful for parallel block-level
  mapping where each worker grabs a different set of blocks.

## Choosing the shard count

A few rules of thumb:

- **One shard per CPU core for CPU-bound consumers.** If your
  post-processing is CPU-heavy (TLS handshakes, parsing, hashing), fan out
  to as many cores as you have.
- **Many more shards than cores for I/O-bound consumers.** If each
  address triggers a network round-trip, you want enough in-flight
  requests to saturate the network, not enough to saturate the CPU. 50
  to 200 shards per host is not unusual.
- **Powers of two if you care about cache lines.** The shard stride is
  `M`; cache-friendly access patterns happen when `M` is a power of two
  or, more pragmatically, when each shard's working set fits in L2.
  This is rarely a measurable effect; do not over-think it.
- **Fixed `M` across reboots.** If you pause a run and want to resume
  with a different shard count, you cannot — `M` is part of the config
  and changing it invalidates the cursor. Pick `M` once and stick with
  it for the whole run.

## Gotchas

- **Each shard needs its own `--state` file.** The default is
  `ipgen.state.json`, so two shards running in the same directory would
  stomp on each other. Use `--state shard-K.state.json`.
- **Each shard needs its own `--out` file.** Same reason.
- **The `cursor` in a checkpoint is shard-local.** A shard `2/8` with
  `cursor: 1000` has emitted 1000 items; the corresponding unsharded
  position is `1000 * 8 + 2 = 8002`. The status command displays the
  shard-local cursor and total.
- **Sharding is not load balancing.** Each shard gets a roughly equal
  number of addresses (`±1`), but if your consumer has
  data-dependent cost (some addresses take longer to process than
  others), shards will finish at different times. Use a work queue if
  you need true load balancing.
- **The `ranges` subcommand does not honour `--shard`.** It always
  prints the unsharded merged-CIDR set. To see the CIDR count for a
  specific shard, divide the unsharded total by `M` (with rounding up
  for the first `total % M` shards).
