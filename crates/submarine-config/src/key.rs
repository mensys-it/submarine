//! Curve25519 key types used in configurations, encoded as base64 like in wg-quick.
//!
//! Secret keys are wiped from memory on drop and NEVER printed by `Debug`.

use std::fmt;
use std::str::FromStr;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::ConfigError;

/// Length in bytes of a WireGuard key.
pub const KEY_LEN: usize = 32;

/// Decodes a base64 key (surrounding whitespace is ignored).
///
/// # Errors
/// [`ConfigError::InvalidKey`] if the input is not base64 or not exactly [`KEY_LEN`] bytes.
fn decode(s: &str) -> Result<[u8; KEY_LEN], ConfigError> {
    let mut bytes = STANDARD
        .decode(s.trim())
        .map_err(|_| ConfigError::InvalidKey)?;
    let key = <[u8; KEY_LEN]>::try_from(bytes.as_slice()).map_err(|_| ConfigError::InvalidKey);
    // the intermediate buffer may hold a secret key: it is wiped before being freed
    bytes.zeroize();
    key
}

/// A Curve25519 public key.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct PublicKey([u8; KEY_LEN]);

impl PublicKey {
    /// Wraps raw key bytes.
    pub fn from_bytes(bytes: [u8; KEY_LEN]) -> Self {
        Self(bytes)
    }

    /// Returns the raw key bytes.
    pub fn as_bytes(&self) -> &[u8; KEY_LEN] {
        &self.0
    }

    /// Encodes the key as standard (padded) base64, as used in `.conf` files.
    pub fn to_base64(&self) -> String {
        STANDARD.encode(self.0)
    }
}

impl FromStr for PublicKey {
    type Err = ConfigError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        decode(s).map(Self)
    }
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PublicKey({})", self.to_base64())
    }
}

impl fmt::Display for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_base64())
    }
}

/// A secret key (private key or preshared key). Wiped from memory on drop
/// and redacted in `Debug` output.
#[derive(Clone, PartialEq, Eq, Zeroize, ZeroizeOnDrop)]
pub struct SecretKey([u8; KEY_LEN]);

impl SecretKey {
    /// Wraps raw key bytes.
    pub fn from_bytes(bytes: [u8; KEY_LEN]) -> Self {
        Self(bytes)
    }

    /// Returns the raw key bytes.
    pub fn as_bytes(&self) -> &[u8; KEY_LEN] {
        &self.0
    }

    /// Encodes the key as standard (padded) base64.
    ///
    /// NB: the returned `String` is NOT wiped on drop.
    pub fn to_base64(&self) -> String {
        STANDARD.encode(self.0)
    }
}

impl FromStr for SecretKey {
    type Err = ConfigError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        decode(s).map(Self)
    }
}

impl fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretKey(<redacted>)")
    }
}

/// Implements `Serialize`/`Deserialize` for a key type as a base64 string.
macro_rules! serde_base64 {
    ($ty:ty) => {
        impl Serialize for $ty {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(&self.to_base64())
            }
        }

        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let s = String::deserialize(deserializer)?;
                s.parse().map_err(serde::de::Error::custom)
            }
        }
    };
}

serde_base64!(PublicKey);
serde_base64!(SecretKey);
