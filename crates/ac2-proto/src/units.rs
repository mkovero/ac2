//! Unit newtypes and identifiers.
//!
//! Every physical quantity on the wire is a newtype so that a field's unit is part of its
//! type. All of them serialize transparently (a msgpack float or integer), so the wire stays
//! plain for non-Rust clients.

use std::fmt;

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

macro_rules! float_unit {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Default, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub f64);
    };
}

float_unit!(
    /// Frequency, hertz.
    Hz
);
float_unit!(
    /// Time span, seconds.
    Seconds
);
float_unit!(
    /// Level ratio, decibels (relative; a gain, offset or difference).
    Db
);
float_unit!(
    /// Level re digital full scale; 0 dBFS = RMS of a full-scale sine (decision 4a).
    Dbfs
);
float_unit!(
    /// Sound pressure level, dB re 20 µPa.
    DbSpl
);
float_unit!(
    /// Angle, degrees.
    Degrees
);

/// Signed sample count (offsets, delays in samples).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Samples(pub i64);

/// Wall-clock time, nanoseconds since the Unix epoch (daemon clock).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct WallNs(pub u64);

/// Index in the session's sample clock; origin = session open.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct SampleIndex(pub u64);

macro_rules! id_type {
    ($(#[$doc:meta])* $name:ident($inner:ty)) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub $inner);

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }
    };
}

id_type!(
    /// Revision of the daemon's state; one counter per incarnation, bumped by every commit.
    Rev(u64)
);
id_type!(
    /// Random per daemon process start.
    DaemonIncarnation(u64)
);
id_type!(
    /// Per incarnation; changes on session open/close, device, rate or buffer change.
    SessionEpoch(u32)
);
id_type!(
    /// A measurement job.
    MeasId(u32)
);
id_type!(
    /// A stored trace.
    TraceId(u32)
);
id_type!(
    /// One `ir.capture` run, per incarnation.
    SweepId(u32)
);
id_type!(
    /// Request id, unique per client within the dedup window.
    RequestId(u64)
);

/// Client identity: the ZAP User-Id under CURVE, otherwise a daemon-assigned id.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ClientId(pub String);

/// Stimulus lease token: 128 random bits, carried as a 16-byte msgpack `bin` (big-endian).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LeaseToken(pub u128);

impl Serialize for LeaseToken {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bytes(&self.0.to_be_bytes())
    }
}

impl<'de> Deserialize<'de> for LeaseToken {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl Visitor<'_> for V {
            type Value = LeaseToken;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("16 bytes")
            }
            fn visit_bytes<E: de::Error>(self, b: &[u8]) -> Result<LeaseToken, E> {
                let a: [u8; 16] = b
                    .try_into()
                    .map_err(|_| E::invalid_length(b.len(), &self))?;
                Ok(LeaseToken(u128::from_be_bytes(a)))
            }
        }
        d.deserialize_bytes(V)
    }
}

/// Opaque bytes carried as msgpack `bin` (never base64, never an integer array).
#[derive(Clone, PartialEq, Eq, Default)]
pub struct Blob(pub Vec<u8>);

impl fmt::Debug for Blob {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Blob({} bytes)", self.0.len())
    }
}

impl Serialize for Blob {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bytes(&self.0)
    }
}

impl<'de> Deserialize<'de> for Blob {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl Visitor<'_> for V {
            type Value = Blob;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("bytes")
            }
            fn visit_bytes<E: de::Error>(self, b: &[u8]) -> Result<Blob, E> {
                Ok(Blob(b.to_vec()))
            }
            fn visit_byte_buf<E: de::Error>(self, b: Vec<u8>) -> Result<Blob, E> {
                Ok(Blob(b))
            }
        }
        d.deserialize_byte_buf(V)
    }
}
