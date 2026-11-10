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

    /// `i:<id>` for an id-ordered collection, else `k:<id>` and one segment per key:
    /// `;t<length>:<text>`, `;n<integer>`, `;r<real>` or `;z` for an absent value. Text is
    /// length-prefixed because a title may hold any character, the separators included.
    fn encode_cursor(&self) -> String {
        let Cursor { keys, id } = &self.0;
        if keys.is_empty() {
            return STANDARD.encode(format!("i:{id}"));
        }
        let segments: String = keys
            .iter()
            .map(|key| match key {
                CursorKey::Text(text) => format!(";t{}:{text}", text.len()),
                CursorKey::Int(value) => format!(";n{value}"),
                // `f64`'s display is the shortest form that parses back to the same value.
                CursorKey::Real(value) => format!(";r{value}"),
                CursorKey::Absent => ";z".to_owned(),
            })
            .collect();
        STANDARD.encode(format!("k:{id}{segments}"))
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
            "k" => {
                let (id, mut segments) = rest.split_once(';').ok_or(CursorError::Malformed)?;
                let id = id.parse().map_err(|_| CursorError::Malformed)?;
                let mut keys = Vec::new();
                loop {
                    let (key, remainder) = next_key(segments)?;
                    keys.push(key);
                    match remainder.strip_prefix(';') {
                        Some(more) => segments = more,
                        None if remainder.is_empty() => break,
                        None => return Err(CursorError::Malformed),
                    }
                }
                Ok(Self(Cursor { keys, id }))
            }
            _ => Err(CursorError::Malformed),
        }
    }
}

/// Reads one key segment, its `;` already consumed, and returns what follows it.
fn next_key(segment: &str) -> Result<(CursorKey, &str), CursorError> {
    let mut chars = segment.chars();
    let tag = chars.next().ok_or(CursorError::Malformed)?;
    let body = chars.as_str();
    let number_end = body.find(';').unwrap_or(body.len());
    match tag {
        't' => {
            let (length, text) = body.split_once(':').ok_or(CursorError::Malformed)?;
            let length: usize = length.parse().map_err(|_| CursorError::Malformed)?;
            let value = text.get(..length).ok_or(CursorError::Malformed)?;
            let remainder = text.get(length..).ok_or(CursorError::Malformed)?;
            Ok((CursorKey::Text(value.to_owned()), remainder))
        }
        'n' => Ok((
            CursorKey::Int(
                body[..number_end]
                    .parse()
                    .map_err(|_| CursorError::Malformed)?,
            ),
            &body[number_end..],
        )),
        'r' => Ok((
            CursorKey::Real(
                body[..number_end]
                    .parse()
                    .map_err(|_| CursorError::Malformed)?,
            ),
            &body[number_end..],
        )),
        'z' => Ok((CursorKey::Absent, body)),
        _ => Err(CursorError::Malformed),
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
    fn round_trips_a_key_containing_the_separators() {
        // Film titles contain colons and may contain anything, which is why text is
        // length-prefixed.
        round_trip(&Cursor::new(
            CursorKey::Text("Jurassic World: Dominion; ;z".to_owned()),
            407_618,
        ));
        round_trip(&Cursor::new(CursorKey::Text("Amélie;".to_owned()), 1));
    }

    #[test]
    fn round_trips_several_keys_an_absent_one_included() {
        round_trip(&Cursor {
            keys: vec![
                CursorKey::Real(7.3),
                CursorKey::Absent,
                CursorKey::Text("alien".to_owned()),
            ],
            id: 407_618,
        });
        round_trip(&Cursor {
            keys: vec![CursorKey::Absent],
            id: 3,
        });
        // A real rating survives at full precision.
        round_trip(&Cursor::new(CursorKey::Real(0.1 + 0.2), 3));
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
        for plain in ["k:4", "k:4;", "k:4;t9:short", "k:4;x1", "k:4;n1x", "k:4;zz"] {
            assert!(
                matches!(
                    OpaqueCursor::decode_cursor(&STANDARD.encode(plain)),
                    Err(CursorError::Malformed)
                ),
                "{plain}"
            );
        }
    }
}
