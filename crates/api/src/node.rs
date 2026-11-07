//! Relay global object identification.
//!
//! Relay requires every object to carry an `id` that is unique across the whole schema and
//! sufficient to refetch it through a single `node(id:)` field. A per-type numeric id is not
//! enough, since library 4 and metadata item 4 would collide, so the id encodes the type
//! alongside the local key.
//!
//! The encoding is opaque by contract: clients must treat it as a token and never parse it.
//! Base64 is the Relay convention precisely because it discourages that.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;

#[derive(Debug, thiserror::Error)]
pub enum GlobalIdError {
    #[error("global id is not valid base64")]
    NotBase64,
    #[error("global id is not valid UTF-8")]
    NotUtf8,
    #[error("global id is not of the form Type:key")]
    Malformed,
    #[error("global id key is not a number")]
    KeyNotNumeric,
}

/// A decoded global id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalId {
    pub type_name: String,
    pub key: i64,
}

impl GlobalId {
    pub fn new(type_name: impl Into<String>, key: i64) -> Self {
        Self {
            type_name: type_name.into(),
            key,
        }
    }

    /// Encodes as Relay expects: base64 of `Type:key`.
    pub fn encode(type_name: &str, key: i64) -> String {
        STANDARD.encode(format!("{type_name}:{key}"))
    }

    /// # Errors
    ///
    /// Fails when the value is not base64, not UTF-8, not `Type:key`, or carries a
    /// non-numeric key. All four are client mistakes rather than server faults.
    pub fn decode(encoded: &str) -> Result<Self, GlobalIdError> {
        let bytes = STANDARD
            .decode(encoded)
            .map_err(|_| GlobalIdError::NotBase64)?;
        let decoded = String::from_utf8(bytes).map_err(|_| GlobalIdError::NotUtf8)?;
        let (type_name, key) = decoded.split_once(':').ok_or(GlobalIdError::Malformed)?;
        if type_name.is_empty() {
            return Err(GlobalIdError::Malformed);
        }
        Ok(Self {
            type_name: type_name.to_owned(),
            key: key.parse().map_err(|_| GlobalIdError::KeyNotNumeric)?,
        })
    }
}

#[cfg(test)]
mod tests {
    // A panicking test is a failing test, which is the intended reporting mechanism.
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

    #[test]
    fn round_trips() {
        let encoded = GlobalId::encode("Library", 4);
        let decoded = GlobalId::decode(&encoded).expect("round trip");
        assert_eq!(decoded, GlobalId::new("Library", 4));
    }

    #[test]
    fn distinguishes_types_sharing_a_key() {
        // The collision Relay's global ids exist to prevent.
        assert_ne!(
            GlobalId::encode("Library", 4),
            GlobalId::encode("Metadata", 4)
        );
    }

    #[test]
    fn is_opaque_rather_than_readable() {
        assert_eq!(GlobalId::encode("Library", 4), "TGlicmFyeTo0");
    }

    #[test]
    fn rejects_malformed_input() {
        assert!(matches!(
            GlobalId::decode("!!!not base64!!!"),
            Err(GlobalIdError::NotBase64)
        ));
        assert!(matches!(
            GlobalId::decode(&STANDARD.encode("no-separator")),
            Err(GlobalIdError::Malformed)
        ));
        assert!(matches!(
            GlobalId::decode(&STANDARD.encode("Library:abc")),
            Err(GlobalIdError::KeyNotNumeric)
        ));
        assert!(matches!(
            GlobalId::decode(&STANDARD.encode(":4")),
            Err(GlobalIdError::Malformed)
        ));
    }
}
