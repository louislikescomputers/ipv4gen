//! The resumable generator. State = `(config, cursor)`.

use crate::checkpoint::Checkpoint;
use crate::config::Config;
use crate::error::Error;
use crate::interval::{Interval, IntervalSet};
use crate::order::{Feistel, Order};
use serde::{Deserialize, Serialize};
use std::net::Ipv4Addr;

/// An aligned CIDR block produced by `blocks` mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CidrBlock {
    pub base: Ipv4Addr,
    pub prefix_len: u8,
    /// How many allowed addresses actually fall inside this block (may be <
    /// the full block size after exclusions).
    pub count: u64,
}

impl CidrBlock {
    pub fn to_cidr_string(&self) -> String {
        format!("{}/{}", self.base, self.prefix_len)
    }
}

/// Algorithmic IPv4 generator with exact pause/resume semantics.
///
/// The complete state is `(config, cursor)` — a handful of integers — so a run
/// can be stopped at any moment and continued later, even in another process.
///
/// ```
/// use ipgen_core::{Config, Generator, Preset, Order};
/// let cfg = Config::builder().preset(Preset::Private).order(Order::Sequential).build().unwrap();
/// let mut g = Generator::new(cfg).unwrap();
/// assert_eq!(g.next(), Some(Ipv4Addr::new(10, 0, 0, 0)));
/// ```
#[derive(Debug, Clone)]
pub struct Generator {
    config: Config,
    set: IntervalSet,
    /// prefix sums of interval lengths; `prefix[k]` = number of addresses in
    /// intervals `[0..k]`. Length = intervals.len() + 1.
    prefix: Vec<u64>,
    /// Total allowed addresses (unsharded).
    total_allowed: u64,
    /// Permutation over shard-local indices `[0, shard_total)`.
    feistel: Option<Feistel>,
    cursor: u64,     // shard-local index of the next item to emit
    emitted: u64,    // items emitted so far (== cursor for fresh streams)
    seq_state: SeqState,
}

/// Fast-path state for sequential iteration without per-address binary search.
#[derive(Debug, Clone, Copy)]
enum SeqState {
    Unstaged,
    InInterval { iv_idx: usize, next: u32, end: u32 },
}

impl Generator {
    /// Create a new generator positioned at the start.
    pub fn new(config: Config) -> Result<Generator, Error> {
        let set = config.interval_set()?;
        let total_allowed = set.len();
        let prefix = build_prefix(&set);
        let shard_total = config.shard.len(total_allowed);
        let feistel = match config.order {
            Order::Sequential => None,
            _ => Some(Feistel::new(shard_total.max(1), config.order.seed())),
        };
        Ok(Generator {
            config,
            set,
            prefix,
            total_allowed,
            feistel,
            cursor: 0,
            emitted: 0,
            seq_state: SeqState::Unstaged,
        })
    }

    /// Rebuild a generator from a checkpoint (validates the config hash).
    pub fn resume(ckpt: Checkpoint) -> Result<Generator, Error> {
        ckpt.validate()?;
        let mut g = Generator::new(ckpt.config.clone())?;
        if ckpt.total != g.total() {
            return Err(Error::CheckpointInvalid(format!(
                "stored total {} does not match recomputed total {}",
                ckpt.total,
                g.total()
            )));
        }
        g.cursor = ckpt.cursor.min(g.total());
        g.emitted = ckpt.emitted.min(g.cursor);
        g.seq_state = SeqState::Unstaged;
        Ok(g)
    }

    /// The configuration in use.
    #[inline]
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Allowed address set (unsharded).
    #[inline]
    pub fn interval_set(&self) -> &IntervalSet {
        &self.set
    }

    /// Number of addresses this (possibly sharded) stream will emit in total.
    #[inline]
    pub fn total(&self) -> u64 {
        self.config.shard.len(self.total_allowed)
    }

    /// Addresses remaining in this stream.
    #[inline]
    pub fn remaining(&self) -> u64 {
        self.total().saturating_sub(self.cursor)
    }

    /// Fraction complete in `[0, 1]`.
    #[inline]
    pub fn progress(&self) -> f64 {
        let t = self.total();
        if t == 0 {
            0.0
        } else {
            self.cursor as f64 / t as f64
        }
    }

    /// Current shard-local cursor.
    #[inline]
    pub fn cursor(&self) -> u64 {
        self.cursor
    }

    /// Items emitted so far.
    #[inline]
    pub fn emitted(&self) -> u64 {
        self.emitted
    }

    /// Jump the cursor to an absolute shard-local index.
    pub fn seek(&mut self, index: u64) -> Result<(), Error> {
        if index > self.total() {
            return Err(Error::IndexOutOfRange {
                index,
                total: self.total(),
            });
        }
        self.cursor = index;
        self.emitted = index;
        self.seq_state = SeqState::Unstaged;
        Ok(())
    }

    /// Map a shard-local stream position to the global allowed-index.
    #[inline]
    fn logical_index(&self, pos: u64) -> u64 {
        let permuted = match &self.feistel {
            Some(f) => f.apply(pos),
            None => pos,
        };
        self.config.shard.index + permuted * self.config.shard.count
    }

    /// nth allowed address via prefix sums + binary search. O(log k).
    fn addr_at_allowed_index(&self, i: u64) -> Option<u32> {
        if i >= self.total_allowed {
            return None;
        }
        // find interval containing cumulative offset i
        let idx = self.prefix.binary_search(&i).unwrap_or_else(|x| x - 1);
        let before = self.prefix[idx];
        let iv = &self.set.intervals()[idx];
        Some(iv.start + (i - before) as u32)
    }

