//! Points in time on the official API.
//!
//! Storage keeps Unix seconds and so does `core`; the schema speaks RFC 3339, converted here
//! on the way in and out.

use async_graphql::{InputValueError, InputValueResult, Scalar, ScalarType, Value};
use chrono::{DateTime as Moment, SecondsFormat};

/// A point in time, held as seconds since the Unix epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DateTime(i64);

impl DateTime {
    pub const fn from_unix_seconds(seconds: i64) -> Self {
        Self(seconds)
    }

    pub const fn unix_seconds(self) -> i64 {
        self.0
    }
}

impl From<i64> for DateTime {
    fn from(seconds: i64) -> Self {
        Self(seconds)
    }
}

/// The first and last second RFC 3339 can write: its years have four digits.
const EARLIEST: i64 = -62_167_219_200;
const LATEST: i64 = 253_402_300_799;

/// Writes `seconds` as RFC 3339 in UTC, `2024-05-01T12:34:56Z`. A time outside years 0000 to
/// 9999 is written as the nearest one RFC 3339 can express.
pub fn format(seconds: i64) -> String {
    Moment::from_timestamp(seconds.clamp(EARLIEST, LATEST), 0)
        .unwrap_or_default()
        .to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Reads an RFC 3339 date-time with any offset, down to the second: a fraction is dropped,
/// toward the past.
pub fn parse(text: &str) -> Option<i64> {
    Moment::parse_from_rfc3339(text)
        .ok()
        .map(|moment| moment.timestamp())
}

/// A date-time as RFC 3339 defines it, with an offset. Written in UTC (`Z`) to the second;
/// read with any offset, a fraction of a second dropped.
#[Scalar(
    name = "DateTime",
    specified_by_url = "https://scalars.graphql.org/andimarek/date-time"
)]
impl ScalarType for DateTime {
    fn parse(value: Value) -> InputValueResult<Self> {
        let Value::String(text) = &value else {
            return Err(InputValueError::expected_type(value));
        };
        parse(text).map(Self).ok_or_else(|| {
            InputValueError::custom(format!("`{text}` is not an RFC 3339 date-time"))
        })
    }

    fn to_value(&self) -> Value {
        Value::String(format(self.0))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn writes_utc_to_the_second_and_reads_it_back() {
        let written = DateTime(1_714_566_896).to_value();
        assert_eq!(written, Value::String("2024-05-01T12:34:56Z".to_owned()));
        assert_eq!(DateTime::parse(written).unwrap(), DateTime(1_714_566_896));
        assert_eq!(format(0), "1970-01-01T00:00:00Z");
        assert_eq!(parse("1969-12-31T23:59:59Z"), Some(-1));
    }

    #[test]
    fn another_offset_is_read_as_the_same_instant() {
        let read = |text: &str| DateTime::parse(Value::String(text.to_owned())).unwrap();
        assert_eq!(read("2024-05-01T14:34:56+02:00"), DateTime(1_714_566_896));
        assert_eq!(read("2024-05-01T07:04:56-05:30"), DateTime(1_714_566_896));
        assert_eq!(
            read("2024-05-01T14:34:56+02:00").to_value(),
            Value::String("2024-05-01T12:34:56Z".to_owned())
        );
    }

    #[test]
    fn a_fraction_of_a_second_is_dropped_toward_the_past() {
        assert_eq!(parse("2024-05-01T12:34:56.999Z"), Some(1_714_566_896));
        assert_eq!(
            parse("2024-05-01T12:34:56.123456789+00:00"),
            Some(1_714_566_896)
        );
        assert_eq!(parse("1969-12-31T23:59:59.5Z"), Some(-1));
    }

    #[test]
    fn refuses_what_is_not_an_rfc_3339_date_time() {
        for text in [
            "",
            "garbage",
            "2024-05-01",
            "2024-05-01T12:34:56",
            "2024-05-01T12:34Z",
            "2024-13-01T12:34:56Z",
            "2024-02-30T12:34:56Z",
            "1714566896",
            "2024-05-01T12:34:56+2:00",
        ] {
            assert!(
                DateTime::parse(Value::String(text.to_owned())).is_err(),
                "{text:?}"
            );
        }
        assert!(DateTime::parse(Value::Number(1_714_566_896.into())).is_err());
        assert!(DateTime::parse(Value::Null).is_err());
    }

    #[test]
    fn a_time_rfc_3339_cannot_write_is_written_as_the_nearest_it_can() {
        assert_eq!(format(i64::MIN), "0000-01-01T00:00:00Z");
        assert_eq!(format(i64::MAX), "9999-12-31T23:59:59Z");
    }
}
