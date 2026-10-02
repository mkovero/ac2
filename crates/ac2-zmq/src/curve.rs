//! CURVE keys, Z85 text encoding, and the client-side CURVE configuration.
//!
//! Server-side CURVE is only available through [`crate::SecureContext`], which guarantees a
//! running ZAP handler.

use std::ffi::CString;
use std::fmt;
use std::str::FromStr;

use crate::error::{Error, Result, zmq};
use crate::raw;

/// Encodes `data` as Z85 (ZeroMQ RFC 32). The length must be a multiple of 4.
pub fn z85_encode(data: &[u8]) -> Result<String> {
    raw::z85_encode(data).ok_or(Error::InvalidArgument(
        "Z85 input length must be a multiple of 4",
    ))
}

/// Decodes Z85 text. The length must be a multiple of 5 and every character in the alphabet.
pub fn z85_decode(text: &str) -> Result<Vec<u8>> {
    let c = CString::new(text).map_err(|_| Error::InvalidArgument("Z85 text contains NUL"))?;
    raw::z85_decode(&c).ok_or(Error::InvalidArgument("not valid Z85 text"))
}

fn key_from_z85(text: &str) -> Result<[u8; 32]> {
    if text.len() != 40 {
        return Err(Error::InvalidKey);
    }
    let mut bytes = z85_decode(text).map_err(|_| Error::InvalidKey)?;
    let key = <[u8; 32]>::try_from(bytes.as_slice()).map_err(|_| Error::InvalidKey);
    raw::zeroize(&mut bytes);
    key
}

/// A CURVE long-term public key (32 bytes). Displays as 40 characters of Z85.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PublicKey([u8; 32]);

impl PublicKey {
    /// From the raw 32 bytes (the form ZAP passes as CURVE credentials).
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The raw 32 bytes.
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// From 40 characters of Z85.
    pub fn from_z85(text: &str) -> Result<Self> {
        key_from_z85(text).map(Self)
    }

    /// 40 characters of Z85.
    pub fn to_z85(&self) -> String {
        raw::z85_encode(&self.0).unwrap_or_default()
    }

    /// Short, human-comparable fingerprint: the first 10 bytes of SHA-256 over the 32 raw
    /// key bytes, as five dash-separated groups of four lowercase hex digits
    /// (`1a2b-3c4d-5e6f-7a8b-9c0d`). 80 bits are enough for a person comparing two strings
    /// (a second-preimage search is out of reach); it never replaces the key itself, which
    /// CURVE pins.
    pub fn fingerprint(&self) -> String {
        use sha2::{Digest, Sha256};
        let d = Sha256::digest(self.0);
        d[..10]
            .chunks(2)
            .map(|c| format!("{:02x}{:02x}", c[0], c[1]))
            .collect::<Vec<_>>()
            .join("-")
    }
}

impl fmt::Display for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_z85())
    }
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PublicKey({})", self.to_z85())
    }
}

impl FromStr for PublicKey {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        Self::from_z85(s)
    }
}

/// A CURVE secret key (32 bytes). Never printed; zeroed on drop.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretKey([u8; 32]);

impl SecretKey {
    /// From the raw 32 bytes.
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The raw 32 bytes.
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// From 40 characters of Z85.
    pub fn from_z85(text: &str) -> Result<Self> {
        key_from_z85(text).map(Self)
    }

    /// 40 characters of Z85, for writing a key file. Handle the result as secret.
    pub fn to_z85(&self) -> String {
        raw::z85_encode(&self.0).unwrap_or_default()
    }
}

impl Drop for SecretKey {
    fn drop(&mut self) {
        raw::zeroize(&mut self.0);
    }
}

impl fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretKey(..)")
    }
}

/// A CURVE long-term keypair.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyPair {
    /// Shared with peers (server: pinned by clients; client: listed in authorized keys).
    pub public: PublicKey,
    /// Never leaves this host.
    pub secret: SecretKey,
}

impl KeyPair {
    /// A fresh keypair from libsodium's CSPRNG.
    pub fn generate() -> Result<Self> {
        let (public, secret) = zmq(raw::curve_keypair())?;
        Ok(Self {
            public: PublicKey(public),
            secret: SecretKey(secret),
        })
    }
}

/// Client-side CURVE: this client's keypair and the server key it pins. Apply with
/// [`crate::Socket::set_curve_client`] before `connect`.
#[derive(Clone, Debug)]
pub struct CurveClient {
    /// This client's long-term keypair; its public half must be in the server's authorized
    /// keys.
    pub keys: KeyPair,
    /// The server's long-term public key. The handshake fails unless the server proves it
    /// holds the matching secret key.
    pub server_key: PublicKey,
}
