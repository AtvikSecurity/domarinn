//! YAML parsing, backed by the pure-Rust `serde-saphyr`.
//!
//! `serde-saphyr` has no YAML `Value` type, and its untyped visitors silently
//! discard YAML tags. Both matter here: suites spell `!raw` / `!file` as tags,
//! and dropping `!raw` would quietly turn a "do not template this" value into a
//! templated one. So every document is parsed through [`Node`], which captures
//! each node's tag with [`serde_saphyr::Tagged`] and builds a
//! [`serde_json::Value`] tree from it:
//!
//! * [`parse_suite_value`] rewrites the sugar tags into their format-agnostic
//!   wrapper objects (`!raw v` → `{$raw: v}`, `!file "p"` → `{$file: "p"}`).
//! * [`from_str`] / [`parse_value`] accept no application tags at all.
//!
//! In both modes the YAML core tags (`!!str`, `!!int`, ...) and the
//! non-specific `!` tag keep their normal meaning, and any other tag is a parse
//! error rather than something that is silently ignored.

use std::fmt;
use std::marker::PhantomData;

use serde::de::{self, DeserializeOwned, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::Deserialize;
use serde_json::{Map, Value as Json};
use serde_saphyr::Tagged;

use crate::val::{FILE_KEY, RAW_KEY};

/// A parsed YAML document. Mappings keep their source key order (the workspace
/// enables `serde_json/preserve_order`).
pub type Yaml = Json;

/// The error every function in this module returns.
pub type Error = serde_saphyr::Error;

/// Parse a suite-shaped document (a suite file or a `file://` test list),
/// desugaring the `!raw` / `!file` tags into their wrapper objects.
pub fn parse_suite_value(text: &str) -> Result<Yaml, Error> {
    serde_saphyr::from_str::<Node<true>>(text).map(|n| n.0)
}

/// Parse a YAML document into a JSON value. Application tags (including
/// `!raw` / `!file`) are rejected.
pub fn parse_value(text: &str) -> Result<Yaml, Error> {
    serde_saphyr::from_str::<Node<false>>(text).map(|n| n.0)
}

/// Parse a YAML document into `T`. Application tags are rejected, so a tag can
/// never be dropped on the way into a typed value.
pub fn from_str<T: DeserializeOwned>(text: &str) -> Result<T, Error> {
    let value = parse_value(text)?;
    serde_json::from_value(value).map_err(|e| <Error as de::Error>::custom(e.to_string()))
}

/// What a node's tag means for the value under it.
enum TagAction {
    /// No tag, a YAML core tag, or the non-specific `!`: keep the value as is.
    Keep,
    /// A sugar tag: wrap the value in `{<key>: value}`.
    Wrap(&'static str),
}

fn classify<E: de::Error>(tag: Option<&str>, desugar: bool) -> Result<TagAction, E> {
    let Some(tag) = tag else {
        return Ok(TagAction::Keep);
    };
    if tag == "!" || tag.starts_with("tag:yaml.org,2002:") {
        return Ok(TagAction::Keep);
    }
    if desugar {
        match tag {
            "!raw" | "raw" => return Ok(TagAction::Wrap(RAW_KEY)),
            "!file" | "file" => return Ok(TagAction::Wrap(FILE_KEY)),
            _ => {}
        }
    }
    let hint = if desugar {
        " (only `!raw` and `!file` are supported)"
    } else {
        ""
    };
    Err(E::custom(format!("unsupported YAML tag `{tag}`{hint}")))
}

/// One YAML node, tag-checked, as JSON. `DESUGAR` selects between
/// [`parse_suite_value`] and [`parse_value`] semantics.
struct Node<const DESUGAR: bool>(Json);

impl<'de, const DESUGAR: bool> Deserialize<'de> for Node<DESUGAR> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let Tagged(Untagged(value), tag) = Tagged::<Untagged<DESUGAR>>::deserialize(d)?;
        Ok(Node(match classify::<D::Error>(tag.as_deref(), DESUGAR)? {
            TagAction::Keep => value,
            TagAction::Wrap(key) => {
                let mut map = Map::new();
                map.insert(key.to_string(), value);
                Json::Object(map)
            }
        }))
    }
}

/// A mapping key: must be a scalar, and its tag is checked like any other.
struct Key<const DESUGAR: bool>(String);

impl<'de, const DESUGAR: bool> Deserialize<'de> for Key<DESUGAR> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let Tagged(key, tag) = Tagged::<String>::deserialize(d)?;
        match classify::<D::Error>(tag.as_deref(), false)? {
            TagAction::Keep => Ok(Key(key)),
            TagAction::Wrap(_) => unreachable!("keys are classified without desugaring"),
        }
    }
}

/// A node's value with its own tag already handled; children are [`Node`]s.
struct Untagged<const DESUGAR: bool>(Json);

impl<'de, const DESUGAR: bool> Deserialize<'de> for Untagged<DESUGAR> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(UntaggedVisitor::<DESUGAR>(PhantomData))
            .map(Untagged)
    }
}

