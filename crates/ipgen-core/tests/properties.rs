//! Property/integration tests for ipgen-core (masterprompt 9 tests 4-8).

use ipgen_core::checkpoint::Checkpoint;
use ipgen_core::{classify, Config, Generator, IntervalSet, Order, Preset, Shard};
use proptest::prelude::*;
use std::collections::HashSet;
use std::net::Ipv4Addr;

fn small_cfg(seed: u64, order: Order) -> Config {
    Config::builder()
        .categories(&[])
        .include("10.0.0.0/24")
        .include("192.0.2.0/25")
        .include("172.16.32.0-172.16.32.99")
        .exclude("10.0.0.128/25")
        .order(order)
        .shard(Shard { index: seed % 3, count: 3 })
        .build()
        .unwrap()
}

// ---- Test 4: Feistel bijection sweep + resume through generator ----

proptest! {
    #![proptest_config(ProptestConfig::with_cases(8))]
    #[test]
    fn feistel_bijection_via_generator(n in 1usize..256, seed in any::<u64>()) {
        let cfg = Config::builder()
            .categories(&[])
            .include(format!("0.0.0.0-0.0.{}", (n - 1)))
            .order(Order::Permuted { seed })
            .build()
            .unwrap();
        let mut g = Generator::new(cfg).unwrap();
        let mut seen = HashSet::new();
        while let Some(ip) = g.next() {
            prop_assert!(seen.insert(ip), "duplicate address {ip}");
        }
        prop_assert_eq!(seen.len(), n);
    }
}

#[test]
fn feistel_large_random_domains() {
    // verify distinctness through the *generator* over a real interval set
    // with a shard domain larger than 2^20.
    let cfg = Config::builder()
        .preset(Preset::Private)
        .order(Order::Permuted { seed: 0xC0FFEE })
        .build()
        .unwrap();
    let total = cfg.shard_total().unwrap();
    assert!(total >= 1 << 20);
    let mut g = Generator::new(cfg).unwrap();
    let mut seen = HashSet::new();
    for _ in 0..200_000 {
        let ip = g.next().unwrap();
        assert!(seen.insert(ip));
    }
    assert_eq!(g.cursor(), 200_000);
}

// ---- Test 5: resume equivalence ----

fn take_n(g: &mut Generator, n: usize) -> Vec<Ipv4Addr> {
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        match g.next() {
            Some(ip) => out.push(ip),
            None => break,
        }
    }
    out
}

fn take_batches(g: &mut Generator, chunks: &[usize]) -> Vec<Ipv4Addr> {
    let mut out = Vec::new();
    for &c in chunks {
        out.extend(g.next_batch(c).unwrap_or_default());
    }
    out
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]
    #[test]
    fn resume_equals_uninterrupted(
        seed in any::<u64>(),
        k in 1usize..300,
        use_shard in any::<bool>(),
        permuted in any::<bool>(),
    ) {
        let order = if permuted { Order::Permuted { seed } } else { Order::Sequential };
        let shard = if use_shard { Shard { index: seed % 3, count: 3 } } else { Shard::one() };
        let cfg = Config {
            categories: vec![ipgen_core::Category::Private],
            extra_include: vec!["192.0.2.0/24".into()],
            extra_exclude: vec!["10.7.0.0/16".into()],
            order,
            shard,
        };
        let mut reference = Generator::new(cfg.clone()).unwrap();
        let want = reference.total();
        let full = take_batches(&mut reference, &[want as usize]);

        let mut paused = Generator::new(cfg).unwrap();
        let first = take_n(&mut paused, k.min(want as usize));
        let ckpt = paused.checkpoint();
        let json = serde_json::to_string(&ckpt).unwrap();
        let loaded = Checkpoint::from_json_bytes(json.as_bytes()).unwrap();
        let mut resumed = Generator::resume(loaded).unwrap();
        let rest = take_batches(&mut resumed, &[want as usize]);

        let combined: Vec<Ipv4Addr> = first.into_iter().chain(rest).collect();
        prop_assert_eq!(combined.len(), full.len());
        prop_assert_eq!(&combined, &full);
    }
}

// ---- Test 6: shard coverage for M in {1,2,3,7} ----

