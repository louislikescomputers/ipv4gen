//! # ipgen-core
//!
//! Algorithmic IPv4 address / range / CIDR generator with exact pause & resume.
//!
//! **Non-goals:** this crate contains *no networking code* — no sockets, no
//! probing, no port scanning, no DNS. It only enumerates addresses. Callers
//! must only use the output against networks they own or are authorized to
//! examine.
//!
//! ## Quick start
//!
//! ```
//! use ipgen_core::{Config, Generator, Order, Preset};
//!
//! let cfg = Config::builder()
//!     .preset(Preset::Public)
//!     .order(Order::Sequential)
//!     .build()
//!     .unwrap();
//!
//! let mut g = Generator::new(cfg).unwrap();
//! let first: Vec<_> = g.by_ref().take(3).collect();
//! assert_eq!(first[0].to_string(), "1.0.0.0");
//! ```
#![forbid(unsafe_code)]

pub mod categories;
pub mod checkpoint;
pub mod config;
pub mod error;
pub mod generator;
pub mod interval;
pub mod order;

pub use categories::{classify, ranges_for, Category};
pub use checkpoint::{Checkpoint, CHECKPOINT_VERSION};
pub use config::{Config, ConfigBuilder, Preset, DEFAULT_PREFIX_LEN};
pub use error::Error;
pub use generator::{CidrBlock, Generator};
pub use interval::{parse_cidr, parse_range_spec, Interval, IntervalSet};
pub use order::{Order, Shard};

/// Re-export of [`std::net::Ipv4Addr`] for convenience at the API edges.
pub use std::net::Ipv4Addr;
