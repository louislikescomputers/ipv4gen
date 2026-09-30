//! Ordering modes: sequential, keyed pseudo-random bijection (cycle-walking
//! Feistel), and permuted /prefix blocks. Plus sharding.

use serde::{Deserialize, Serialize};

/// Default block prefix length for `Order::Blocks`.
pub const DEFAULT_BLOCK_PREFIX: u8 = 24;

/// How the generator maps its cursor index to an output position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "order", content = "params", rename_all = "kebab-case")]
pub enum Order {
    /// Ascending order over the allowed set. `order(i) = i`.
    Sequential,
    /// Keyed pseudo-random bijection over `[0, total)` implemented as a
    /// cycle-walking balanced Feistel network. **Not cryptographic** — it is a
    /// cheap deterministic spread suitable for survey workloads.
    Permuted { seed: u64 },
    /// Iterate aligned `/prefix_len` blocks in permuted order (see
    /// [`crate::Generator::next_cidr_blocks`]). Falls back to
    /// [`DEFAULT_BLOCK_PREFIX`] when `prefix_len` is `None`.
    Blocks {
        seed: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prefix_len: Option<u8>,
    },
}

impl Default for Order {
    fn default() -> Self {
        Order::Permuted { seed: 0 }
    }
}

impl Order {
    pub fn seed(&self) -> u64 {
        match self {
            Order::Sequential => 0,
            Order::Permuted { seed } | Order::Blocks { seed, .. } => *seed,
        }
    }
    pub fn name(&self) -> &'static str {
        match self {
            Order::Sequential => "sequential",
            Order::Permuted { .. } => "permuted",
            Order::Blocks { .. } => "blocks",
        }
    }
}

/// Shard specification: worker `index` of `count`, i.e. emit indices `i` with
/// `i % count == index`. Shards are disjoint and cover everything exactly once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Shard {
    pub index: u64,
    pub count: u64,
}

impl Shard {
    pub fn one() -> Shard {
        Shard { index: 0, count: 1 }
    }

    /// Parse `K/M`.
    pub fn parse(s: &str) -> Result<Shard, crate::Error> {
        let (k, m) = s
            .trim()
            .split_once('/')
            .ok_or_else(|| crate::Error::BadShard(s.to_string()))?;
        let index: u64 = k.trim().parse().map_err(|_| crate::Error::BadShard(s.to_string()))?;
        let count: u64 = m.trim().parse().map_err(|_| crate::Error::BadShard(s.to_string()))?;
        if count == 0 || index >= count {
            return Err(crate::Error::BadShard(s.to_string()));
        }
        Ok(Shard { index, count })
    }

    /// First logical index belonging to this shard.
    #[inline]
    pub fn first_index(&self, total: u64) -> Option<u64> {
        if self.index < total {
            Some(self.index)
        } else {
            None
        }
    }

    /// Number of logical indices belonging to this shard within `[0, total)`.
    #[inline]
    pub fn len(&self, total: u64) -> u64 {
        // count of i in [0,total) with i % count == index
        if total <= self.index {
            0
        } else {
            (total - self.index + self.count - 1) / self.count
        }
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
}

/// Cheap keyed integer mixer (murmur3 finalizer style). Not cryptographic.
#[inline]
fn mix(x: u32, key: u64) -> u32 {
    let mut h = x ^ ((key as u32).wrapping_mul(0x9E37_79B9));
    h ^= (key >> 32) as u32;
    h = h.wrapping_mul(0x85eb_ca6b);
    h ^= h >> 13;
    h = h.wrapping_mul(0xc2b2_ae35);
    h ^= h >> 16;
    h
}

const ROUNDS: usize = 6;

/// A cycle-walking balanced Feistel permutation over `[0, n)`.
///
/// Construction: pick the smallest even bit width `2w` with `2^(2w) >= n`
/// (minimum `2w = 2`). Permute `2w`-bit values with [`ROUNDS`] Feistel rounds;
/// if the result is `>= n`, re-apply the permutation until the value falls in
/// `[0, n)` (cycle walking). The result is an exact bijection on `[0, n)` and
/// is stateless: `apply(i)` depends only on `(seed, n, i)`. O(1) memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Feistel {
    n: u64,
    w: u32,       // half width in bits (1..=16)
    half_mask: u32,
    round_keys: [u64; ROUNDS],
}

