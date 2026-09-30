# Pause & Resume

Pause and resume is the central design decision behind `ipgen`. The whole
generator state is `(config, cursor)` — a small JSON document — and that
is enough to stop a run at any moment and pick it up later, in another
process, on another machine, after a crash, or after a reboot. There are no
partial outputs to reconcile, no "skip-ahead" heuristics, no
deduplication tables. The cursor is an integer that says "the next address
to emit is at stream position N," and every aspect of the generator is
deterministic given the config.

This guide explains what the checkpoint file is, how `ipgen` writes it,
how to inspect it, how to resume from it, and how to embed pause/resume
into a larger pipeline that needs to be killed and restarted regularly.

## What the checkpoint contains

A checkpoint is a JSON file with the following shape (see
`ipgen.state.json` in the repository for a real example):

```json
{
  "version": 1,
  "session_id": "6abcc4f6-161b-00000000",
  "config": {
    "categories": ["public"],
    "extra_include": [],
    "extra_exclude": [],
    "order": { "order": "permuted", "params": { "seed": 99 } },
    "shard": { "index": 0, "count": 1 }
  },
  "config_hash": 16145446169603702680,
  "total": 3702258433,
  "cursor": 200,
  "emitted": 200,
  "created_at": 1790756086,
  "updated_at": 1790756086
}
```

Fields:

- `version` — checkpoint format version. The current version is `1`. A
  future incompatible change to the on-disk layout will bump this, and
  `ipgen resume` will refuse to load older checkpoints rather than guess.
- `session_id` — a deterministic-ish unique id combining the wall clock,
  the process id, and an atomic counter. Mostly for logging and MCP
  session tracking; not security-relevant.
- `config` — the full generator configuration. This is what makes a run
  reproducible: same config + same cursor = same exact stream.
- `config_hash` — an FNV-1a 64-bit hash of the canonical JSON form of
  `config`. The hash is dependency-free and stable across Rust versions.
- `total` — number of addresses this (sharded) stream will emit, in
  total. Storing it lets `ipgen status` show a percentage without
  re-running the config.
- `cursor` — the shard-local index of the next item to emit. This is the
  one number you actually need to resume.
- `emitted` — count of items emitted so far. Always `<= cursor`. They
  differ only if you `seek` past the emitted position; for a normal
  forward run they stay equal.
- `created_at`, `updated_at` — Unix-epoch seconds, mostly for debugging.

## How checkpoints are written

`ipgen` writes checkpoints at three points:

1. **Periodically during a run**, if `--checkpoint-every N` is set. The
   value `N` is in batches; with the default batch size of 4096, a value
   of `1000` writes a checkpoint roughly every four million addresses.
2. **On `Ctrl-C`** (or `SIGINT`). The CLI installs a handler via the
   `ctrlc` crate that flips an atomic boolean; the generator notices on
   the next batch boundary, flushes the checkpoint, and exits cleanly.
3. **On clean exit** at the end of a run, regardless of
   `--checkpoint-every`. If the cursor reached the total, the final
   checkpoint will show `cursor == total`.

The write itself is **atomic and durable**:

1. The checkpoint JSON is serialised to a `Vec<u8>`.
2. A temp file is created in the same directory as the target, with a
   name like `.<target>.tmp`.
3. The bytes are written, `flush`ed, and `fsync`ed to the temp file.
4. The temp file is `rename`d over the target.
5. The directory containing the target is `fsync`ed as well, so the
   rename itself is durable.

A crash at any point leaves either the previous checkpoint intact or a
temp file alongside it. The loader never sees a half-written checkpoint.
This is the only sensible way to support long-running enumerations where
the user might kill the process or the power might go out.

## Inspecting a checkpoint

The `status` subcommand prints the human-readable form of a checkpoint
without doing any generation:

```bash
ipgen status --state ipgen.state.json
```

```
session_id:  6abcc4f6-161b-00000000
config_hash: 0x8b3e1f4c2a5b6e1f
order:       permuted
shard:       0/1
cursor:      200 / 3702258433 (0.0000% complete)
remaining:   3702258233
created_at:  1790756086
updated_at:  1790756086
```

The cursor and total are the numbers you actually care about. The
`config_hash` is what `ipgen resume` recomputes and compares against the
stored value; if they disagree, the load is refused. That catches
accidental edits to `config` (or, more realistically, accidental edits
to the JSON file in a text editor) before they silently corrupt a run.

For machine-readable progress, parse the JSON directly with `jq`:

```bash
jq -r '"\(.cursor) / \(.total) (\(.cursor * 100 / .total) | . * 1000 | floor / 1000)%"' \
  ipgen.state.json
```

## Resuming a run

The `resume` subcommand loads a checkpoint, rebuilds the in-memory
generator from the stored config, sets the cursor to the stored value,
and continues producing addresses. The output file keeps appending:

```bash
# 1. Start a long run.
ipgen generate --preset public --order permuted --seed 7 \
  --out addrs.txt --checkpoint-every 100

# 2. Hit Ctrl-C partway through.
#    ipgen: paused after 123456 more items — state saved to ipgen.state.json

# 3. Check where you stopped.
ipgen status --state ipgen.state.json

# 4. Continue. The output file keeps appending; no duplicates, no gaps.
ipgen resume --state ipgen.state.json --out addrs.txt
```