    /// Next address, advancing the cursor.
    pub fn next(&mut self) -> Option<Ipv4Addr> {
        let mut buf = [0u32; 1];
        if self.next_batch_into(&mut buf) == 1 {
            Some(Ipv4Addr::from(buf[0]))
        } else {
            None
        }
    }

    /// Fill `out` with up to `out.len()` addresses; returns how many were
    /// written. This is the hot batch path. For sequential, unsharded streams
    /// it walks interval by interval with no binary search per address.
    pub fn next_batch_into(&mut self, out: &mut [u32]) -> usize {
        let total = self.total();
        let mut written = 0usize;
        if self.config.shard.count == 1 && matches!(self.config.order, Order::Sequential) {
            // pure sequential fast path: bulk fill per interval
            while written < out.len() && self.cursor < total {
                let (iv_idx, next) = match self.seq_state {
                    SeqState::InInterval { iv_idx, next, .. } => (iv_idx, next),
                    SeqState::Unstaged => {
                        let i = self.cursor;
                        let idx = self.prefix.binary_search(&i).unwrap_or_else(|x| x - 1);
                        (idx, self.set.intervals()[idx].start + (i - self.prefix[idx]) as u32)
                    }
                };
                let iv = self.set.intervals()[iv_idx];
                let take = ((iv.end - next + 1) as u64)
                    .min(total - self.cursor)
                    .min((out.len() - written) as u64) as usize;
                for k in 0..take {
                    out[written + k] = next + k as u32;
                }
                written += take;
                self.cursor += take as u64;
                self.emitted += take as u64;
                if next as u64 + take as u64 > iv.end as u64 {
                    let ni = iv_idx + 1;
                    if ni < self.set.intervals().len() {
                        let niv = self.set.intervals()[ni];
                        self.seq_state = SeqState::InInterval {
                            iv_idx: ni,
                            next: niv.start,
                            end: niv.end,
                        };
                    } else {
                        self.seq_state = SeqState::Unstaged;
                    }
                } else {
                    self.seq_state = SeqState::InInterval {
                        iv_idx,
                        next: next + take as u32,
                        end: iv.end,
                    };
                }
            }
            return written;
        }
        // general path (permuted or sharded): one logical_index call each
        while written < out.len() && self.cursor < total {
            let i = self.logical_index(self.cursor);
            match self.addr_at_allowed_index(i) {
                Some(v) => {
                    out[written] = v;
                    written += 1;
                    self.cursor += 1;
                    self.emitted += 1;
                }
                None => break,
            }
        }
        written
    }

    /// Next batch of `n` addresses. Returns `None` only when the stream is
    /// already exhausted; otherwise returns however many remain (up to `n`).
    pub fn next_batch(&mut self, n: usize) -> Option<Vec<Ipv4Addr>> {
        if self.remaining() == 0 {
            return None;
        }
        let want = n.min(self.remaining() as usize).max(1);
        let mut buf = vec![0u32; want];
        let got = self.next_batch_into(&mut buf);
        buf.truncate(got);
        Some(buf.into_iter().map(Ipv4Addr::from).collect())
    }

    /// Produce the next `n` aligned `/prefix_len` blocks that contain at least
    /// one allowed address, in permuted order. Blocks are deduplicated: a
    /// block is emitted once, when the cursor first lands inside it. The
    /// cursor advances by one *stream position* per returned block, so pause /
    /// resume of block iteration needs only `(config, cursor)`.
    pub fn next_cidr_blocks(&mut self, n: usize, prefix_len: u8) -> Result<Vec<CidrBlock>, Error> {
        if !(8..=32).contains(&prefix_len) {
            return Err(Error::BadPrefixLen(prefix_len));
        }
        let mut out = Vec::with_capacity(n);
        let size: u64 = 1u64 << (32 - prefix_len);
        let total = self.total();
        while out.len() < n && self.cursor < total {
            let i = self.logical_index(self.cursor);
            let addr = self.addr_at_allowed_index(i).ok_or(Error::EmptySelection)?;
            let base = (addr as u64 / size) * size;
            // count allowed addresses within this block (for reporting)
            let mut cnt = 0u64;
            let mut j = i;
            while j < self.total_allowed {
                match self.addr_at_allowed_index(j) {
                    Some(a) if (a as u64) < base + size => {
                        cnt += 1;
                        j += 1;
                    }
                    _ => break,
                }
            }
            out.push(CidrBlock {
                base: Ipv4Addr::from(base as u32),
                prefix_len,
                count: cnt,
            });
            // advance to the first stream position whose address falls in a
            // different block
            self.cursor += 1;
            self.emitted = self.cursor;
            while self.cursor < total {
                let ni = self.logical_index(self.cursor);
                match self.addr_at_allowed_index(ni) {
                    Some(a) if (a as u64) >= base && (a as u64) < base + size => {
                        self.cursor += 1;
                        self.emitted = self.cursor;
                    }
                    _ => break,
                }
            }
        }
        Ok(out)
    }

    /// Snapshot the current state as a [`Checkpoint`].
    pub fn checkpoint(&self) -> Checkpoint {
        Checkpoint::from_generator(self)
    }
}

impl Iterator for Generator {
    type Item = Ipv4Addr;
    fn next(&mut self) -> Option<Ipv4Addr> {
        Generator::next(self)
    }
}

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

/// helper for tests: enumerate all addresses of a generator
pub fn collect_all(g: &mut Generator) -> Vec<Ipv4Addr> {
    let mut out = Vec::new();
    while let Some(ip) = g.next() {
        out.push(ip);
    }
    out
}

/// Exposed for benchmarks/tests: iterate intervals directly.
pub fn intervals(set: &IntervalSet) -> &[Interval] {
    set.intervals()
}
