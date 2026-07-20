//! Transparent string-newtype identifiers.
//!
//! [`RunId`] (a run document's id) and [`CaseKey`] (the stable per-cell
//! identity used for diffing and as the server's per-case key) replace bare
//! `String` so the two can't be cross-assigned by accident.
//!
//! `#[serde(transparent)]` plus the single-field tuple shape make every type
//! here serialize byte-for-byte identical to a bare string on the wire —
//! ts-rs likewise emits a plain `string` alias, and serde_json accepts the
//! type directly as a `BTreeMap`/`HashMap` key (it forwards straight to the
//! inner `String`'s `Serialize`/`Deserialize` impl, bypassing the newtype
//! wrapper entirely). This is load-bearing for run-ingest content-hash
//! idempotency: do not add a hand-written `Serialize`/`Deserialize` impl, and
//! do not drop the `transparent` attribute.
//!
//! `new`/`FromStr`/`From` are infallible and never validate the id's
//! contents: existing stored/emitted ids are opaque strings supplied by
//! whatever produced them, and validating here would start rejecting
//! previously-valid data.

use std::fmt;
use std::str::FromStr;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

macro_rules! string_id {
    ($(#[$meta:meta])* pub struct $name:ident;) => {
        $(#[$meta])*
        #[derive(
            Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema, TS,
        )]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Wrap an existing id verbatim. No format validation: ids are
            /// opaque, client-supplied strings.
            pub fn new(id: impl Into<String>) -> Self {
                $name(id.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl FromStr for $name {
            type Err = std::convert::Infallible;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Ok($name::new(s))
            }
        }

        impl From<String> for $name {
            fn from(s: String) -> Self {
                $name(s)
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                $name::new(s)
            }
        }
    };
}

string_id!(
    /// A run document's id — its content-hash idempotency key
    /// (see [`crate::result::RunResult::run_id`]).
    pub struct RunId;
);

string_id!(
    /// The stable cross-run identity of one matrix cell, used for diffing and
    /// as the server's per-case key (see
    /// [`crate::result::CellKey::case_key`]).
    pub struct CaseKey;
);

impl RunId {
    /// Mint a fresh run id: a ULID, lexicographically sortable by mint time.
    pub fn generate() -> Self {
        RunId::new(ulid::Ulid::new().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn run_id_serializes_as_a_bare_string_not_an_object() {
        let id = RunId::new("abc");
        let json = serde_json::to_value(&id).unwrap();
        assert_eq!(json, serde_json::json!("abc"));
        let back: RunId = serde_json::from_value(json).unwrap();
        assert_eq!(back, id);
    }

    #[test]
    fn case_key_serializes_as_a_bare_string_not_an_object() {
        let key = CaseKey::new("deadbeef");
        let json = serde_json::to_value(&key).unwrap();
        assert_eq!(json, serde_json::json!("deadbeef"));
        let back: CaseKey = serde_json::from_value(json).unwrap();
        assert_eq!(back, key);
    }

    #[test]
    fn case_key_works_as_a_btreemap_key_and_serializes_as_a_json_object() {
        // The transparent impl must forward straight through to `String`'s
        // Serialize when used as a map key, or serde_json would reject it
        // with "key must be a string".
        let mut map: BTreeMap<CaseKey, i32> = BTreeMap::new();
        map.insert(CaseKey::new("b"), 2);
        map.insert(CaseKey::new("a"), 1);
        let json = serde_json::to_value(&map).unwrap();
        assert_eq!(json, serde_json::json!({"a": 1, "b": 2}));
    }

    #[test]
    fn display_and_from_str_round_trip() {
        let id = RunId::new("run-123");
        assert_eq!(id.to_string(), "run-123");
        let parsed: RunId = "run-123".parse().unwrap();
        assert_eq!(parsed, id);
    }

    #[test]
    fn conversions_are_infallible_and_never_validate_content() {
        // Existing stored ids may be arbitrary strings; new/FromStr/From must
        // never reject or mutate them.
        let weird = "  not/a valid::id?? \n";
        let id: RunId = weird.into();
        assert_eq!(id.as_str(), weird);
        let id2: RunId = weird.to_string().into();
        assert_eq!(id2, id);
        assert_eq!(weird.parse::<RunId>().unwrap(), id);
    }

    #[test]
    fn run_id_generate_produces_a_valid_distinct_ulid() {
        let id = RunId::generate();
        assert!(ulid::Ulid::from_string(id.as_str()).is_ok());
        assert_ne!(RunId::generate(), RunId::generate());
    }
}