#[test]
fn shard_coverage_sequential_small() {
    for m in [1u64, 2, 3, 7] {
        let base = Config::builder()
            .categories(&[])
            .include("10.0.0.0-10.0.0.49")
            .order(Order::Sequential)
            .build()
            .unwrap();
        let mut got: Vec<Ipv4Addr> = Vec::new();
        for k in 0..m {
            let cfg = Config { shard: Shard { index: k, count: m }, ..base.clone() };
            let mut g = Generator::new(cfg).unwrap();
            got.extend(g.by_ref());
        }
        got.sort();
        let expected: Vec<Ipv4Addr> = (0..50).map(|i| Ipv4Addr::new(10, 0, 0, i)).collect();
        assert_eq!(got, expected, "M={m}");
    }
}

#[test]
fn shard_sizes_partition_total() {
    // exhaustive coverage check on a mid-size private set, permuted order
    for m in [1u64, 2, 3, 7] {
        let base = Config::builder()
            .preset(Preset::Private)
            .exclude("10.128.0.0/9")
            .order(Order::Permuted { seed: 99 })
            .build()
            .unwrap();
        let full_total = base.total_allowed().unwrap();
        let mut all = HashSet::new();
        let mut sum_len = 0u64;
        for k in 0..m {
            let cfg = Config { shard: Shard { index: k, count: m }, ..base.clone() };
            let mut g = Generator::new(cfg).unwrap();
            assert_eq!(g.total(), Shard { index: k, count: m }.len(full_total));
            sum_len += g.total();
            // exhaust fully only when m==1 or m==2 to bound runtime; for
            // m>=3 sample 300k per shard and check disjointness there.
            let limit = if m <= 2 { usize::MAX } else { 300_000 };
            let mut c = 0usize;
            while c < limit {
                match g.next() {
                    Some(ip) => { assert!(all.insert(ip), "shard overlap (M={m})"); c += 1; }
                    None => break,
                }
            }
        }
        assert_eq!(sum_len, full_total);
        if m <= 2 {
            assert_eq!(all.len() as u64, full_total);
        }
    }
}

// ---- Test 7: checkpoint tamper (cursor ok, config not) ----

#[test]
fn checkpoint_tamper_semantics() {
    let cfg = small_cfg(1, Order::Permuted { seed: 5 });
    let mut g = Generator::new(cfg).unwrap();
    let _ = take_n(&mut g, 20);
    let mut ck = g.checkpoint();
    ck.cursor = 7;
    ck.validate().unwrap();
    let gg = Generator::resume(ck).unwrap();
    assert_eq!(gg.cursor(), 7);
    let mut ck2 = g.checkpoint();
    ck2.config.extra_exclude.push("10.0.0.5".into());
    assert!(Generator::resume(ck2).is_err());
}

// ---- nth strictly increasing & in range ----

#[test]
fn nth_strictly_increasing() {
    let set = IntervalSet::new(vec![
        ipgen_core::Interval { start: 5, end: 9 },
        ipgen_core::Interval { start: 100, end: 199 },
        ipgen_core::Interval { start: 1000, end: 1004 },
    ]);
    let mut prev = None;
    for i in 0..set.len() {
        let v = set.nth(i).unwrap();
        if let Some(p) = prev {
            assert!(v > p);
        }
        assert!(set.contains(v));
        prev = Some(v);
    }
    assert_eq!(set.nth(set.len()), None);
}

// ---- Test 9: ignored full-space bitset test ----

/// Generates every address for `Preset::All` in permuted order into a 512 MiB
/// bitset and asserts all 2^32 bits are set exactly once.
/// Run with: `cargo test --release -p ipgen-core -- --ignored`
#[test]
#[ignore]
fn full_space_permuted_bitset() {
    const BYTES: usize = (1usize << 32) / 8;
    let mut bits: Vec<u8> = vec![0u8; BYTES];
    let cfg = Config::builder()
        .preset(Preset::All)
        .order(Order::Permuted { seed: 0xA5A5 })
        .build()
        .unwrap();
    let mut g = Generator::new(cfg).unwrap();
    assert_eq!(g.total(), 4_294_967_296);
    let mut buf = vec![0u32; 1 << 20];
    let mut emitted: u64 = 0;
    loop {
        let got = g.next_batch_into(&mut buf);
        if got == 0 {
            break;
        }
        for &v in &buf[..got] {
            let byte = (v >> 3) as usize;
            let mask = 1u8 << (v & 7);
            assert_eq!(bits[byte] & mask, 0, "address {v} emitted twice");
            bits[byte] |= mask;
        }
        emitted += got as u64;
    }
    assert_eq!(emitted, 4_294_967_296);
    assert!(bits.iter().all(|b| *b == 0xFF), "not all addresses were emitted");
}

#[test]
fn classify_smoke_from_tests() {
    assert_eq!(classify(Ipv4Addr::new(1, 1, 1, 1)), ipgen_core::Category::Public);
}
