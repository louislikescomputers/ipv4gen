# Rust Library API (`ipgen-core`)

`ipgen-core` is the library behind both the `ipgen` CLI and the
`ipgen-mcp` server. It exposes a small, ergonomic surface designed to
let a caller get going in five lines: a `Config` builder, a `Generator`
that emits addresses in batches, and a `Checkpoint` that serialises to
JSON. There is no async, no `tokio`, no `rand` — the only dependencies
are `serde` and `serde_json`.

This document is the API reference. It mirrors the doc comments in
`crates/ipgen-core/src/lib.rs` and its submodules; if anything here
disagrees with the source, the source is canonical.

## Top-level exports

```rust
pub use categories::{classify, ranges_for, Category};
pub use checkpoint::{Checkpoint, CHECKPOINT_VERSION};
pub use config::{Config, ConfigBuilder, Preset, DEFAULT_PREFIX_LEN};
pub use error::Error;
pub use generator::{CidrBlock, Generator};
pub use interval::{parse_cidr, parse_range_spec, Interval, IntervalSet};
pub use order::{Order, Shard};

pub use std::net::Ipv4Addr;  // re-exported for convenience
```

Every type below is at the crate root; submodules (`categories`,
`checkpoint`, etc.) are also public for callers who need fine-grained
access.

## `Config` and `ConfigBuilder`

`Config` is the complete, hashable description of a generation session.
It fully determines the address sequence together with the cursor, so
resume only needs `(config, cursor)`.

```rust
pub struct Config {
    pub categories: Vec<Category>,
    pub extra_include: Vec<String>,   // CIDR / range / address strings
    pub extra_exclude: Vec<String>,
    pub order: Order,
    pub shard: Shard,
}
```

Use `Config::builder()` to construct one. The builder validates the
include/exclude specs eagerly (so a typo fails at `build()`, not on the
first `next()` call) and refuses to build an empty selection.

```rust
use ipgen_core::{Config, Order, Preset};

let cfg = Config::builder()
    .preset(Preset::Public)              // or .categories(&[...])
    .order(Order::Permuted { seed: 42 })
    .include("10.0.0.0/8")              // optional
    .exclude("10.1.0.0/16")             // optional
    .shard(ipgen_core::Shard { index: 0, count: 4 })
    .build()?;
```

### Methods

- `Config::builder() -> ConfigBuilder` — start a builder.
- `cfg.interval_set() -> Result<IntervalSet, Error>` — resolve the
  allowed address set from categories + includes/excludes.
- `cfg.total_allowed() -> Result<u64, Error>` — count of addresses
  before sharding.
- `cfg.shard_total() -> Result<u64, Error>` — count of addresses this
  shard will emit.
- `cfg.feistel() -> Result<Feistel, Error>` — the permutation sized
  for this config's shard domain. Returns `Err` for `Sequential` order
  in some impls; check the source.
- `cfg.hash() -> u64` — FNV-1a 64 of the canonical JSON form of the
  config. Used for checkpoint tamper detection.
- `cfg.to_json_string() -> String` — canonical JSON.

### `Preset`

```rust
pub enum Preset {
    Public,         // only Public
    Private,        // only RFC 1918 Private
    All,            // every category, partitioning 2^32
    PublicPrivate,  // Public + Private
}
```

`Preset::parse(s: &str) -> Result<Preset, Error>` accepts
`"public"`, `"private"`, `"all"`, `"public+private"` (also
`"public_private"`, `"publicprivate"`), case-insensitive.

### `Category`

The 13-variant IANA category enum, plus `Category::ALL` (a static slice
of all variants in canonical order). `Category::parse(s)` accepts
hyphens or underscores interchangeably, case-insensitive. Names are
stable lowercase strings via `Category::name()`:

