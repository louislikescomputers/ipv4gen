//! Sorted, merged, non-overlapping inclusive `u32` intervals — the single
//! source of truth for "what is allowed" in a session.

use crate::error::Error;
use serde::{Deserialize, Serialize};
use std::net::Ipv4Addr;

/// An inclusive interval `[start, end]` of IPv4 addresses represented as
/// big-endian numeric `u32` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Interval {
    pub start: u32,
    pub end: u32,
}

impl Interval {
    /// Number of addresses contained in the interval (as `u64`; a full /0 has
    /// 2^32 addresses which does not fit in `u32`).
    #[inline]
    pub fn len(&self) -> u64 {
        self.end as u64 - self.start as u64 + 1
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        false // intervals are always non-empty by construction
    }

    #[inline]
    pub fn contains(&self, v: u32) -> bool {
        self.start <= v && v <= self.end
    }
}

/// Parse a CIDR string (`a.b.c.d/len`) into an [`Interval`].
pub fn parse_cidr(s: &str) -> Result<Interval, Error> {
    let s = s.trim();
    let (addr_s, len_s) = s
        .split_once('/')
        .ok_or_else(|| Error::BadCidr(s.to_string()))?;
    let addr: Ipv4Addr = addr_s.parse().map_err(|_| Error::BadCidr(s.to_string()))?;
    let len: u8 = len_s
        .parse()
        .map_err(|_| Error::BadCidr(s.to_string()))?;
    if len > 32 {
        return Err(Error::BadCidr(s.to_string()));
    }
    let v: u32 = addr.into();
    let mask: u32 = if len == 0 { 0 } else { !0u32 << (32 - len) };
    let start = v & mask;
    let end = start | !mask;
    Ok(Interval { start, end })
}

/// Parse an address (`a.b.c.d`), a CIDR (`a.b.c.d/len`) or an explicit range
/// (`a.b.c.d-e.f.g.h`) into an [`Interval`].
pub fn parse_range_spec(s: &str) -> Result<Interval, Error> {
    let s = s.trim();
    if s.contains('/') {
        return parse_cidr(s);
    }
    if let Some((a, b)) = s.split_once('-') {
        let start: Ipv4Addr = a.trim().parse().map_err(|_| Error::BadRange(s.to_string()))?;
        let end: Ipv4Addr = b.trim().parse().map_err(|_| Error::BadRange(s.to_string()))?;
        let (start, end): (u32, u32) = (start.into(), end.into());
        if start > end {
            return Err(Error::BadRange(s.to_string()));
        }
        return Ok(Interval { start, end });
    }
    let addr: Ipv4Addr = s.parse().map_err(|_| Error::BadAddress(s.to_string()))?;
    let v: u32 = addr.into();
    Ok(Interval { start: v, end: v })
}

/// A set of IPv4 addresses represented as a sorted, non-overlapping, merged
/// list of inclusive intervals.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct IntervalSet {
    intervals: Vec<Interval>,
}

impl IntervalSet {
    /// Build from raw intervals; sorts and merges touching/overlapping ones.
    pub fn new(mut ivs: Vec<Interval>) -> Self {
        ivs.retain(|i| i.start <= i.end);
        ivs.sort();
        let mut merged: Vec<Interval> = Vec::with_capacity(ivs.len());
        for iv in ivs {
            match merged.last_mut() {
                // merge when overlapping or adjacent (end + 1 >= next start)
                Some(last) if (iv.start as u64) <= (last.end as u64) + 1 => {
                    if iv.end > last.end {
                        last.end = iv.end;
                    }
                }
                _ => merged.push(iv),
            }
        }
        IntervalSet { intervals: merged }
    }

    pub fn empty() -> Self {
        IntervalSet {
            intervals: Vec::new(),
        }
    }

    #[inline]
    pub fn intervals(&self) -> &[Interval] {
        &self.intervals
    }

