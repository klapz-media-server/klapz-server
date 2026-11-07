//! Opaque connection cursors.
//!
//! Relay requires a cursor to be an opaque string: a client must be able to hand it back
//! without understanding it, and the server must be free to change what it encodes. Exposing
//! the raw ordering value would make that a breaking change the day a collection gains a
//! second sort field.
//!
//! The encoding carries the ordering column's value and the row id that breaks ties, because
//! a title-sorted list cannot be resumed from an id alone.

use async_graphql::connection::CursorType;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use open_plex_core::{Cursor, CursorKey};

/// A [`Cursor`] in its wire form.
#[derive(Debug, Clone, PartialEq)]
pub struct OpaqueCursor(pub Cursor);

#[derive(Debug, thiserror::Error)]
pub enum CursorError {
    #[error("cursor is not valid base64")]
    NotBase64,
    #[error("cursor is not valid UTF-8")]
    NotUtf8,
    #[error("cursor is malformed")]
    Malformed,
}

impl CursorType for OpaqueCursor {
    type Error = CursorError;

    fn encode_cursor(&self) -> String {
        // `i:<id>` for an id-ordered collection, `t:<id>:<text>` or `n:<id>:<number>` when a
        // sort key is involved. The id comes before the key so the key may contain colons.
        let plain = match &self.0.key {
            None => format!("i:{}", self.0.id),
            Some(CursorKey::Text(text)) => format!("t:{}:{text}", self.0.id),
            Some(CursorKey::Int(value)) => format!("n:{}:{value}", self.0.id),
        };
        STANDARD.encode(plain)
    }

    fn decode_cursor(encoded: &str) -> Result<Self, Self::Error> {
        let bytes = STANDARD
            .decode(encoded)
            .map_err(|_| CursorError::NotBase64)?;
        let plain = String::from_utf8(bytes).map_err(|_| CursorError::NotUtf8)?;

        let (tag, rest) = plain.split_once(':').ok_or(CursorError::Malformed)?;
        match tag {
            "i" => Ok(Self(Cursor::by_id(
                rest.parse().map_err(|_| CursorError::Malformed)?,
            ))),
            "t" | "n" => {
                let (id, key) = rest.split_once(':').ok_or(CursorError::Malformed)?;
                let id = id.parse().map_err(|_| CursorError::Malformed)?;
                let key = if tag == "t" {
                    CursorKey::Text(key.to_owned())
                } else {
                    CursorKey::Int(key.parse().map_err(|_| CursorError::Malformed)?)
                };
                Ok(Self(Cursor::new(key, id)))
            }
            _ => Err(CursorError::Malformed),
        }
    }
}

#[cfg(test)]
mod tests {
    // A panicking test is a failing test, which is the intended reporting mechanism.
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

    fn round_trip(cursor: &Cursor) {
        let encoded = OpaqueCursor(cursor.clone()).encode_cursor();
        let decoded = OpaqueCursor::decode_cursor(&encoded).expect("round trip");
        assert_eq!(&decoded.0, cursor);
    }

    #[test]
    fn round_trips_an_id_cursor() {
        round_trip(&Cursor::by_id(17));
    }

    #[test]
    fn round_trips_a_text_cursor() {
        round_trip(&Cursor::new(CursorKey::Text("Avatar".to_owned()), 407_618));
    }

    #[test]
    fn round_trips_a_numeric_cursor() {
        round_trip(&Cursor::new(CursorKey::Int(2022), 407_618));
    }

    #[test]
    fn round_trips_a_key_containing_colons() {
        // Film titles contain colons, which is why the id is encoded before the key.
        round_trip(&Cursor::new(
            CursorKey::Text("Jurassic World: Dominion".to_owned()),
            407_618,
        ));
    }

    #[test]
    fn is_opaque_rather_than_readable() {
        // A client must not be able to construct one by guessing, nor depend on its shape.
        let encoded = OpaqueCursor(Cursor::by_id(4)).encode_cursor();
        assert_ne!(encoded, "4");
        assert_eq!(encoded, "aTo0");
    }

    #[test]
    fn rejects_malformed_input() {
        assert!(matches!(
            OpaqueCursor::decode_cursor("!!!"),
            Err(CursorError::NotBase64)
        ));
        assert!(matches!(
            OpaqueCursor::decode_cursor(&STANDARD.encode("no-tag")),
            Err(CursorError::Malformed)
        ));
        assert!(matches!(
            OpaqueCursor::decode_cursor(&STANDARD.encode("i:not-a-number")),
            Err(CursorError::Malformed)
        ));
        assert!(matches!(
            OpaqueCursor::decode_cursor(&STANDARD.encode("t:4")),
            Err(CursorError::Malformed)
        ));
    }
}