| Variant           | `name()`           | Examples                                  |
|-------------------|--------------------|-------------------------------------------|
| `ThisNetwork`     | `"this-network"`   | `0.0.0.0/8`                               |
| `Private`         | `"private"`        | `10/8`, `172.16/12`, `192.168/16`         |
| `SharedCgnat`     | `"shared-cgnat"`   | `100.64.0.0/10`                           |
| `Loopback`        | `"loopback"`       | `127.0.0.0/8`                             |
| `LinkLocal`       | `"link-local"`     | `169.254.0.0/16`                          |
| `IetfProtocol`    | `"ietf-protocol"`  | `192.0.0.0/24`                            |
| `Documentation`   | `"documentation"`  | `192.0.2.0/24`, `198.51.100.0/24`, `203.0.113.0/24` |
| `Deprecated6to4`  | `"deprecated-6to4"`| `192.88.99.0/24`                          |
| `Benchmarking`    | `"benchmarking"`   | `198.18.0.0/15`                           |
| `Multicast`       | `"multicast"`      | `224.0.0.0/4`                             |
| `Reserved`        | `"reserved"`       | `240.0.0.0/4` minus broadcast             |
| `Broadcast`       | `"broadcast"`      | `255.255.255.255/32`                      |
| `Public`          | `"public"`         | everything else                           |

Two free functions cover the common operations:

- `classify(addr: Ipv4Addr) -> Category` — linear scan over the table,
  with a fast path for the broadcast address.
- `ranges_for(cats: &[Category]) -> IntervalSet` — the merged address
  set for the requested categories. For `Public`, computed as the
  universe minus every special block.

## `Order` and `Shard`

```rust
pub enum Order {
    Sequential,
    Permuted { seed: u64 },
    Blocks { seed: u64, prefix_len: Option<u8> },
}

pub struct Shard {
    pub index: u64,
    pub count: u64,
}
```

- `Sequential` — ascending order over the allowed set. `order(i) = i`.
- `Permuted { seed }` — keyed pseudo-random bijection over `[0, N)`
  via cycle-walking Feistel. **Not cryptographic.** See
  [`ALGORITHM.md`](ALGORITHM.md) §4.
- `Blocks { seed, prefix_len }` — iterate aligned `/N` blocks
  (default `/24` when `prefix_len` is `None`) in permuted order.

`Shard::parse(s)` accepts the `"K/M"` form, e.g. `"2/7"`. Returns
`Err` if `M == 0` or `K >= M`. `Shard::one()` is the unsharded default.

## `IntervalSet`, `Interval`

```rust
pub struct Interval { pub start: u32, pub end: u32 }   // inclusive

pub struct IntervalSet { /* sorted, merged, non-overlapping intervals */ }
```

- `IntervalSet::new(ivs: Vec<Interval>) -> IntervalSet` — sorts and
  merges.
- `IntervalSet::empty() -> IntervalSet`
- `set.len() -> u64` — total addresses (up to 2³²).
- `set.is_empty() -> bool`
- `set.contains(v: u32) -> bool` — binary search, `O(log k)`.
- `set.union(&other) -> IntervalSet`
- `set.subtract(&other) -> IntervalSet`
- `set.nth(i: u64) -> Option<u32>` — the i-th address in ascending
  order. `O(log k)` via prefix sums.
- `set.to_cidrs() -> Vec<(Ipv4Addr, u8)>` — all maximal aligned CIDR
  blocks covering the set.
- `set.intervals() -> &[Interval]`
- `set.iter_intervals() -> Iter<Interval>`

Parsing helpers:

- `parse_cidr("a.b.c.d/len") -> Result<Interval, Error>`
- `parse_range_spec("a.b.c.d/len" | "a.b.c.d-e.f.g.h" | "a.b.c.d") -> Result<Interval, Error>`

The `RangeSpec` form is what `--include` and `--exclude` accept: a CIDR,
an explicit range, or a single address.

## `Generator`

The resumable generator. State is `(config, cursor)` plus a `SeqState`
fast-path cache for the sequential, unsharded case.