You can also change the output format on resume (useful if you started a
run in `text` format and want to switch to `jsonl` mid-stream — though
mixing formats in the same file is rarely a good idea, the option is
there if you need it):

```bash
ipgen resume --state ipgen.state.json --format jsonl --out addrs.jsonl
```

## The resume equivalence guarantee

Resuming produces a stream **identical** to an uninterrupted run. The
project's test suite enforces this property for both `sequential` and
`permuted` orders and for sharded configurations (see
`crates/ipgen-core/tests/properties.rs` in the source tree, test named
`resume_equals_uninterrupted`). The test generates a full reference
stream, then chops the same generation into a "first K items" run and a
"resume from K to end" run, concatenates them, and asserts byte-equality
with the reference.

Why this is true in general:

- The generator's state is exactly `(config, cursor)`.
- The address at cursor `i` is `nth(order(i, config), config)`, a pure
  function of `(i, config)`. There is no hidden state — no PRNG registers,
  no counters, no memoised tables.
- Resuming restores `(config, cursor)` exactly and starts emitting from
  `cursor` onward, which is the same as if the original run had simply
  continued past `cursor`.

This property is what makes `ipgen` safe to use in cron jobs, systemd
timers, and other environments where the process may be killed at any
time by the supervisor.

## Embedded use: pausing from your own code

If you are using `ipgen-core` as a library, you can pause from your own
code by calling `g.checkpoint()` at any moment. The returned `Checkpoint`
implements `Serialize`, so you can `serde_json::to_string` it and stash
it anywhere — a database, a Redis key, an S3 object, etc. Resuming is
`Generator::resume(Checkpoint::from_json_bytes(...)?)`.

```rust
use ipgen_core::{Config, Generator, Order, Preset, Checkpoint};
use std::fs;

let cfg = Config::builder()
    .preset(Preset::Public)
    .order(Order::Permuted { seed: 42 })
    .build()?;

let mut g = Generator::new(cfg)?;
for _ in 0..1_000_000 {
    let _ = g.next();
}
let ckpt = g.checkpoint();
let json = serde_json::to_vec_pretty(&ckpt)?;
fs::write("state.json", &json)?;

// Later, in another process:
let bytes = fs::read("state.json")?;
let ckpt = Checkpoint::from_json_bytes(&bytes)?;
let mut g = Generator::resume(ckpt)?;
```

`Checkpoint::save_atomic` does the temp-file-and-rename dance for you. If
you need to write to a non-filesystem destination (database, KV store),
use the `from_json_bytes` / `serde_json::to_vec` pair directly.

## Checkpoint tamper detection

The `config_hash` field is recomputed on load and compared to the stored
value. Modifying the `cursor` or `emitted` fields is allowed — that's just
"seek to a different position." Modifying the `config` field without
also recomputing the hash is refused, because that would silently change
the meaning of the cursor and produce a stream that does not match the
recorded run.

You can demonstrate the behaviour with a small script:

```bash
# Save a checkpoint, then bump the cursor; resume should still work.
jq '.cursor = 100' ipgen.state.json > ipgen.state.json.new && \
  mv ipgen.state.json.new ipgen.state.json
ipgen status --state ipgen.state.json    # prints cursor: 100

# Now add an extra_exclude without fixing the hash; resume should refuse.
jq '.config.extra_exclude += ["1.2.3.4"]' ipgen.state.json > bad.json
ipgen status --state bad.json
# error: checkpoint invalid: config hash mismatch: stored ..., recomputed ...
```

This is intentionally a refusal, not a warning. A silent acceptance of a
mismatched config could ruin a long enumeration by interleaving items
from two different streams.

## Seeking to an arbitrary position

The CLI does not expose `seek` directly today (it is a library-only
feature), but you can achieve the same effect by editing the checkpoint
JSON:

```bash
# Jump to stream position 1,000,000 of an existing run.
jq '.cursor = 1000000 | .emitted = 1000000' ipgen.state.json \
  > ipgen.state.json.new && mv ipgen.state.json.new ipgen.state.json
ipgen resume --state ipgen.state.json --out addrs.txt
```

This works because both `cursor` and `emitted` are within `total` (the
loader clamps to the total). Use it for "skip ahead" workflows — for
example, re-running only a slice of a permuted stream.

## What does NOT survive resume

A few things are intentionally not in the checkpoint:

- **Open file handles**. The output file is opened fresh on `resume`. If
  you used `--out` before, you almost certainly want to use the same
  `--out` on `resume`; otherwise the file will be created but the
  existing output will not be appended to.
- **Random seed source state**. There is none. The seed is part of the
  config, and the permutation is a pure function of `(seed, n, i)`.
- **In-memory batch buffer**. The generator may have been mid-batch when
  paused; the cursor is rounded up to a batch boundary, and resume
  continues from there.

If you need to checkpoint mid-batch (which would mean "emit the last
2000 of a 4096-address batch, then resume"), you can set `--batch 1` —
the generator will then write one address at a time and the checkpoint
cursor will be exactly the count of addresses emitted. The cost is
throughput, since the bulk-fill fast path does not kick in for `batch == 1`.
A reasonable compromise is `--batch 1024 --checkpoint-every 1`.
