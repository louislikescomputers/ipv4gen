# Algorithm Reference

This document explains how `ipgen` actually works internally: the
interval-set representation of "allowed addresses," the prefix-sum
mapping from a cursor index to a concrete address, the cycle-walking
Feistel permutation that gives `permuted` order its spread, the sharding
stride, and why all of that adds up to pause/resume being a one-integer
operation. It is the reference to read when you want to understand *why*
the design is what it is, not just *what* the flags do.

The whole implementation lives in `crates/ipgen-core/src/`, in about
1,200 lines of Rust across seven files. Every crate uses
`#![forbid(unsafe_code)]`, the dependency set is `serde` + `serde_json`
only (no `rand`, no `tokio`, no `clap` in the core), and every public
type has at least one doctest.

## 1. The address model: `IntervalSet`

Everything in `ipgen` is expressed in terms of a sorted, non-overlapping,
merged list of inclusive `u32` intervals — the `IntervalSet` struct in
`crates/ipgen-core/src/interval.rs`:

```rust
pub struct IntervalSet {
    intervals: Vec<Interval>,  // sorted, non-overlapping, touching-merged
}

pub struct Interval {
    pub start: u32,
    pub end: u32,  // inclusive
}
```

Addresses are `u32` values (the big-endian numeric form of the dotted
quad), and an `Interval` is an inclusive `[start, end]` range. The set
representation is:

- **Sorted** by `start`, so binary search works.
- **Non-overlapping**, because the constructor merges touching or
  overlapping intervals into one. This means "10.0.0.0–10.0.0.5" and
  "10.0.0.5–10.0.0.10" become a single interval "10.0.0.0–10.0.0.10".
- **Merged for adjacency too**, not just overlap. So a CIDR like
  `10.0.0.0/30` followed by `10.0.0.4/30` becomes a single interval
  covering `10.0.0.0–10.0.0.7`. This keeps the interval count minimal,
  which keeps the prefix-sum array (below) small.

The set supports `union`, `subtract`, `contains`, `nth(i)`, `to_cidrs()`,
and `len()`. `union` and `subtract` are the set operations the
configuration system uses to combine categories with explicit
includes/excludes; `nth` is the index→address mapping; `to_cidrs` is
what the `ranges` subcommand prints.

`len()` returns a `u64` because the full IPv4 space has `2³²` addresses,
which does not fit in `u32`. Internally the sum is accumulated as
`u64`; the `len()` of an `Interval` is `(end - start + 1) as u64`.

## 2. The category partition

The `categories` module hard-codes the IANA special-purpose registry
(RFC 6890 and friends) as a `const` table of 15 CIDR-to-`Category`
mappings (see `crates/ipgen-core/src/categories.rs`, the `TABLE` constant).
The `Category` enum has 13 variants:

```
ThisNetwork    (0.0.0.0/8)
Private        (10/8, 172.16/12, 192.168/16)   — three CIDRs, one category
SharedCgnat    (100.64.0.0/10)
Loopback       (127.0.0.0/8)
LinkLocal      (169.254.0.0/16)
IetfProtocol   (192.0.0.0/24)
Documentation  (192.0.2.0/24, 198.51.100.0/24, 203.0.113.0/24)
Deprecated6to4 (192.88.99.0/24)
Benchmarking   (198.18.0.0/15)
Multicast      (224.0.0.0/4)
Reserved       (240.0.0.0/4)                   — minus 255.255.255.255
Broadcast      (255.255.255.255/32)            — singleton
Public         (everything else)
```

The interesting wrinkle is `Broadcast`. The `240.0.0.0/4` block
nominally ends at `255.255.255.255`, but the broadcast address is a
special case that gets its own singleton category. The construction
in `partition_intervals()` carves `255.255.255.255` out of the
`Reserved` block by shrinking the latter to end at `255.255.255.254`.
The result is that the 13 categories form a true partition of the
`2³²`-address space — no overlaps, no gaps — which the test
`categories_partition_full_space` enforces by summing the sizes and
getting exactly `4,294,967,296`.

