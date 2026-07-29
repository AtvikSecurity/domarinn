//! Instance-level tools: `get_server_info`.
//!
//! Everything else in the catalog answers a question about *eval data*. This
//! answers questions about the instance itself — what version it is, which
//! result-schema versions it will accept from a CLI, how it is authenticated,
//! and how the shared response cache is doing. Without it an agent has no way
//! to tell a stale server from a misconfigured one, and no way to reason about
//! why a run's cost was low (cache hits) versus high (all fresh calls).

use serde::Deserialize;
use serde_json::{json, Value};

use super::runs::{finish, internal, structured_with_budget};
use super::{parse_args, read_only_annotations, ToolResult};
use crate::mcp::text;
use crate::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum ServerInclude {
    Cache,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GetServerInfoArgs {
    pub include: Option<Vec<ServerInclude>>,
}

pub(super) fn definitions() -> Vec<Value> {
    vec![json!({
        "name": "get_server_info",
        "title": "domarinn: about this instance",
        "description": "What this domarinn instance is: version, the result-schema versions it \
            accepts from uploading clients, its authentication mode, and whether initial setup is \
            still pending. Pass include=[\"cache\"] for shared-cache health — entry count, total \
            bytes, and the configured retention limits — which is what explains why one run cost \
            far less than another. Call this when a run fails to upload, when you need to know \
            what the server supports, or when cost looks anomalous.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "include": {
                    "type": "array",
                    "items": { "type": "string", "enum": ["cache"] },
                    "description": "Extra sections to embed. Default: none."
                }
            },
            "additionalProperties": false
        },
        "annotations": read_only_annotations(),
    })]
}

pub(super) async fn get_server_info(state: &AppState, args: Value) -> ToolResult {
    let args: GetServerInfoArgs = match parse_args(args) {
        Ok(a) => a,
        Err(e) => return e,
    };

    let meta = match crate::routes::meta_view(state).await {
        Ok(meta) => meta,
        Err(e) => return internal(e, "reading server metadata"),
    };
    let mut structured = json!({ "server": meta });

    if args
        .include
        .unwrap_or_default()
        .contains(&ServerInclude::Cache)
    {
        match state.storage.cache_stats().await {
            Ok(stats) => structured["cache"] = json!(stats),
            Err(e) => return internal(e, "reading cache stats"),
        }
    }

    let budget = structured_with_budget(&mut structured);
    let text = text::json_block(&structured);
    finish(budget, structured, text)
}
