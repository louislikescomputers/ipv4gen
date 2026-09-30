use std::fmt;

/// All errors produced by `ipgen-core`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// A CIDR string could not be parsed (`a.b.c.d/len`).
    BadCidr(String),
    /// An address range string could not be parsed (`a.b.c.d-e.f.g.h`).
    BadRange(String),
    /// A dotted-quad IPv4 address could not be parsed.
    BadAddress(String),
    /// A category name could not be parsed.
    BadCategory(String),
    /// A shard spec `K/M` was invalid (non-numeric, or `K >= M`, or `M == 0`).
    BadShard(String),
    /// The selected configuration contains zero addresses.
    EmptySelection,
    /// The cursor is outside `[0, total)`.
    IndexOutOfRange { index: u64, total: u64 },
    /// A prefix length was out of the supported range.
    BadPrefixLen(u8),
    /// A checkpoint file failed validation (bad version, hash mismatch…).
    CheckpointInvalid(String),
    /// An I/O error while saving/loading a checkpoint.
    Io(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::BadCidr(s) => write!(f, "invalid CIDR notation: '{s}'"),
            Error::BadRange(s) => write!(f, "invalid range notation: '{s}'"),
            Error::BadAddress(s) => write!(f, "invalid IPv4 address: '{s}'"),
            Error::BadCategory(s) => write!(f, "unknown category: '{s}'"),
            Error::BadShard(s) => write!(f, "invalid shard specification: '{s}' (expected K/M with K < M)"),
            Error::EmptySelection => write!(f, "the selection contains no addresses"),
            Error::IndexOutOfRange { index, total } => {
                write!(f, "index {index} out of range for {total} addresses")
            }
            Error::BadPrefixLen(p) => write!(f, "invalid prefix length {p} (must be 8..=32)"),
            Error::CheckpointInvalid(s) => write!(f, "checkpoint invalid: {s}"),
            Error::Io(s) => write!(f, "I/O error: {s}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e.to_string())
    }
}
