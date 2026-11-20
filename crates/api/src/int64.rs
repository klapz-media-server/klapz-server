//! Integers wider than GraphQL's `Int`.

use async_graphql::{InputValueError, InputValueResult, Scalar, ScalarType, Value};
use serde::de::Error;
use serde::{Deserialize, Deserializer};

/// A signed 64-bit integer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Int64(pub i64);

/// Reads the decimal form `Int64` is written in.
pub fn parse(text: &str) -> Option<i64> {
    text.parse().ok()
}

/// Deserializes an `Int64` from a response of the schema, for the adapters.
///
/// # Errors
///
/// When the value is not a 64-bit integer in decimal.
pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<i64, D::Error> {
    let text = String::deserialize(deserializer)?;
    parse(&text).ok_or_else(|| D::Error::custom(format!("`{text}` is not an Int64")))
}

/// Deserializes a nullable `Int64`; with `#[serde(default)]`, an absent one too.
///
/// # Errors
///
/// When the value is not a 64-bit integer in decimal.
pub fn deserialize_optional<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<i64>, D::Error> {
    Option::<String>::deserialize(deserializer)?
        .map(|text| {
            parse(&text).ok_or_else(|| D::Error::custom(format!("`{text}` is not an Int64")))
        })
        .transpose()
}

/// A signed 64-bit integer, written as a decimal string: GraphQL's `Int` stops at 32 bits, and
/// a JSON number loses precision past 2^53 in most clients. Read from such a string or from an
/// integer literal.
#[Scalar(name = "Int64")]
impl ScalarType for Int64 {
    fn parse(value: Value) -> InputValueResult<Self> {
        match &value {
            Value::String(text) => parse(text).map(Self).ok_or_else(|| {
                InputValueError::custom(format!("`{text}` is not a 64-bit integer"))
            }),
            Value::Number(number) => number.as_i64().map(Self).ok_or_else(|| {
                InputValueError::custom(format!("{number} is not a 64-bit integer"))
            }),
            _ => Err(InputValueError::expected_type(value)),
        }
    }

    fn to_value(&self) -> Value {
        Value::String(self.0.to_string())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn is_written_as_a_string_past_both_limits() {
        let past_json = Int64(8_457_612_157_739_110_435);
        assert_eq!(
            past_json.to_value(),
            Value::String("8457612157739110435".to_owned())
        );
        assert_eq!(Int64::parse(past_json.to_value()).unwrap(), past_json);
    }

    #[test]
    fn reads_an_integer_literal_and_refuses_the_rest() {
        assert_eq!(
            Int64::parse(Value::Number(5_825_269_622_i64.into())).unwrap(),
            Int64(5_825_269_622)
        );
        for refused in [
            Value::String("1.5".to_owned()),
            Value::String("9223372036854775808".to_owned()),
            Value::String(String::new()),
            Value::Number(serde_json::Number::from_f64(1.5).unwrap()),
            Value::Boolean(true),
        ] {
            assert!(Int64::parse(refused.clone()).is_err(), "{refused:?}");
        }
    }
}
