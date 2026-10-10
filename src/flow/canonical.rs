//! Canonical JSON and digests.
//!
//! This module owns the RFC 8785 serialization of digested flow data and the
//! `sha256:` digest form. Serializable values are the input. Canonical bytes and
//! digests are the output.
//!
//! Digested flow layers contain integers, strings, booleans, and null only. The
//! serializer rejects every other number, so a float can never enter a digest.

use std::{fmt, io::Write as _};

use serde::Serialize;
use serde_json::Value;

use super::models::Sha256Digest;

/// Serializes one value as RFC 8785 canonical JSON.
///
/// Object keys sort by UTF-16 code units. Strings use the JSON escapes of
/// ECMAScript `JSON.stringify`.
///
/// # Errors
/// Returns an error when the value holds a non-integer number or cannot serialize.
fn canonical_bytes(value: &impl Serialize) -> Result<Vec<u8>, CanonicalError> {
    let value = serde_json::to_value(value).map_err(CanonicalError::Json)?;
    let mut output = Vec::new();
    write_value(&value, &mut output)?;
    Ok(output)
}

/// Returns the SHA-256 digest of the canonical form of one value.
///
/// # Errors
/// Returns an error when the value cannot have a canonical form.
pub fn digest(value: &impl Serialize) -> Result<Sha256Digest, CanonicalError> {
    Ok(digest_bytes(&canonical_bytes(value)?))
}

/// Returns the SHA-256 digest of raw bytes.
#[must_use]
pub fn digest_bytes(bytes: &[u8]) -> Sha256Digest {
    Sha256Digest::of_bytes(bytes)
}

/// Returns the lowercase hex SHA-256 of the canonical form of one value.
///
/// # Errors
/// Returns an error when the value cannot have a canonical form.
pub fn hex_digest(value: &impl Serialize) -> Result<String, CanonicalError> {
    Ok(digest(value)?.hex().to_owned())
}

fn write_value(value: &Value, output: &mut Vec<u8>) -> Result<(), CanonicalError> {
    match value {
        // Write the literal null.
        Value::Null => output.extend_from_slice(b"null"),
        // Write a boolean literal.
        Value::Bool(flag) => output.extend_from_slice(if *flag { b"true" } else { b"false" }),
        // Write an integer; reject every other number.
        Value::Number(number) => {
            if let Some(integer) = number.as_i64() {
                write!(output, "{integer}").map_err(CanonicalError::Io)?;
            } else if let Some(integer) = number.as_u64() {
                write!(output, "{integer}").map_err(CanonicalError::Io)?;
            } else {
                return Err(CanonicalError::Float);
            }
        }
        // Write an escaped string.
        Value::String(text) => write_string(text, output)?,
        // Write items in their order.
        Value::Array(items) => {
            output.push(b'[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    output.push(b',');
                }
                write_value(item, output)?;
            }
            output.push(b']');
        }
        // Write members in UTF-16 key order.
        Value::Object(map) => {
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_by(|(left, _), (right, _)| left.encode_utf16().cmp(right.encode_utf16()));
            output.push(b'{');
            for (index, (key, item)) in entries.into_iter().enumerate() {
                if index > 0 {
                    output.push(b',');
                }
                write_string(key, output)?;
                output.push(b':');
                write_value(item, output)?;
            }
            output.push(b'}');
        }
    }
    Ok(())
}

fn write_string(text: &str, output: &mut Vec<u8>) -> Result<(), CanonicalError> {
    // serde_json uses the same escapes as JSON.stringify: short forms, then lowercase \u00XX.
    serde_json::to_writer(output, text).map_err(CanonicalError::Json)
}

// -----------------------------------------------------------------------------
// Errors
// -----------------------------------------------------------------------------

#[derive(Debug)]
pub enum CanonicalError {
    Float,
    Json(serde_json::Error),
    Io(std::io::Error),
}

impl fmt::Display for CanonicalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Float => write!(formatter, "digested flow data must not contain a float"),
            Self::Json(error) => write!(formatter, "cannot serialize digested flow data: {error}"),
            Self::Io(error) => write!(formatter, "cannot write canonical JSON: {error}"),
        }
    }
}

impl std::error::Error for CanonicalError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Float => None,
            Self::Json(error) => Some(error),
            Self::Io(error) => Some(error),
        }
    }
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorts_keys_by_utf16_units_and_removes_whitespace() {
        let value = serde_json::json!({"b": [1, true, null], "a": {"é": "x", "z": -2}, "\u{e000}": 0, "\u{1f600}": 1});
        let bytes = canonical_bytes(&value).expect("canonical");
        // U+1F600 encodes as a surrogate pair (0xD83D) and sorts before U+E000.
        assert_eq!(
            String::from_utf8(bytes).expect("UTF-8"),
            "{\"a\":{\"z\":-2,\"é\":\"x\"},\"b\":[1,true,null],\"\u{1f600}\":1,\"\u{e000}\":0}"
        );
    }

    #[test]
    fn escapes_control_characters_like_json_stringify() {
        let bytes = canonical_bytes(&"a\"\\\n\t\u{1}\u{7f}/\u{2028}").expect("canonical");
        assert_eq!(
            String::from_utf8(bytes).expect("UTF-8"),
            "\"a\\\"\\\\\\n\\t\\u0001\u{7f}/\u{2028}\""
        );
    }

    #[test]
    fn rejects_floats() {
        assert!(matches!(
            canonical_bytes(&serde_json::json!({"x": 1.5})),
            Err(CanonicalError::Float)
        ));
    }

    #[test]
    fn digests_match_the_documented_step_id_example() {
        let hex = hex_digest(&("step", "flow-generation", "src/main.rs::callable:main", 0))
            .expect("digest");
        assert_eq!(&hex[..16], "67244b2b65163f51");
    }
}
