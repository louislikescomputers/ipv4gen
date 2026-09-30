//! [`Config`] — the full, hashable description of what a session generates.

use crate::categories::{ranges_for, Category};
use crate::error::Error;
use crate::interval::{parse_range_spec, IntervalSet};
use crate::order::{Feistel, Order, Shard};
use serde::{Deserialize, Serialize};

/// Convenience presets over the category table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Preset {
    /// Only `Public` addresses.
    Public,
    /// Only `Private` (RFC 1918) addresses.
    Private,
    /// Every address in the space (`all` categories).
    All,
    /// Public + Private.
    PublicPrivate,
}

impl Preset {
    pub fn parse(s: &str) -> Result<Preset, Error> {
        Ok(match s.trim().to_lowercase().as_str() {
            "public" => Preset::Public,
            "private" => Preset::Private,
            "all" => Preset::All,
            "public+private" | "public_private" | "publicprivate" => Preset::PublicPrivate,
            _ => return Err(Error::BadCategory(s.to_string())),
        })
    }

    pub fn categories(&self) -> &'static [Category] {
        match self {
            Preset::Public => &[Category::Public],
            Preset::Private => &[Category::Private],
            Preset::All => Category::ALL,
            Preset::PublicPrivate => &[Category::Public, Category::Private],
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Preset::Public => "public",
            Preset::Private => "private",
            Preset::All => "all",
            Preset::PublicPrivate => "public+private",
        }
    }
}

/// The complete, serializable configuration of a generation session.
///
/// A `Config` fully determines the address sequence together with the cursor:
/// resume only needs `(config, cursor)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    /// Included categories (empty means: use `include_ranges` only).
    pub categories: Vec<Category>,
    /// Extra include specs (CIDR / range / single address strings).
    pub extra_include: Vec<String>,
    /// Extra exclude specs.
    pub extra_exclude: Vec<String>,
    /// Ordering mode.
    pub order: Order,
    /// Sharding (defaults to a single shard covering everything).
    pub shard: Shard,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            categories: vec![Category::Public],
            extra_include: Vec::new(),
            extra_exclude: Vec::new(),
            order: Order::Sequential,
            shard: Shard::one(),
        }
    }
}

impl Config {
    /// Start building a config.
    pub fn builder() -> ConfigBuilder {
        ConfigBuilder::new()
    }

    /// Resolve the allowed [`IntervalSet`] from categories + includes/excludes.
    pub fn interval_set(&self) -> Result<IntervalSet, Error> {
        let mut set = ranges_for(&self.categories);
        for spec in &self.extra_include {
            let iv = parse_range_spec(spec)?;
            set = set.union(&IntervalSet::new(vec![iv]));
        }
        for spec in &self.extra_exclude {
            let iv = parse_range_spec(spec)?;
            set = set.subtract(&IntervalSet::new(vec![iv]));
        }
        if set.is_empty() {
            return Err(Error::EmptySelection);
        }
        Ok(set)
    }

    /// Total number of *allowed* addresses before sharding.
    pub fn total_allowed(&self) -> Result<u64, Error> {
        Ok(self.interval_set()?.len())
    }

    /// Number of addresses this shard will emit.
    pub fn shard_total(&self) -> Result<u64, Error> {
        Ok(self.shard.len(self.total_allowed()?))
    }

    /// Feistel permutation sized to this config's shard domain.
    pub fn feistel(&self) -> Result<Feistel, Error> {
        let n = self.shard_total()?.max(1);
        Ok(Feistel::new(n, self.order.seed()))
    }

    /// Dependency-free content hash (FNV-1a 64) of the canonical JSON form of
    /// this config. Used to detect checkpoint tampering/corruption.
    pub fn hash(&self) -> u64 {
        let json = serde_json::to_string(self).expect("Config is always serializable");
        fnv1a_64(json.as_bytes())
    }

    /// Canonical string form used by CLI/MCP round-trips.
    pub fn to_json_string(&self) -> String {
        serde_json::to_string(self).expect("Config is always serializable")
    }
}

/// FNV-1a 64-bit hash (documented choice: simple, dependency-free).
pub fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Ergonomic builder for [`Config`].
#[derive(Debug, Clone, Default)]
pub struct ConfigBuilder {
    categories: Option<Vec<Category>>,
    preset: Option<Preset>,
    extra_include: Vec<String>,
    extra_exclude: Vec<String>,
    order: Option<Order>,
    shard: Option<Shard>,
}

