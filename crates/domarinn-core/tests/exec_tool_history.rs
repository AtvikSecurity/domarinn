//! What an `exec` child actually receives when a case replays a tool-using
//! transcript.
//!
//! `exec_provider::rendered_prompt_json` writes `{"messages": msgs}` straight
//! off `ChatMessage`'s serde derive, so the tool fields reach a child with no
//! code of their own — which is convenient and, precisely because it is
//! automatic, untested by anything else. These two tests pin both halves of the
//! contract `docs/reference/protocol.md` now promises exec-provider authors:
//!
//! - a tool-bearing turn arrives **as structure**, not as prose;
//! - a tool-free turn is byte-identical to what it always was, so an existing
//!   provider is unaffected and its warm cache entries still key the same.
//!
//! It lives here rather than beside the code because `exec_provider.rs` is at
//! 994 of the repo's 1000-line source cap.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use async_trait::async_trait;
use domarinn_core::cache::{
    CacheBackend, CacheEntry, CacheError, CacheKey, CacheStats, PurgeFilter,
};
use domarinn_core::runner::{run, RunOptions};
use domarinn_core::types::Output;
use serde_json::{json, Value as Json};

/// A minimal in-memory cache, as every other integration test here defines.
#[derive(Default)]
struct MemCache {
    map: Mutex<HashMap<String, CacheEntry>>,
}

#[async_trait]
impl CacheBackend for MemCache {
    async fn get(&self, key: &CacheKey) -> Result<Option<CacheEntry>, CacheError> {
        Ok(self.map.lock().unwrap().get(&key.0).cloned())
    }
    async fn put(&self, key: &CacheKey, entry: &CacheEntry) -> Result<(), CacheError> {
        self.map
            .lock()
            .unwrap()
            .entry(key.0.clone())
            .or_insert_with(|| entry.clone());
        Ok(())
    }
    async fn stats(&self) -> Result<CacheStats, CacheError> {
        Ok(CacheStats::default())
    }
    async fn purge(&self, _filter: &PurgeFilter) -> Result<u64, CacheError> {
        Ok(0)
    }
}

/// A suite whose provider echoes the request document it was handed, so the
/// assertions can read the exact bytes that crossed the wire.
fn echo_suite(history: &str) -> String {
    format!(
        r#"
version: 1
project: exec-tool-history
suite: wire
providers:
  - id: echo
    type: exec
    command: ["python3", "-c", "import json,sys; r=json.load(sys.stdin); json.dump({{'output': r}}, sys.stdout)"]
tests:
  - id: t
{history}
"#
    )
}

fn captured_prompt(history: &str) -> Json {
    let suite = domarinn_core::loader::load_str(&echo_suite(history)).expect("the suite loads");
    let cache = MemCache::default();
    let result = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(run(
            &suite,
            Path::new("."),
            &cache,
            None,
            &RunOptions::default(),
        ))
        .expect("the suite runs");
    assert_eq!(result.cases.len(), 1, "one cell");
    let echoed = match result.cases[0]
        .output
        .as_ref()
        .expect("the echo provider returned the request")
    {
        Output::Json(v) => v.clone(),
        Output::Text(t) => serde_json::from_str(t).expect("the echoed request is JSON"),
    };
    echoed["prompt"].clone()
}

/// Structure in, structure out: the child sees a real call and a real result,
/// never a paraphrase of one.
#[test]
fn a_tool_bearing_transcript_reaches_the_child_as_structure() {
    let prompt = captured_prompt(
        r#"    history:
      - {role: user, content: "where is 1042?"}
      - role: assistant
        tool_calls:
          - {id: call_1, name: lookup_order, arguments: {order_id: 1042}}
      - {role: tool, tool_call_id: call_1, content: '{"status":"shipped"}'}"#,
    );

    let messages = prompt["messages"].as_array().expect("a messages prompt");
    assert_eq!(messages.len(), 3);

    // The call, as an object rather than a sentence about one.
    assert_eq!(messages[1]["role"], "assistant");
    assert_eq!(messages[1]["tool_calls"][0]["name"], "lookup_order");
    assert_eq!(
        messages[1]["tool_calls"][0]["arguments"],
        json!({"order_id": 1042}),
        "arguments stay a decoded object, and 1042 stays an integer"
    );

    // The result, attributed to the call it answers.
    assert_eq!(messages[2]["role"], "tool");
    assert_eq!(messages[2]["tool_call_id"], "call_1");
}

/// The cache tripwire at the wire layer: a transcript that uses neither tool
/// turns nor content blocks must serialize to exactly the keys it always did,
/// or every warm `exec` entry re-keys on upgrade.
#[test]
fn a_tool_free_transcript_is_byte_identical_to_before() {
    let prompt = captured_prompt(
        r#"    history:
      - {role: user, content: "hi"}
      - {role: assistant, content: "hello"}"#,
    );

    assert_eq!(
        prompt,
        json!({"messages": [
            {"role": "user", "content": "hi"},
            {"role": "assistant", "content": "hello"},
        ]}),
        "a tool-free turn must carry no extra keys"
    );
}