impl Feistel {
    /// Build a permutation over `[0, n)`. `n` may be any value `>= 1`.
    pub fn new(n: u64, seed: u64) -> Feistel {
        assert!(n >= 1);
        // smallest even bit-width 2w with 2^(2w) >= n, minimum 2 (w = 1)
        let bits_needed = if n == 1 { 1 } else { 64 - (n - 1).leading_zeros() };
        let mut two_w = bits_needed.max(2);
        if two_w % 2 != 0 {
            two_w += 1;
        }
        let w = two_w / 2; // 1..=16 because n <= 2^32
        let half_mask = (1u32 << w) - 1;
        let mut round_keys = [0u64; ROUNDS];
        let mut k = seed ^ 0x5DEC_EE66_B;
        for rk in round_keys.iter_mut() {
            // splitmix64 step
            k = k.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = k;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            *rk = z ^ (z >> 31);
        }
        Feistel {
            n,
            w,
            half_mask,
            round_keys,
        }
    }

    #[inline]
    fn permute_full(&self, x: u32) -> u32 {
        let mut l = x & self.half_mask;
        let mut r = (x >> self.w) & self.half_mask;
        for &key in self.round_keys.iter() {
            let nl = r;
            let nr = l ^ mix(r, key);
            l = nl;
            r = nr & self.half_mask;
        }
        (r << self.w) | l
    }

    /// Map `i` in `[0, n)` to another index in `[0, n)`; bijective.
    pub fn apply(&self, i: u64) -> u64 {
        debug_assert!(i < self.n);
        let mut x = i as u32; // fits: n <= 2^32
        loop {
            x = self.permute_full(x);
            if (x as u64) < self.n {
                return x as u64;
            }
        }
    }

    pub fn domain(&self) -> u64 {
        self.n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feistel_bijection_small_sweep() {
        for n in 1..=512u64 {
            let f = Feistel::new(n, 12345);
            let mut seen = vec![false; n as usize];
            for i in 0..n {
                let j = f.apply(i);
                assert!(j < n, "n={n}: {i} -> {j} out of range");
                assert!(!seen[j as usize], "n={n}: duplicate image {j}");
                seen[j as usize] = true;
            }
        }
    }

    #[test]
    fn feistel_bijection_larger_values() {
        for &n in &[1_000_000u64, 1_048_576, 999_983, 524_288, 1_000_001] {
            let f = Feistel::new(n, 7);
            let mut seen = vec![false; n as usize];
            for i in 0..n {
                let j = f.apply(i);
                assert!(!seen[j as usize]);
                seen[j as usize] = true;
            }
        }
    }

    #[test]
    fn feistel_deterministic_and_seed_sensitive() {
        let a = Feistel::new(100_000, 1);
        let b = Feistel::new(100_000, 1);
        let c = Feistel::new(100_000, 2);
        for i in 0..1000 {
            assert_eq!(a.apply(i), b.apply(i));
        }
        let same: Vec<u64> = (0..1000).filter(|i| a.apply(*i) == c.apply(*i)).collect();
        assert!(same.len() < 5, "different seeds must give different orders");
    }

    #[test]
    fn shard_len_math() {
        for m in [1u64, 2, 3, 7, 10] {
            let mut sum = 0;
            for k in 0..m {
                sum += Shard { index: k, count: m }.len(100);
            }
            assert_eq!(sum, 100, "shards of M={m} must partition 100 indices");
        }
        assert_eq!(Shard { index: 5, count: 7 }.len(3), 0);
    }

    #[test]
    fn shard_parse() {
        assert_eq!(Shard::parse("2/7").unwrap(), Shard { index: 2, count: 7 });
        assert!(Shard::parse("7/2").is_err());
        assert!(Shard::parse("0/0").is_err());
        assert!(Shard::parse("abc").is_err());
    }
}