impl ConfigBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set a preset (shorthand for a category list).
    pub fn preset(mut self, p: Preset) -> Self {
        self.preset = Some(p);
        self
    }

    /// Explicitly choose included categories (overrides any preset).
    pub fn categories(mut self, cats: &[Category]) -> Self {
        self.categories = Some(cats.to_vec());
        self.preset = None;
        self
    }

    /// Add an extra included CIDR / range / address.
    pub fn include<S: Into<String>>(mut self, spec: S) -> Self {
        self.extra_include.push(spec.into());
        self
    }

    /// Add an extra excluded CIDR / range / address.
    pub fn exclude<S: Into<String>>(mut self, spec: S) -> Self {
        self.extra_exclude.push(spec.into());
        self
    }

    pub fn order(mut self, o: Order) -> Self {
        self.order = Some(o);
        self
    }

    pub fn shard(mut self, s: Shard) -> Self {
        self.shard = Some(s);
        self
    }

    pub fn build(self) -> Result<Config, Error> {
        let categories = match (self.categories, self.preset) {
            (Some(c), _) => c,
            (None, Some(p)) => p.categories().to_vec(),
            (None, None) => Config::default().categories,
        };
        // validate all specs eagerly
        for spec in self.extra_include.iter().chain(self.extra_exclude.iter()) {
            parse_range_spec(spec)?;
        }
        let cfg = Config {
            categories,
            extra_include: self.extra_include,
            extra_exclude: self.extra_exclude,
            order: self.order.unwrap_or(Order::Permuted { seed: 0 }),
            shard: self.shard.unwrap_or_else(Shard::one),
        };
        // must be non-empty
        let _ = cfg.interval_set()?;
        Ok(cfg)
    }
}

/// Default prefix length used by block iteration when unspecified.
pub const DEFAULT_PREFIX_LEN: u8 = crate::order::DEFAULT_BLOCK_PREFIX;

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn preset_counts() {
        assert_eq!(
            Config::builder().preset(Preset::All).build().unwrap().total_allowed().unwrap(),
            4_294_967_296
        );
        // private = 10/8 (2^24) + 172.16/12 (2^20) + 192.168/16 (2^16)
        assert_eq!(
            Config::builder().preset(Preset::Private).build().unwrap().total_allowed().unwrap(),
            (1 << 24) + (1 << 20) + (1 << 16)
        );
        let public = Config::builder()
            .preset(Preset::Public)
            .build()
            .unwrap()
            .total_allowed()
            .unwrap();
        let specials = 4_294_967_296u64 - public;
        // sum of disjoint special blocks: 2^24 + 2^24 + 2^22 + 2^24 + 2^16 +
        // 2^24 + 3*2^8 + 2^15 + 2^28 + 2^28 = 592_708_863
        assert_eq!(specials, 592_708_863);
    }

    #[test]
    fn include_exclude() {
        let cfg = Config::builder()
            .preset(Preset::Private)
            .exclude("10.1.0.0/16")
            .build()
            .unwrap();
        assert_eq!(
            cfg.total_allowed().unwrap(),
            (1 << 24) + (1 << 20) + (1 << 16) - (1 << 16)
        );
        let cfg2 = Config::builder()
            .categories(&[])
            .include("192.0.2.0/24")
            .build()
            .unwrap();
        assert_eq!(cfg2.total_allowed().unwrap(), 256);
        assert!(cfg2.interval_set().unwrap().contains(u32::from(Ipv4Addr::new(192, 0, 2, 5))));
    }

    #[test]
    fn empty_selection_is_error() {
        let e = Config::builder().categories(&[]).build();
        assert!(matches!(e, Err(Error::EmptySelection)));
    }

    #[test]
    fn hash_is_stable_and_sensitive() {
        let a = Config::builder().preset(Preset::Public).order(Order::Sequential).build().unwrap();
        let b = Config::builder().preset(Preset::Public).order(Order::Sequential).build().unwrap();
        let c = Config::builder().preset(Preset::Private).order(Order::Sequential).build().unwrap();
        assert_eq!(a.hash(), b.hash());
        assert_ne!(a.hash(), c.hash());
        let d = Config {
            order: Order::Permuted { seed: 9 },
            ..a.clone()
        };
        assert_ne!(a.hash(), d.hash());
    }

    #[test]
    fn json_roundtrip_preserves_hash() {
        let cfg = Config::builder()
            .preset(Preset::PublicPrivate)
            .exclude("8.8.8.8")
            .order(Order::Permuted { seed: 42 })
            .shard(Shard { index: 1, count: 3 })
            .build()
            .unwrap();
        let s = cfg.to_json_string();
        let back: Config = serde_json::from_str(&s).unwrap();
        assert_eq!(cfg, back);
        assert_eq!(cfg.hash(), back.hash());
    }
}