```rust
pub struct Generator { /* ... */ }

impl Generator {
    pub fn new(config: Config) -> Result<Generator, Error>;
    pub fn resume(ckpt: Checkpoint) -> Result<Generator, Error>;

    pub fn config(&self) -> &Config;
    pub fn interval_set(&self) -> &IntervalSet;
    pub fn total(&self) -> u64;
    pub fn remaining(&self) -> u64;
    pub fn progress(&self) -> f64;
    pub fn cursor(&self) -> u64;
    pub fn emitted(&self) -> u64;

    pub fn seek(&mut self, index: u64) -> Result<(), Error>;

    pub fn next(&mut self) -> Option<Ipv4Addr>;
    pub fn next_batch(&mut self, n: usize) -> Option<Vec<Ipv4Addr>>;
    pub fn next_batch_into(&mut self, out: &mut [u32]) -> usize;

    pub fn next_cidr_blocks(&mut self, n: usize, prefix_len: u8)
        -> Result<Vec<CidrBlock>, Error>;

    pub fn checkpoint(&self) -> Checkpoint;
}

impl Iterator for Generator { type Item = Ipv4Addr; /* ... */ }
```

### Method-by-method

- `new(config)` — build the prefix-sum array and the Feistel permutation
  (if needed). Cursor is `0`.
- `resume(ckpt)` — validate the checkpoint, rebuild the generator from
  the stored config, set `cursor` to the stored value. Refuses if the
  config hash does not match.
- `total()` — count of items this (possibly sharded) stream will emit.
- `remaining()` — `total().saturating_sub(cursor)`.
- `progress()` — `cursor / total` as `f64` in `[0, 1]`.
- `seek(index)` — jump the cursor to `index`. `Err` if `index > total`.
  Resets the `SeqState` cache.
- `next()` — one address, or `None` if exhausted. Allocates a 1-element
  buffer; prefer `next_batch_into` for performance.
- `next_batch(n)` — convenience: allocates a `Vec<Ipv4Addr>` of size up
  to `n`, fills it, returns `None` only when the stream is already
  exhausted.
- `next_batch_into(&mut [u32])` — the hot path. Fills the buffer with
  up to `buf.len()` addresses; returns the count written. For the
  sequential unsharded case, uses the bulk-fill fast path with no
  per-address binary search.
- `next_cidr_blocks(n, prefix_len)` — for `blocks` order. Returns up to
  `n` `CidrBlock`s, each containing the base address, prefix length,
  and the count of allowed addresses that fall inside the block. The
  cursor advances by one stream position per returned block (so
  pause/resume of blocks iteration needs only `(config, cursor)`).
- `checkpoint()` — snapshot the current state.

### `CidrBlock`

```rust
pub struct CidrBlock {
    pub base: Ipv4Addr,
    pub prefix_len: u8,
    pub count: u64,    // allowed addresses in this block (may be < block size)
}

impl CidrBlock {
    pub fn to_cidr_string(&self) -> String;  // "1.2.3.0/24"
}
```

## `Checkpoint`

```rust
pub struct Checkpoint {
    pub version: u32,            // CHECKPOINT_VERSION = 1
    pub session_id: String,
    pub config: Config,
    pub config_hash: u64,
    pub total: u64,
    pub cursor: u64,
    pub emitted: u64,
    pub created_at: u64,         // Unix seconds
    pub updated_at: u64,
}

impl Checkpoint {
    pub fn with_session_id(self, id: impl Into<String>) -> Self;
    pub fn validate(&self) -> Result<(), Error>;
    pub fn load(path: impl AsRef<Path>) -> Result<Checkpoint, Error>;
    pub fn from_json_bytes(data: &[u8]) -> Result<Checkpoint, Error>;
    pub fn save_atomic(&self, path: impl AsRef<Path>) -> Result<(), Error>;
}
```

