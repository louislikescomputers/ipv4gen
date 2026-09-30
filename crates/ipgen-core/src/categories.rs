//! Hard-coded IPv4 range classification from the IANA special-purpose
//! registry (RFC 6890 and related RFCs).

use crate::interval::{Interval, IntervalSet};
use serde::{Deserialize, Serialize};
use std::net::Ipv4Addr;

/// Category of an IPv4 address per the IANA special-purpose registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Category {
    /// 0.0.0.0/8 — "this network on this host".
    ThisNetwork,
    /// RFC 1918 private-use space (10/8, 172.16/12, 192.168/16).
    Private,
    /// 100.64.0.0/10 — shared address space / CGNAT (RFC 6598).
    SharedCgnat,
    /// 127.0.0.0/8 — loopback (RFC 1122).
    Loopback,
    /// 169.254.0.0/16 — link-local (RFC 3927).
    LinkLocal,
    /// 192.0.0.0/24 — IETF protocol assignments (RFC 6890).
    IetfProtocol,
    /// TEST-NET documentation ranges (RFC 5737).
    Documentation,
    /// 192.88.99.0/24 — deprecated 6to4 anycast relay (RFC 7528).
    Deprecated6to4,
    /// 198.18.0.0/15 — benchmarking (RFC 2544).
    Benchmarking,
    /// 224.0.0.0/4 — multicast (RFC 5771).
    Multicast,
    /// 240.0.0.0/4 — reserved for future use.
    Reserved,
    /// 255.255.255.255/32 — limited broadcast.
    Broadcast,
    /// Everything else: globally routable unicast space.
    Public,
}

/// 255.255.255.255 is carved out of the reserved 240/4 block into its own
/// singleton Broadcast set, so the 14 categories partition the space and sum
/// to exactly 2^32 with no overlaps.
pub const BROADCAST_ADDR: Ipv4Addr = Ipv4Addr::new(255, 255, 255, 255);

impl Category {
    /// All categories, in canonical order.
    pub const ALL: &'static [Category] = &[
        Category::ThisNetwork,
        Category::Private,
        Category::SharedCgnat,
        Category::Loopback,
        Category::LinkLocal,
        Category::IetfProtocol,
        Category::Documentation,
        Category::Deprecated6to4,
        Category::Benchmarking,
        Category::Multicast,
        Category::Reserved,
        Category::Broadcast,
        Category::Public,
    ];

    /// Parse a category name (case-insensitive, `-`/`_` interchangeable).
    pub fn parse(s: &str) -> Result<Category, crate::Error> {
        let norm: String = s.trim().to_lowercase().replace('-', "_");
        Ok(match norm.as_str() {
            "thisnetwork" | "this_network" | "this-network" => Category::ThisNetwork,
            "private" => Category::Private,
            "sharedcgnat" | "shared_cgnat" | "cgnat" => Category::SharedCgnat,
            "loopback" => Category::Loopback,
            "linklocal" | "link_local" => Category::LinkLocal,
            "ietfprotocol" | "ietf_protocol" => Category::IetfProtocol,
            "documentation" | "docs" => Category::Documentation,
            "deprecated6to4" | "deprecated_6to4" | "6to4" => Category::Deprecated6to4,
            "benchmarking" => Category::Benchmarking,
            "multicast" => Category::Multicast,
            "reserved" => Category::Reserved,
            "broadcast" => Category::Broadcast,
            "public" => Category::Public,
            _ => return Err(crate::Error::BadCategory(s.to_string())),
        })
    }

    /// Stable lowercase name used in output formats.
    pub fn name(self) -> &'static str {
        match self {
            Category::ThisNetwork => "this-network",
            Category::Private => "private",
            Category::SharedCgnat => "shared-cgnat",
            Category::Loopback => "loopback",
            Category::LinkLocal => "link-local",
            Category::IetfProtocol => "ietf-protocol",
            Category::Documentation => "documentation",
            Category::Deprecated6to4 => "deprecated-6to4",
            Category::Benchmarking => "benchmarking",
            Category::Multicast => "multicast",
            Category::Reserved => "reserved",
            Category::Broadcast => "broadcast",
            Category::Public => "public",
        }
    }
}

