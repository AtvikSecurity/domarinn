//! `Val` — a templatable-or-raw configuration value.
//!
//! Test inputs sometimes contain literal template syntax that must never be
//! interpolated (for example an SSTI payload like `{{7*7}}`). `Val` gives every
//! templatable leaf a per-field opt-out from rendering:
//!
//! * `!raw "{{7*7}}"` — a YAML tag (preferred, YAML only).
//! * `{$raw: "{{7*7}}"}` — an object wrapper that works in any format
//!   (JSON / CSV / JSONL, which have no tags).
//! * anything else — a normal value that is rendered through the template engine.
//!
//! YAML tags only exist in YAML, so [`crate::yaml::parse_suite_value`] rewrites `!raw x` into
//! `{$raw: x}` (and `!file "p"` into `{$file: "p"}`) while it parses.
//! After that single normalization every source format flows through one code
//! path: [`Val`]'s custom `Deserialize` recognizes the `{$raw: …}` object and
//! marks the value raw, and the resolver recognizes `{$file: …}` file-content
//! references (see [`crate::filevars`]).

use serde::de::{Deserialize, Deserializer};
use serde_json::Value as Json;

/// The key that marks an object as a raw (never-rendered) value.
pub const RAW_KEY: &str = "$raw";

/// The key that marks a var as a sandboxed file-content reference, resolved at
/// suite-load time by [`crate::filevars`]. Not handled by [`Val`]'s
/// `Deserialize` (which needs no filesystem access); the marker survives as an
/// ordinary object until the resolver, which has the suite directory, loads it.
pub const FILE_KEY: &str = "$file";

/// A configuration value that is either rendered through the template engine
/// ([`Val::Tpl`]) or passed through verbatim ([`Val::Raw`]).
#[derive(Debug, Clone, PartialEq)]
pub enum Val {
    /// Rendered through the template engine at resolve time.
    Tpl(Json),
    /// Passed through untouched — never seen by the template engine.
    Raw(Json),
}

impl Val {
    /// The underlying JSON value, ignoring the render/raw distinction.
    pub fn as_json(&self) -> &Json {
        match self {
            Val::Tpl(v) | Val::Raw(v) => v,
        }
    }

    /// True when this value must never be rendered.
    pub fn is_raw(&self) -> bool {
        matches!(self, Val::Raw(_))
    }

    /// Classify a plain JSON value into a [`Val`], honoring the `{$raw: …}` form.
    pub fn classify(v: Json) -> Val {
        if let Json::Object(map) = &v {
            if map.len() == 1 {
                if let Some(inner) = map.get(RAW_KEY) {
                    return Val::Raw(inner.clone());
                }
            }
        }
        Val::Tpl(v)
    }
}

impl<'de> Deserialize<'de> for Val {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let v = Json::deserialize(deserializer)?;
        Ok(Val::classify(v))
    }
}

impl serde::Serialize for Val {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Val::Tpl(v) => v.serialize(serializer),
            Val::Raw(v) => {
                use serde::ser::SerializeMap;
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry(RAW_KEY, v)?;
                map.end()
            }
        }
    }
}

impl schemars::JsonSchema for Val {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("Val")
    }

    fn json_schema(_gen: &mut schemars::SchemaGenerator) -> schemars::Schema {
        // A `Val` may be any JSON value, or the `{$raw: <any>}` opt-out wrapper.
        // Accept anything; the distinction is semantic, not structural.
        schemars::json_schema!(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse suite-style YAML the way the loader will: desugar `!raw`, then
    /// deserialize into the target type.
    fn from_yaml<T: for<'de> Deserialize<'de>>(text: &str) -> T {
        let raw = crate::yaml::parse_suite_value(text).unwrap();
        serde_json::from_value(raw).unwrap()
    }

    #[test]
    fn plain_scalar_is_templatable() {
        let v: Val = from_yaml("\"{{ request }}\"");
        assert_eq!(v, Val::Tpl(Json::String("{{ request }}".into())));
        assert!(!v.is_raw());
    }

    #[test]
    fn yaml_raw_tag_marks_value_raw() {
        let v: Val = from_yaml("!raw \"{{7*7}}\"");
        assert_eq!(v, Val::Raw(Json::String("{{7*7}}".into())));
        assert!(v.is_raw());
    }

    #[test]
    fn yaml_file_tag_desugars_to_file_wrapper() {
        // `!file "p"` becomes `{$file: "p"}`; the resolver (not `Val`) loads it,
        // so here it stays a plain templatable object.
        let v: Val = from_yaml("!file \"data/doc.txt\"");
        assert_eq!(v, Val::Tpl(serde_json::json!({ "$file": "data/doc.txt" })));
        assert!(!v.is_raw());
    }

    #[test]
    fn object_raw_wrapper_marks_value_raw() {
        // The format-agnostic form, as it would appear in JSON/CSV/JSONL.
        let v: Val = from_yaml("{$raw: \"{{7*7}}\"}");
        assert_eq!(v, Val::Raw(Json::String("{{7*7}}".into())));
    }

    #[test]
    fn json_object_raw_wrapper_marks_value_raw() {
        let v: Val = serde_json::from_str(r#"{"$raw": "{{7*7}}"}"#).unwrap();
        assert_eq!(v, Val::Raw(Json::String("{{7*7}}".into())));
    }

    #[test]
    fn ordinary_object_is_not_raw() {
        let v: Val = serde_json::from_str(r#"{"$raw": 1, "other": 2}"#).unwrap();
        assert!(!v.is_raw(), "two-key object is not the raw wrapper");
    }

    #[test]
    fn raw_tag_inside_a_vars_map_is_isolated() {
        use std::collections::BTreeMap;
        let vars: BTreeMap<String, Val> =
            from_yaml("user_input: !raw \"{{7*7}}\"\nnormal: \"hi {{ name }}\"");
        assert!(vars["user_input"].is_raw());
        assert!(!vars["normal"].is_raw());
    }

    #[test]
    fn round_trip_raw_survives_serialization() {
        let v = Val::Raw(Json::String("{{7*7}}".into()));
        let s = serde_json::to_string(&v).unwrap();
        assert_eq!(s, r#"{"$raw":"{{7*7}}"}"#);
        let back: Val = serde_json::from_str(&s).unwrap();
        assert_eq!(back, v);
    }
}