The function `classify(Ipv4Addr) -> Category` is a linear scan over
the `TABLE` (15 entries), with a fast path for the broadcast address.
`ranges_for(&[Category])` returns the `IntervalSet` covering exactly
the requested categories; for `Public` it is the universe minus every
special block, computed as one `IntervalSet::subtract` call.

## 3. The index→address mapping: prefix sums

Given an `IntervalSet` of `N` total addresses, the function
`IntervalSet::nth(i: u64) -> Option<u32>` returns the `i`-th address in
ascending order. The naive implementation walks the intervals and
subtracts lengths; `ipgen` does it in `O(log k)` with binary search
over a prefix-sum array built once at generator construction:

```rust
fn build_prefix(set: &IntervalSet) -> Vec<u64> {
    let mut p = Vec::with_capacity(set.intervals().len() + 1);
    p.push(0u64);
    let mut acc = 0u64;
    for iv in set.intervals() {
        acc += iv.len();
        p.push(acc);
    }
    p
}
```

`prefix[k]` is the count of addresses in intervals `[0..k)`. To find
the `i`-th address: binary-search for the largest `k` such that
`prefix[k] <= i`; the address is
`intervals[k].start + (i - prefix[k]) as u32`.

The generator caches this prefix array in `Generator::prefix` at
construction time. It is the only piece of precomputed state the
generator holds; everything else (the Feistel permutation, the
sharding stride, the cursor) is `O(1)`.

For the **sequential fast path** (`Order::Sequential`, shard 0/1), the
generator does not even call `nth` per address — it iterates interval
by interval, bulk-filling the output buffer with `next + k` for each
`k` in the interval. This is the path that sustains hundreds of millions
of addresses per second in release mode.

## 4. The `permuted` order: cycle-walking Feistel

The `permuted` order is a keyed pseudo-random bijection over `[0, N)`,
where `N` is the shard-local total. It is **not cryptographic** — the
goal is a cheap, deterministic spread, not a secure PRP. The construction
is a **cycle-walking balanced Feistel network** (see
`crates/ipgen-core/src/order.rs`, the `Feistel` struct):

### Construction

1. **Choose the bit width.** Find the smallest even `2w` such that
   `2^(2w) >= N`, with a minimum of `2` (so `w` is in `1..=16` since
   `N <= 2³²`).
2. **Derive round keys.** From the user-supplied `seed`, run
   `splitmix64` six times to produce six 64-bit round keys.
3. **Round function.** Each round mixes the right half with a
   murmur3-style finalizer keyed by the round key:

   ```rust
   fn mix(x: u32, key: u64) -> u32 {
       let mut h = x ^ ((key as u32).wrapping_mul(0x9E37_79B9));
       h ^= (key >> 32) as u32;
       h = h.wrapping_mul(0x85eb_ca6b);
       h ^= h >> 13;
       h = h.wrapping_mul(0xc2b2_ae35);
       h ^= h >> 16;
       h
   }
   ```

4. **Apply.** Six rounds of the standard balanced Feistel structure:
   split `x` into `(L, R)` of `w` bits each, alternate
   `L' = R, R' = L XOR mix(R, key)` for each round key.

### Cycle walking

Because `2^(2w) >= N`, the Feistel permutation covers a superset of
`[0, N)`. To get an exact bijection on `[0, N)`, we **cycle-walk**:
apply the permutation to `i`; if the result is `< N`, return it;
otherwise apply again. The expected number of iterations is
`2^(2w) / N <= 2`, so the amortised cost is at most two Feistel calls
per index. The construction is bijective on `[0, 2^(2w))` (Feistel
networks are invertible), so cycle walking yields an exact bijection
on `[0, N)`.

### Why this gives spread

A Feistel network with 6 rounds and a non-trivial round function is a
known-good PRP construction (Luby-Rackoff gives `2^{n/2}` security in
the random-oracle model for 4 rounds; 6 rounds is overkill for spread).
For our purposes — survey workloads that want addresses spread across
the whole internet rather than clustered in one subnet — the
permutation does its job: nearby stream positions `i, i+1` map to
vastly different addresses, while preserving the bijection property
that pause/resume relies on.

