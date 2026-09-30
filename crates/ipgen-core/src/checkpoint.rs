//! [`Checkpoint`] — serializable, atomically-written session state.

use crate::config::Config;
use crate::error::Error;
use crate::generator::Generator;
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

/// Current checkpoint format version.
pub const CHECKPOINT_VERSION: u32 = 1;

/// A snapshot of a generation session. The whole state is `(config, cursor)`
/// plus bookkeeping fields; serialized as JSON.
///
/// ```no_run
/// use ipgen_core::{Checkpoint, Config, Generator, Preset, Order};
/// let cfg = Config::builder().preset(Preset::Public).order(Order::Sequential).build().unwrap();
/// let mut g = Generator::new(cfg).unwrap();
/// let _ = g.next();
/// let ckpt = g.checkpoint();
/// ckpt.save_atomic("state.json").unwrap();           // atomic: temp + fsync + rename
/// let loaded = Checkpoint::load("state.json").unwrap();
/// let _g = Generator::resume(loaded).unwrap();
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checkpoint {
    pub version: u32,
    pub session_id: String,
    pub config: Config,
    /// FNV-1a 64 of the canonical JSON of `config` (dependency-free hash).
    pub config_hash: u64,
    /// Total addresses in this (sharded) stream.
    pub total: u64,
    /// Shard-local cursor: next stream position to emit.
    pub cursor: u64,
    /// Addresses emitted so far.
    pub emitted: u64,
    /// Unix seconds.
    pub created_at: u64,
    /// Unix seconds.
    pub updated_at: u64,
}

impl Checkpoint {
    /// Snapshot from a live generator.
    pub(crate) fn from_generator(g: &Generator) -> Checkpoint {
        let now = unix_now();
        Checkpoint {
            version: CHECKPOINT_VERSION,
            session_id: new_session_id(now),
            config: g.config().clone(),
            config_hash: g.config().hash(),
            total: g.total(),
            cursor: g.cursor(),
            emitted: g.emitted(),
            created_at: now,
            updated_at: now,
        }
    }

    /// Build with an explicit session id (used by the MCP server).
    pub fn with_session_id(mut self, id: impl Into<String>) -> Self {
        self.session_id = id.into();
        self
    }

    /// Validate version and recompute the config hash; refuses tampered or
    /// corrupted configs.
    pub fn validate(&self) -> Result<(), Error> {
        if self.version != CHECKPOINT_VERSION {
            return Err(Error::CheckpointInvalid(format!(
                "unsupported checkpoint version {} (expected {})",
                self.version, CHECKPOINT_VERSION
            )));
        }
        let recomputed = self.config.hash();
        if recomputed != self.config_hash {
            return Err(Error::CheckpointInvalid(format!(
                "config hash mismatch: stored {:#018x}, recomputed {:#018x} (tampering or corruption)",
                self.config_hash, recomputed
            )));
        }
        Ok(())
    }

    /// Load and parse a checkpoint file. Garbage input yields `Err`, never a
    /// panic.
    pub fn load(path: impl AsRef<Path>) -> Result<Checkpoint, Error> {
        let data = fs::read(path.as_ref())?;
        let ckpt: Checkpoint = serde_json::from_slice(&data)
            .map_err(|e| Error::CheckpointInvalid(format!("not valid checkpoint JSON: {e}")))?;
        Ok(ckpt)
    }

    /// Parse from bytes (used by tests / MCP store).
    pub fn from_json_bytes(data: &[u8]) -> Result<Checkpoint, Error> {
        serde_json::from_slice(data)
            .map_err(|e| Error::CheckpointInvalid(format!("not valid checkpoint JSON: {e}")))
    }

    /// Atomically write the checkpoint: temp file in the same directory →
    /// write → flush → fsync → rename over the target. A crash between any
    /// steps leaves the previous checkpoint intact.
    pub fn save_atomic(&self, path: impl AsRef<Path>) -> Result<(), Error> {
        let path = path.as_ref();
        let json = serde_json::to_vec_pretty(self)
            .map_err(|e| Error::CheckpointInvalid(format!("serialize failed: {e}")))?;
        save_atomic_bytes(&json, path)
    }
}