struct UntaggedVisitor<const DESUGAR: bool>(PhantomData<()>);

impl<'de, const DESUGAR: bool> Visitor<'de> for UntaggedVisitor<DESUGAR> {
    type Value = Json;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("any YAML value")
    }

    fn visit_bool<E>(self, v: bool) -> Result<Json, E> {
        Ok(Json::Bool(v))
    }

    fn visit_i64<E>(self, v: i64) -> Result<Json, E> {
        Ok(Json::from(v))
    }

    fn visit_u64<E>(self, v: u64) -> Result<Json, E> {
        Ok(Json::from(v))
    }

    fn visit_i128<E: de::Error>(self, v: i128) -> Result<Json, E> {
        i64::try_from(v)
            .map(Json::from)
            .or_else(|_| u64::try_from(v).map(Json::from))
            .map_err(|_| E::custom(format!("integer {v} is out of range")))
    }

    fn visit_u128<E: de::Error>(self, v: u128) -> Result<Json, E> {
        u64::try_from(v)
            .map(Json::from)
            .map_err(|_| E::custom(format!("integer {v} is out of range")))
    }

    fn visit_f64<E>(self, v: f64) -> Result<Json, E> {
        // Like `serde_json::Value`, a non-finite float has no JSON form: null.
        Ok(serde_json::Number::from_f64(v).map_or(Json::Null, Json::Number))
    }

    fn visit_str<E>(self, v: &str) -> Result<Json, E> {
        Ok(Json::String(v.to_owned()))
    }

    fn visit_string<E>(self, v: String) -> Result<Json, E> {
        Ok(Json::String(v))
    }

    fn visit_bytes<E: de::Error>(self, v: &[u8]) -> Result<Json, E> {
        String::from_utf8(v.to_vec())
            .map(Json::String)
            .map_err(|_| E::custom("binary YAML value is not valid UTF-8"))
    }

    fn visit_none<E>(self) -> Result<Json, E> {
        Ok(Json::Null)
    }

    fn visit_unit<E>(self) -> Result<Json, E> {
        Ok(Json::Null)
    }

    fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Json, D::Error> {
        Untagged::<DESUGAR>::deserialize(d).map(|u| u.0)
    }

    fn visit_newtype_struct<D: Deserializer<'de>>(self, d: D) -> Result<Json, D::Error> {
        Untagged::<DESUGAR>::deserialize(d).map(|u| u.0)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Json, A::Error> {
        let mut out = Vec::with_capacity(seq.size_hint().unwrap_or(0));
        while let Some(Node::<DESUGAR>(item)) = seq.next_element()? {
            out.push(item);
        }
        Ok(Json::Array(out))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Json, A::Error> {
        let mut out = Map::new();
        while let Some(Key::<DESUGAR>(key)) = map.next_key()? {
            let Node::<DESUGAR>(value) = map.next_value()?;
            out.insert(key, value);
        }
        Ok(Json::Object(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sugar_tags_become_wrapper_objects_at_any_depth() {
        let v = parse_suite_value(
            "a: !raw \"{{7*7}}\"\nb: [!file data.txt, plain]\nc: {d: !raw {x: 1}}\n",
        )
        .unwrap();
        assert_eq!(
            v,
            json!({
                "a": {"$raw": "{{7*7}}"},
                "b": [{"$file": "data.txt"}, "plain"],
                "c": {"d": {"$raw": {"x": 1}}},
            })
        );
    }

    #[test]
    fn unknown_tags_are_errors_not_silently_dropped() {
        let err = parse_suite_value("a: !rwa \"{{7*7}}\"\n").unwrap_err();
        assert!(
            err.to_string().contains("unsupported YAML tag `!rwa`"),
            "{err}"
        );
        // Outside suite parsing even the sugar tags are rejected.
        let err = parse_value("a: !raw x\n").unwrap_err();
        assert!(
            err.to_string().contains("unsupported YAML tag `!raw`"),
            "{err}"
        );
        assert!(parse_value("!weird {}").is_err());
    }

    #[test]
    fn core_tags_and_plain_scalars_keep_their_meaning() {
        let v = parse_value("s: !!str 123\nn: 7\nf: 1.5\nb: true\nz: ~\nt: ! 5\n").unwrap();
        assert_eq!(
            v,
            json!({"s": "123", "n": 7, "f": 1.5, "b": true, "z": null, "t": "5"})
        );
    }

    #[test]
    fn mapping_order_is_preserved_and_scalar_keys_become_strings() {
        let v = parse_value("z: 1\na: 2\n3: three\n").unwrap();
        let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys, ["z", "a", "3"]);
    }

    #[test]
    fn from_str_reaches_typed_values() {
        #[derive(Deserialize, Debug, PartialEq)]
        struct S {
            name: String,
            n: u32,
        }
        let s: S = from_str("name: x\nn: 3\n").unwrap();
        assert_eq!(
            s,
            S {
                name: "x".into(),
                n: 3
            }
        );
        assert!(from_str::<S>("name: x\nn: [1]\n").is_err());
    }
}