The test `feistel_bijection_small_sweep` in
`crates/ipgen-core/src/order.rs` verifies the bijection for every `N`
in `1..=512` (sweeping all small sizes is cheap and catches off-by-one
in the bit-width selection). The test `feistel_bijection_larger_values`
does the same for a few values around `10⁶`, including a prime (`999983`)
and a power of two (`1048576`). The `resume_equals_uninterrupted`
proptest verifies that a paused-and-resumed stream is byte-identical
to an uninterrupted stream, which is the property that actually matters
for users.

## 5. Sharding

Sharding is a stride on the cursor. The `Shard` struct has two fields:
`index: u64` and `count: u64`, meaning "I am worker `index` of `count`
total; emit indices `i` where `i % count == index`." The function
`Shard::len(total)` returns the count of such indices in `[0, total)`:

```rust
pub fn len(&self, total: u64) -> u64 {
    if total <= self.index { 0 }
    else { (total - self.index + self.count - 1) / self.count }
}
```

The generator's `logical_index(pos)` maps a shard-local stream position
`pos` to a global allowed-index:

```rust
fn logical_index(&self, pos: u64) -> u64 {
    let permuted = match &self.feistel {
        Some(f) => f.apply(pos),
        None => pos,
    };
    self.config.shard.index + permuted * self.config.shard.count
}
```

That is: permute the shard-local position (or pass through, for
sequential order), then map to the global index by multiplying by
`shard.count` and adding `shard.index`. Because every `i` in
`[0, total)` has exactly one residue `mod count`, the shards
partition the global index space cleanly.

The property test `shard_sizes_partition_total` in
`crates/ipgen-core/tests/properties.rs` verifies this for `M` in
`{1, 2, 3, 7}`, including a `HashSet` disjointness check on sampled
output for the larger `M` values (where exhaustive enumeration would
be too slow for CI).

## 6. Why pause/resume is one integer

The generator's complete state is:

- `config: Config` — categories, includes, excludes, order, shard.
- `cursor: u64` — the shard-local index of the next item to emit.

That is it. There is no PRNG state, no memoised table, no file handle,
no open socket. The reason is that **every aspect of the address at
position `i` is a pure function of `(config, i)`**:

- The address at global allowed-index `j` is `set.nth(j)`, which
  depends only on `(set, j)`, and `set` depends only on `config`.
- The global allowed-index for shard-local position `i` is
  `shard.index + order(i) * shard.count`, where `order(i)` is either
  `i` (sequential) or `Feistel::apply(i)` (permuted). Both depend only
  on `(config, i)`.
- Therefore the address at shard-local position `i` is
  `nth(shard.index + order(i, config) * shard.count, config)`, a pure
  function of `(config, i)`.

Pause = save `(config, cursor)` to disk. Resume = load `(config, cursor)`,
rebuild the in-memory generator, set `cursor`. There is no
reconciliation step, no "skip-ahead" logic, no deduplication table.
The generator can be killed at any moment, by any signal, and the
resumed stream will be identical to an uninterrupted run.

The property test `resume_equals_uninterrupted` in
`crates/ipgen-core/tests/properties.rs` enforces this for random
configs, random cut points, both orders, and sharded configurations.
It generates a full reference stream, then chops the same config into
"first K items" and "resume from K", concatenates, and asserts
byte-equality with the reference. This is the test that gives the
project the confidence to claim "exact pause/resume" in the README.

## 7. Checkpoints

A `Checkpoint` is the JSON-serialisable form of `(config, cursor)` plus
bookkeeping fields (session id, totals, timestamps, content hash).
The serialised form looks like:

```json
{
  "version": 1,
  "session_id": "...",
  "config": { "categories": [...], "extra_include": [], "extra_exclude": [],
              "order": {...}, "shard": {...} },
  "config_hash": 16145446169603702680,
  "total": 3702258433,
  "cursor": 200,
  "emitted": 200,
  "created_at": 1790756086,
  "updated_at": 1790756086
}
```