    /// Iterate the underlying intervals.
    #[inline]
    pub fn iter_intervals(&self) -> std::slice::Iter<'_, Interval> {
        self.intervals.iter()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.intervals.is_empty()
    }

    /// Total number of addresses in the set (up to 2^32).
    pub fn len(&self) -> u64 {
        self.intervals.iter().map(|i| i.len()).sum()
    }

    /// Does the set contain address `v`? Binary search over the merged list.
    pub fn contains(&self, v: u32) -> bool {
        self.intervals
            .binary_search_by(|i| {
                if i.end < v {
                    std::cmp::Ordering::Less
                } else if i.start > v {
                    std::cmp::Ordering::Greater
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .is_ok()
    }

    /// Union of two sets.
    pub fn union(&self, other: &IntervalSet) -> IntervalSet {
        let mut v = self.intervals.clone();
        v.extend(other.intervals.iter().copied());
        IntervalSet::new(v)
    }

    /// `self` minus `other` (set difference).
    pub fn subtract(&self, other: &IntervalSet) -> IntervalSet {
        let mut out: Vec<Interval> = Vec::new();
        for iv in &self.intervals {
            let mut cur = iv.start;
            for o in &other.intervals {
                if (o.end as u64) + 1 < cur as u64 || o.start > iv.end {
                    continue;
                }
                if o.start > cur {
                    out.push(Interval {
                        start: cur,
                        end: o.start - 1,
                    });
                }
                cur = cur.max(o.end.saturating_add(1));
                if cur > iv.end {
                    break;
                }
            }
            if cur <= iv.end {
                out.push(Interval { start: cur, end: iv.end });
            }
        }
        IntervalSet { intervals: out } // already sorted & disjoint
    }

    /// The `i`-th address of the set (`i` in `[0, len())`), via binary search
    /// over the intervals. O(log k).
    pub fn nth(&self, i: u64) -> Option<u32> {
        if i >= self.len() {
            return None;
        }
        // binary search: find first interval whose exclusive prefix end > i
        let mut lo = 0usize;
        let mut hi = self.intervals.len();
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let before: u64 = self.intervals[..mid].iter().map(|x| x.len()).sum();
            let after = before + self.intervals[mid].len();
            if i < before {
                hi = mid;
            } else if i >= after {
                lo = mid + 1;
            } else {
                return Some(self.intervals[mid].start + (i - before) as u32);
            }
        }
        None
    }

    /// All maximal aligned CIDR blocks covering this set (used for `--format
    /// cidr` and `ranges`).
    pub fn to_cidrs(&self) -> Vec<(Ipv4Addr, u8)> {
        let mut out = Vec::new();
        for iv in &self.intervals {
            let mut start = iv.start as u64;
            let end = iv.end as u64;
            while start <= end {
                // largest block starting at `start` that fits within `end`
                let mut plen = 32u8;
                while plen > 0 {
                    let size: u64 = 1 << (32 - (plen - 1));
                    let base_ok = start % size == 0;
                    let fits = start + size - 1 <= end;
                    if base_ok && fits {
                        plen -= 1;
                    } else {
                        break;
                    }
                }
                let size: u64 = 1u64 << (32 - plen);
                out.push((Ipv4Addr::from(start as u32), plen));
                start += size;
            }
        }
        out
    }
}

impl FromIterator<Interval> for IntervalSet {
    fn from_iter<T: IntoIterator<Item = Interval>>(it: T) -> Self {
        IntervalSet::new(it.into_iter().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn iv(s: u32, e: u32) -> Interval {
        Interval { start: s, end: e }
    }

    #[test]
    fn cidr_parsing() {
        let i = parse_cidr("10.0.0.0/8").unwrap();
        assert_eq!(i.start, u32::from(Ipv4Addr::new(10, 0, 0, 0)));
        assert_eq!(i.end, u32::from(Ipv4Addr::new(10, 255, 255, 255)));
        let i = parse_cidr("255.255.255.255/32").unwrap();
        assert_eq!((i.start, i.end), (u32::MAX, u32::MAX));
        let i = parse_cidr("0.0.0.0/0").unwrap();
        assert_eq!((i.start, i.end), (0, u32::MAX));
        assert!(parse_cidr("10.0.0.0/33").is_err());
        assert!(parse_cidr("not-a-cidr").is_err());
    }

    #[test]
    fn range_spec_parsing() {
        let i = parse_range_spec("1.2.3.4-1.2.3.10").unwrap();
        assert_eq!(i.len(), 7);
        let i = parse_range_spec("1.2.3.4").unwrap();
        assert_eq!(i.len(), 1);
        assert!(parse_range_spec("1.2.3.10-1.2.3.4").is_err());
    }

    #[test]
    fn merge_and_subtract() {
        let a = IntervalSet::new(vec![iv(0, 9), iv(5, 19), iv(30, 39)]);
        assert_eq!(a.intervals(), &[iv(0, 19), iv(30, 39)]);
        let b = IntervalSet::new(vec![iv(10, 31)]);
        let d = a.subtract(&b);
        assert_eq!(d.intervals(), &[iv(0, 9), iv(32, 39)]);
        assert_eq!(d.len(), 18);
        let u = a.union(&b);
        assert_eq!(u.intervals(), &[iv(0, 39)]);
    }

    #[test]
    fn brute_force_model() {
        // property-style check with fixed pseudo-random inputs (no rand dep)
        let mut state = 0x1234_5678_9abc_def0u64;
        let mut rnd = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for _ in 0..200 {
            let mut av = Vec::new();
            let mut bv = Vec::new();
            for _ in 0..6 {
                let s = (rnd() % 64) as u32;
                let e = s + (rnd() % 24) as u32;
                av.push(iv(s, e));
            }
            for _ in 0..4 {
                let s = (rnd() % 64) as u32;
                let e = s + (rnd() % 24) as u32;
                bv.push(iv(s, e));
            }
            let a = IntervalSet::new(av.clone());
            let b = IntervalSet::new(bv.clone());
            // brute force membership over 0..64+ margins
            let universe = 0..128u32;
            let bf_union: Vec<u32> = universe
                .clone()
                .filter(|v| av.iter().any(|i| i.contains(*v)) || bv.iter().any(|i| i.contains(*v)))
                .collect();
            let got_union: Vec<u32> = universe
                .clone()
                .filter(|v| a.union(&b).contains(*v))
                .collect();
            assert_eq!(bf_union, got_union);
            let bf_sub: Vec<u32> = universe
                .clone()
                .filter(|v| av.iter().any(|i| i.contains(*v)) && !bv.iter().any(|i| i.contains(*v)))
                .collect();
            let got_sub: Vec<u32> = universe.filter(|v| a.subtract(&b).contains(*v)).collect();
            assert_eq!(bf_sub, got_sub);
            // nth matches enumeration
            let set = a.subtract(&b);
            for (idx, v) in bf_sub.iter().enumerate() {
                assert_eq!(set.nth(idx as u64), Some(*v));
            }
            assert_eq!(set.nth(set.len()), None);
        }
    }

    #[test]
    fn cidrs_cover_set() {
        let set = IntervalSet::new(vec![iv(1, 300)]);
        let cidrs = set.to_cidrs();
        let total: u64 = cidrs.iter().map(|(_, p)| 1u64 << (32 - p)).sum();
        assert_eq!(total, 300);
        // each block aligned
        for (addr, p) in &cidrs {
            let v: u32 = (*addr).into();
            let size = 1u64 << (32 - p);
            assert_eq!(v as u64 % size, 0);
        }
    }
}
