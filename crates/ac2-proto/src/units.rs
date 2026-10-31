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
    /// True-peak level re digital full scale (BS.1770 Annex 2: the inter-sample peak the
    /// converter will see), so a value above 0 is possible.
    Dbtp
);
float_unit!(
    /// Programme loudness, LUFS (BS.1770 / EBU R128: K-weighted, channel-weighted mean
    /// square, `−0.691 + 10 log₁₀`, so a 1 kHz stereo tone at −18 dBFS reads −18 LUFS).
    /// A loudness difference (LRA) is a [`Db`], shown as LU.
    Lufs
);
float_unit!(
    /// Sound pressure level, dB re 20 µPa.
    DbSpl
);
float_unit!(
    /// Angle, degrees.
    Degrees
);
float_unit!(
    /// Voltage, volts RMS.
    Volts
);
float_unit!(
    /// Line level, dB re 0.7746 V RMS (1 mW into 600 Ω).
    Dbu
);
float_unit!(
    /// Microphone sensitivity, millivolts per pascal (the unit data sheets state it in).
    MvPerPa
);
float_unit!(
    /// Impedance magnitude, ohms.
    Ohms
);
float_unit!(
    /// Loudness level, phon: the level in dB SPL of a 1 kHz tone heard as equally loud.
    Phon
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
/// A daemon's identity: random (never 0), made once and kept across restarts, renames,
/// address changes and a move between local and network mode, so that an app holding
/// several rigs tells them apart by this and not by where they are reached. It cannot be
/// the CURVE key, which a local or embedded daemon does not have. Shown as 16 lowercase
/// hex digits, the form of its file (`<config>/rig-id`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RigId(pub u64);

impl fmt::Display for RigId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

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
    /// A spatial walk, unique within the session.
    WalkId(u32)
);
id_type!(
    /// An EQ set of a measurement, unique within the session.
    EqSetId(u32)
);
id_type!(
    /// One `sweep.run` run, per incarnation.
    SweepId(u32)
);
id_type!(
    /// One `sim.run` or `sim.fit`, per incarnation.
    SimId(u32)
);
id_type!(
    /// A session archive being uploaded or downloaded (`file.upload_begin`,
    /// `file.download_begin`), per incarnation.
    TransferId(u32)
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