/// Atomic byte write used by `save_atomic` (and testable on its own).
pub fn save_atomic_bytes(bytes: &[u8], path: &Path) -> Result<(), Error> {
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty());
    let tmp = match dir {
        Some(d) => {
            fs::create_dir_all(d)?;
            d.join(format!(".{}.tmp", file_name_of(path)))
        }
        None => Path::new(&format!("{}.tmp", file_name_of(path))).to_path_buf(),
    };
    {
        let mut f = File::create(&tmp)?;
        f.write_all(bytes)?;
        f.flush()?;
        f.sync_all().map_err(|e| Error::Io(format!("fsync: {e}")))?;
    }
    fs::rename(&tmp, path)?;
    // best-effort directory fsync so the rename itself is durable
    if let Some(d) = dir {
        if let Ok(df) = File::open(d) {
            let _ = df.sync_all();
        }
    }
    Ok(())
}

fn file_name_of(p: &Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "checkpoint".to_string())
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Deterministic-ish unique session id without external deps: time + counter
/// + a cheap address-space hash.
pub fn new_session_id(now: u64) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let c = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    format!("{:08x}-{:04x}-{:08x}", now as u32, pid ^ (c.wrapping_mul(2654435761) as u32), c)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::order::Order;
    use crate::{Config, Preset};

    fn sample_ckpt() -> Checkpoint {
        let cfg = Config::builder()
            .preset(Preset::Private)
            .order(Order::Permuted { seed: 3 })
            .build()
            .unwrap();
        let mut g = Generator::new(cfg).unwrap();
        for _ in 0..10 {
            let _ = g.next();
        }
        g.checkpoint()
    }

    #[test]
    fn roundtrip_and_validate() {
        let ck = sample_ckpt();
        assert_eq!(ck.cursor, 10);
        ck.validate().unwrap();
        let dir = std::env::temp_dir().join(format!("ipgen-ck-test-{}", std::process::id()));
        let p = dir.join("state.json");
        ck.save_atomic(&p).unwrap();
        let back = Checkpoint::load(&p).unwrap();
        assert_eq!(ck, back);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn tamper_detection() {
        let mut ck = sample_ckpt();
        // modifying cursor alone is allowed
        ck.cursor = 5;
        ck.validate().unwrap();
        // modifying config without fixing hash is rejected
        let mut bad = sample_ckpt();
        bad.config.extra_exclude.push("10.5.0.0/16".into());
        match bad.validate() {
            Err(Error::CheckpointInvalid(_)) => {}
            other => panic!("expected hash mismatch, got {other:?}"),
        }
    }

    #[test]
    fn garbage_file_is_err_not_panic() {
        let dir = std::env::temp_dir().join(format!("ipgen-ck-garbage-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join("bad.json");
        fs::write(&p, b"{ not json at all \x00\x01 truncated").unwrap();
        assert!(matches!(Checkpoint::load(&p), Err(Error::CheckpointInvalid(_))));
        // empty file
        fs::write(&p, b"").unwrap();
        assert!(Checkpoint::load(&p).is_err());
        // nonexistent
        assert!(Checkpoint::load(dir.join("missing.json")).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn atomic_save_failure_keeps_old_checkpoint() {
        let dir = std::env::temp_dir().join(format!("ipgen-ck-atomic-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join("state.json");
        let ck1 = sample_ckpt();
        ck1.save_atomic(&p).unwrap();
        let original = fs::read(&p).unwrap();

        // Simulate a crash between temp-write and rename: leave a stale temp
        // file behind; the real checkpoint must be untouched.
        let tmp = dir.join(format!(".{}.tmp", file_name_of(&p)));
        fs::write(&tmp, b"half written garbage").unwrap();
        // (in a crash the rename never happens) — verify target intact:
        assert_eq!(fs::read(&p).unwrap(), original);
        let back = Checkpoint::load(&p).unwrap();
        assert_eq!(back, ck1);

        // Now a successful save replaces both cleanly.
        let mut ck2 = ck1.clone();
        ck2.cursor = 42;
        ck2.updated_at += 1;
        ck2.save_atomic(&p).unwrap();
        assert_eq!(Checkpoint::load(&p).unwrap().cursor, 42);
        assert!(!tmp.exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
