//! Flow identifiers and digests.
//!
//! This module owns the validated string forms that flow artifacts use as keys.
//! Each type checks its pattern on construction and on deserialization.

use std::{borrow::Borrow, fmt};

use serde::{Deserialize, Serialize};

macro_rules! identifier {
    ($(#[$doc:meta])* $name:ident, $check:path, $what:literal) => {
        $(#[$doc])*
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = InvalidIdentifier;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                if $check(&value) {
                    Ok(Self(value))
                } else {
                    Err(InvalidIdentifier { kind: $what, value })
                }
            }
        }

        impl TryFrom<&str> for $name {
            type Error = InvalidIdentifier;

            fn try_from(value: &str) -> Result<Self, Self::Error> {
                Self::try_from(value.to_owned())
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl Borrow<str> for $name {
            fn borrow(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }
    };
}

identifier!(
    /// The flow slug. It equals the store folder name and never changes.
    FlowId,
    is_flow_id,
    "flow ID"
);
identifier!(
    /// The agent handle of one flow cluster.
    ClusterKey,
    is_cluster_key,
    "cluster key"
);
identifier!(
    /// A derived `st-` step ID.
    StepId,
    is_step_id,
    "step ID"
);
identifier!(
    /// The indexer entity key of one graph node.
    NodeKey,
    is_node_key,
    "node key"
);
identifier!(
    /// A `sha256:` digest with 64 lowercase hex digits.
    Sha256Digest,
    is_sha256_digest,
    "SHA-256 digest"
);
identifier!(
    /// The indexer FNV-1a hash of one declaration-to-body span.
    ContentHash,
    is_content_hash,
    "content hash"
);

impl Sha256Digest {
    /// Returns the digest of raw bytes in the store digest form.
    #[must_use]
    pub fn of_bytes(bytes: &[u8]) -> Self {
        Self(crate::store::digest_of(bytes))
    }

    /// Returns the 64 hex digits without the prefix.
    #[must_use]
    pub fn hex(&self) -> &str {
        &self.0["sha256:".len()..]
    }
}

fn lower_hex(text: &str) -> bool {
    text.bytes()
        .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn lower_alnum(byte: u8) -> bool {
    byte.is_ascii_lowercase() || byte.is_ascii_digit()
}

/// Checks `^[a-z0-9](?:[a-z0-9-]{0,62}[a-z0-9])?$`.
#[must_use]
pub fn is_flow_id(text: &str) -> bool {
    let bytes = text.as_bytes();
    (1..=64).contains(&bytes.len())
        && bytes.first().copied().is_some_and(lower_alnum)
        && bytes.last().copied().is_some_and(lower_alnum)
        && bytes.iter().all(|byte| lower_alnum(*byte) || *byte == b'-')
}

/// Checks `^[a-z][a-z0-9]*(?:-[a-z0-9]+)*$` with 2 to 48 characters.
#[must_use]
pub fn is_cluster_key(text: &str) -> bool {
    let bytes = text.as_bytes();
    (2..=48).contains(&bytes.len())
        && bytes.first().is_some_and(u8::is_ascii_lowercase)
        && text
            .split('-')
            .all(|part| !part.is_empty() && part.bytes().all(lower_alnum))
}

/// Checks the derived step ID form `st-` followed by 16 lowercase hex digits.
#[must_use]
pub fn is_step_id(text: &str) -> bool {
    text.strip_prefix("st-")
        .is_some_and(|hex| hex.len() == 16 && lower_hex(hex))
}

/// Checks the node key text rule: no control characters and no outer whitespace.
#[must_use]
pub fn is_node_key(text: &str) -> bool {
    let count = text.chars().count();
    (1..=1024).contains(&count)
        && !text.chars().any(crate::flow::text::is_control)
        && !text.starts_with(crate::flow::text::is_space)
        && !text.ends_with(crate::flow::text::is_space)
}

fn is_sha256_digest(text: &str) -> bool {
    text.strip_prefix("sha256:")
        .is_some_and(|hex| hex.len() == 64 && lower_hex(hex))
}

fn is_content_hash(text: &str) -> bool {
    text.len() == 16 && lower_hex(text)
}

// -----------------------------------------------------------------------------
// Errors
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidIdentifier {
    pub kind: &'static str,
    pub value: String,
}

impl fmt::Display for InvalidIdentifier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid {}: {:?}", self.kind, self.value)
    }
}

impl std::error::Error for InvalidIdentifier {}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_and_rejects_identifier_patterns() {
        assert!(
            is_flow_id("flow-generation")
                && is_flow_id("a")
                && !is_flow_id("-a")
                && !is_flow_id("A")
        );
        assert!(
            is_cluster_key("load-verify")
                && !is_cluster_key("l")
                && !is_cluster_key("9a")
                && !is_cluster_key("a--b")
        );
        assert!(is_step_id("st-67244b2b65163f51") && !is_step_id("s-18d4a3aa4a9977c8-8512-0"));
        assert!(!is_step_id("step-1") && !is_step_id("st-67244B2B65163F51"));
        assert!(
            is_node_key("src/main.rs::callable:main") && !is_node_key(" a") && !is_node_key("a\tb")
        );
        assert!(Sha256Digest::try_from(format!("sha256:{}", "a".repeat(64))).is_ok());
        assert!(Sha256Digest::try_from("sha256:ABC").is_err());
    }
}