/// `(CIDR base octets, prefix length, category)` — hard-coded table.
const TABLE: &[([u8; 4], u8, Category)] = &[
    ([0, 0, 0, 0], 8, Category::ThisNetwork),
    ([10, 0, 0, 0], 8, Category::Private),
    ([100, 64, 0, 0], 10, Category::SharedCgnat),
    ([127, 0, 0, 0], 8, Category::Loopback),
    ([169, 254, 0, 0], 16, Category::LinkLocal),
    ([172, 16, 0, 0], 12, Category::Private),
    ([192, 0, 0, 0], 24, Category::IetfProtocol),
    ([192, 0, 2, 0], 24, Category::Documentation),
    ([192, 88, 99, 0], 24, Category::Deprecated6to4),
    ([192, 168, 0, 0], 16, Category::Private),
    ([198, 18, 0, 0], 15, Category::Benchmarking),
    ([198, 51, 100, 0], 24, Category::Documentation),
    ([203, 0, 113, 0], 24, Category::Documentation),
    ([224, 0, 0, 0], 4, Category::Multicast),
    ([240, 0, 0, 0], 4, Category::Reserved),
];

fn cidr_interval(octets: [u8; 4], len: u8) -> Interval {
    let v: u32 = Ipv4Addr::from(octets).into();
    let mask: u32 = if len == 0 { 0 } else { !0u32 << (32 - len) };
    let start = v & mask;
    Interval {
        start: start,
        end: start | !mask,
    }
}

/// The special (non-public) blocks that make up the exact category partition.
/// 255.255.255.255 is carved out of the Reserved 240.0.0.0/4 block and lives in
/// the Broadcast set, so these intervals are pairwise disjoint and cover the
/// whole space together with Public.
fn partition_intervals() -> Vec<Interval> {
    let broadcast = cidr_interval([255, 255, 255, 255], 32);
    let mut ivs: Vec<Interval> = TABLE
        .iter()
        .filter(|(_, _, c)| *c != Category::Broadcast)
        .map(|&(o, l, _)| cidr_interval(o, l))
        // Shrink any block that ends at 255.255.255.255 (Reserved 240/4) so it
        // stops at 255.255.255.254; the last address belongs to Broadcast.
        .map(|iv| Interval {
            start: iv.start,
            end: if iv.end == u32::MAX {
                debug_assert!(iv.start <= broadcast.start);
                u32::MAX - 1
            } else {
                iv.end
            },
        })
        .collect();
    ivs.push(broadcast);
    ivs
}

/// Classify a single address.
pub fn classify(addr: Ipv4Addr) -> Category {
    if addr == BROADCAST_ADDR {
        return Category::Broadcast;
    }
    let v: u32 = addr.into();
    for &(octets, len, cat) in TABLE {
        let iv = cidr_interval(octets, len);
        if iv.contains(v) {
            return cat;
        }
    }
    Category::Public
}