`save_atomic` writes a temp file in the same directory, `fsync`s it, and
`rename`s over the target. A crash at any point leaves the previous
checkpoint intact. `validate` recomputes the config hash and refuses to
load a checkpoint whose hash does not match — that catches tampering
and corruption. `load` parses a checkpoint file; garbage input yields
`Err`, never a panic.

The free function `new_session_id(now: u64) -> String` produces a
deterministic-ish unique id from the wall clock, the PID, and an atomic
counter. It is not cryptographically unique; it is unique enough for
logging and MCP session tracking.

## `Error`

```rust
pub enum Error {
    BadCidr(String),
    BadRange(String),
    BadAddress(String),
    BadCategory(String),
    BadShard(String),
    EmptySelection,
    IndexOutOfRange { index: u64, total: u64 },
    BadPrefixLen(u8),
    CheckpointInvalid(String),
    Io(String),
}
```

Implements `std::error::Error` and `From<std::io::Error>`. The library
does not depend on `anyhow`; callers can `?`-propagate `ipgen_core::Error`
into their own error type or use `anyhow` themselves.

## Usage patterns

### Enumerate all public addresses in batches

```rust
use ipgen_core::{Config, Generator, Order, Preset};

let cfg = Config::builder()
    .preset(Preset::Public)
    .order(Order::Permuted { seed: 42 })
    .build()?;

let mut g = Generator::new(cfg)?;
let mut buf = vec![0u32; 8192];
loop {
    let n = g.next_batch_into(&mut buf);
    if n == 0 { break; }
    for &v in &buf[..n] {
        let ip = std::net::Ipv4Addr::from(v);
        // ... do something with ip
    }
}
```

### Pause and resume across processes

```rust
use ipgen_core::{Config, Generator, Order, Preset, Checkpoint};
use std::fs;

let cfg = Config::builder()
    .preset(Preset::Public)
    .order(Order::Permuted { seed: 7 })
    .build()?;

// First run: emit 1 million addresses, then checkpoint.
let mut g = Generator::new(cfg)?;
for _ in 0..1_000_000 {
    let _ = g.next();
}
let ckpt = g.checkpoint();
let bytes = serde_json::to_vec_pretty(&ckpt)?;
fs::write("state.json", &bytes)?;

// Later, in another process:
let bytes = fs::read("state.json")?;
let ckpt = Checkpoint::from_json_bytes(&bytes)?;
let mut g = Generator::resume(ckpt)?;
// g.cursor() == 1_000_000; g.next() returns the 1_000_001st address.
```

### Sharded enumeration

```rust
use ipgen_core::{Config, Generator, Order, Preset, Shard};

let total_shards = 8;
for k in 0..total_shards {
    let cfg = Config::builder()
        .preset(Preset::Public)
        .order(Order::Permuted { seed: 42 })
        .shard(Shard { index: k, count: total_shards as u64 })
        .build()?;
    let mut g = Generator::new(cfg)?;
    while let Some(ip) = g.next() {
        // ... shard k processes ip
    }
}
```

### Block-level enumeration

```rust
use ipgen_core::{Config, Generator, Order, Preset};

let cfg = Config::builder()
    .preset(Preset::Public)
    .order(Order::Blocks { seed: 42, prefix_len: Some(24) })
    .build()?;

let mut g = Generator::new(cfg)?;
let blocks = g.next_cidr_blocks(1000, 24)?;
for blk in blocks {
    println!("{} ({} allowed addrs)", blk.to_cidr_string(), blk.count);
}
```

## Crate features

`ipgen-core` has no default features and no optional features. The
dev-dependencies (`proptest`) are only used by the property tests and
do not affect downstream callers.

## MSRV

The library targets Rust 1.75 (pinned in `rust-toolchain.toml`). The
workspace uses edition 2021 and `resolver = "2"`. Any 1.75+ toolchain
will build the library without warnings.