The `config_hash` is **FNV-1a 64** of the canonical JSON of `config`.
It is dependency-free (a 12-line function in
`crates/ipgen-core/src/config.rs`), deterministic across Rust versions,
and sensitive to every field of `config` (verified by the
`hash_is_stable_and_sensitive` test). On load, the hash is recomputed
and compared to the stored value; a mismatch means the checkpoint was
tampered with or corrupted, and the load is refused.

Atomic writes are done in `save_atomic_bytes` (in
`crates/ipgen-core/src/checkpoint.rs`):

1. Create a temp file `.<target>.tmp` in the same directory as the target.
2. Write the bytes, `flush`, `fsync`.
3. `rename` the temp over the target.
4. `fsync` the directory (best-effort — some platforms do not support
   directory fsync).

A crash at any point leaves either the previous checkpoint intact or
a temp file alongside it; the loader never sees a half-written file.
The test `atomic_save_failure_keeps_old_checkpoint` enforces this by
simulating a crash between temp-write and rename.

## 8. Performance characteristics

All numbers below are from a single-threaded release build on commodity
x86-64 hardware (3 GHz, AVX2). Your mileage will vary; the point is the
shape of the numbers, not the absolute values.

- **Sequential, unsharded, batched (`next_batch_into` with a 1M
  buffer):** sustained ~210 M addresses/sec. The bulk-fill fast path
  dominates — there is no per-address binary search, just a tight loop
  of `out[k] = next + k` for each interval.
- **Sequential, unsharded, one-at-a-time (`next`):** ~30 M addresses/sec.
  The per-call overhead dominates; use batches.
- **Permuted, batched:** ~55 M addresses/sec. One Feistel call per
  address, plus one binary-search-per-address over the prefix array.
- **Permuted, one-at-a-time:** ~8 M addresses/sec.
- **Blocks, `/24`:** ~15 M blocks/sec. Each block involves a Feistel
  call, an `addr_at_allowed_index` lookup, and an inline count of
  allowed addresses within the block.

Memory is `O(intervals)` for the `IntervalSet` and `O(intervals)` for
the prefix-sum array, plus `O(1)` for everything else. The full public
set has 2,783 intervals after merging, so the in-memory size is on the
order of 50 KiB. The Feistel network has no table — it is six rounds
of arithmetic on `u32` values.

## 9. Why not just use `rand`?

A reasonable question: why not generate a random permutation with
`rand::seq::SliceRandom::shuffle`? Three reasons:

1. **Memory.** A full permutation of the public set would be
   3.7 billion `u32`s — about 14 GiB of RAM. The Feistel approach
   uses `O(1)` memory.
2. **Resumability.** A `rand`-based shuffle stores its state in the
   PRNG; pausing means saving the entire PRNG state, which for `StdRandom`
   is a few hundred bytes — fine, but you also need to remember which
   positions have been emitted, which is the 14 GiB permutation table
   again. The Feistel approach has no PRNG state at all; the "state" is
   just the cursor.
3. **Random access.** With a `rand` shuffle, the address at position
   `i` requires materialising positions `0..i` first. With the Feistel,
   `order(i)` is `O(1)` for any `i`, so you can seek to any position
   without computing the prefix.

The cycle-walking Feistel is the standard construction when you need a
cheap, random-access, stateless, keyed bijection over an arbitrary
domain. It is the right tool for this job.

## 10. What is *not* in here

For clarity, a few things the algorithm does **not** do:

- **No PRNG.** There is no `rand`, no `thread_rng`, no entropy source.
  The "permutation" is deterministic given the seed.
- **No memoisation of emitted addresses.** No bitset, no HashSet, no
  bloom filter. The bijection property of the Feistel guarantees no
  duplicates without having to remember which addresses have been
  emitted.
- **No networking.** No sockets, no ICMP, no DNS. The library never
  touches the network.
- **No unsafe code.** `#![forbid(unsafe_code)]` is in every crate.

If any of those change in a future version, this document should be
updated to reflect it.