/// The interval set covering exactly the given categories.
pub fn ranges_for(cats: &[Category]) -> IntervalSet {
    let specials = partition_intervals();
    let universe = IntervalSet::new(vec![Interval {
        start: 0,
        end: u32::MAX,
    }]);
    let special_set = IntervalSet::new(specials.clone());

    let mut ivs: Vec<Interval> = Vec::new();
    for want in cats {
        if *want == Category::Public {
            // Public = whole space minus every special block.
            ivs.extend(universe.subtract(&special_set).intervals());
        } else {
            for &iv in &specials {
                if classify(Ipv4Addr::from(iv.start)) == *want {
                    ivs.push(iv);
                }
            }
        }
    }
    IntervalSet::new(ivs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broadcast_is_its_own_partition_set() {
        assert_eq!(classify(BROADCAST_ADDR), Category::Broadcast);
        // Broadcast is a real (singleton) set in the partition:
        let bcast = ranges_for(&[Category::Broadcast]);
        assert_eq!(bcast.len(), 1);
        assert!(bcast.contains(u32::from(BROADCAST_ADDR)));
        let public = ranges_for(&[Category::Public]);
        assert!(!public.contains(u32::from(BROADCAST_ADDR)));
        let reserved = ranges_for(&[Category::Reserved]);
        // Reserved now stops at 255.255.255.254; .255 belongs to Broadcast.
        assert!(!reserved.contains(u32::from(BROADCAST_ADDR)));
        assert!(reserved.contains(u32::MAX - 1));
    }

    #[test]
    fn classify_known_addresses() {
        assert_eq!(classify(Ipv4Addr::new(0, 1, 2, 3)), Category::ThisNetwork);
        assert_eq!(classify(Ipv4Addr::new(10, 0, 0, 1)), Category::Private);
        assert_eq!(classify(Ipv4Addr::new(172, 16, 0, 1)), Category::Private);
        assert_eq!(classify(Ipv4Addr::new(172, 31, 255, 255)), Category::Private);
        assert_eq!(classify(Ipv4Addr::new(172, 32, 0, 0)), Category::Public);
        assert_eq!(classify(Ipv4Addr::new(192, 168, 1, 1)), Category::Private);
        assert_eq!(classify(Ipv4Addr::new(100, 64, 0, 1)), Category::SharedCgnat);
        assert_eq!(classify(Ipv4Addr::new(127, 0, 0, 1)), Category::Loopback);
        assert_eq!(classify(Ipv4Addr::new(169, 254, 1, 1)), Category::LinkLocal);
        assert_eq!(classify(Ipv4Addr::new(192, 0, 0, 1)), Category::IetfProtocol);
        assert_eq!(classify(Ipv4Addr::new(192, 0, 2, 1)), Category::Documentation);
        assert_eq!(classify(Ipv4Addr::new(198, 51, 100, 1)), Category::Documentation);
        assert_eq!(classify(Ipv4Addr::new(203, 0, 113, 1)), Category::Documentation);
        assert_eq!(classify(Ipv4Addr::new(192, 88, 99, 1)), Category::Deprecated6to4);
        assert_eq!(classify(Ipv4Addr::new(198, 18, 0, 1)), Category::Benchmarking);
        assert_eq!(classify(Ipv4Addr::new(198, 19, 255, 255)), Category::Benchmarking);
        assert_eq!(classify(Ipv4Addr::new(224, 0, 0, 1)), Category::Multicast);
        assert_eq!(classify(Ipv4Addr::new(239, 255, 255, 255)), Category::Multicast);
        assert_eq!(classify(Ipv4Addr::new(240, 0, 0, 1)), Category::Reserved);
        assert_eq!(classify(Ipv4Addr::new(255, 255, 255, 255)), Category::Broadcast);
        // note: this address counts toward the Reserved set size below
        assert_eq!(classify(Ipv4Addr::new(8, 8, 8, 8)), Category::Public);
        assert_eq!(classify(Ipv4Addr::new(1, 0, 0, 1)), Category::Public);
    }

    #[test]
    fn categories_partition_full_space() {
        // sizes sum to exactly 2^32 with no overlaps
        let sets: Vec<(Category, IntervalSet)> = Category::ALL
            .iter()
            .copied()
            .map(|c| (c, ranges_for(&[c])))
            .collect();
        let total: u64 = sets.iter().map(|(_, s)| s.len()).sum();
        assert_eq!(total, 4_294_967_296, "category sizes must sum to 2^32");

        // no overlaps: union of all == full space and sum of lens == 2^32
        // implies disjointness for merged interval sets. Additionally check
        // pairwise emptiness of intersections for adjacent pairs cheaply via
        // boundary sampling of every interval edge.
        let all = ranges_for(Category::ALL);
        assert_eq!(all.intervals().len(), 1);
        assert_eq!(all.intervals()[0], Interval { start: 0, end: u32::MAX });

        // spot-check boundaries on both sides for each table entry
        for &(octets, len, cat) in TABLE {
            let iv = cidr_interval(octets, len);
            // 255.255.255.255 is carved out of 240/4 into the Broadcast set.
            let expect = if iv.end == u32::MAX && cat == Category::Reserved {
                Category::Broadcast
            } else {
                cat
            };
            assert_eq!(classify(Ipv4Addr::from(iv.start)), cat);
            assert_eq!(classify(Ipv4Addr::from(iv.end)), expect);
        }
    }

    #[test]
    fn exhaustive_boundary_check() {
        // Every aligned block boundary in the table: verify classify agrees
        // with ranges_for membership for all 16 table intervals + public.
        for &(octets, len, cat) in TABLE {
            let iv = cidr_interval(octets, len);
            let set = ranges_for(&[cat]);
            assert!(set.contains(iv.start));
            if iv.end == u32::MAX && cat == Category::Reserved {
                // 255.255.255.255 was carved out of Reserved into Broadcast.
                assert!(!set.contains(iv.end));
                assert!(ranges_for(&[Category::Broadcast]).contains(iv.end));
            } else {
                assert!(set.contains(iv.end));
            }
            if iv.start > 0 {
                // start-1 belongs to some *other* category set
                let prev = classify(Ipv4Addr::from(iv.start - 1));
                assert_ne!(prev, cat);
            }
            if iv.end < u32::MAX {
                let next = classify(Ipv4Addr::from(iv.end + 1));
                assert_ne!(next, cat);
            }
        }
    }

    #[test]
    fn public_excludes_everything_special() {
        let public = ranges_for(&[Category::Public]);
        for &(octets, len, _) in TABLE {
            let iv = cidr_interval(octets, len);
            assert!(!public.contains(iv.start));
            assert!(!public.contains(iv.end));
        }
        assert!(public.contains(u32::from(Ipv4Addr::new(1, 1, 1, 1))));
    }
}
