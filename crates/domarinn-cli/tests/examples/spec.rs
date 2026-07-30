//! What a row in [`crate::table`] is made of: the row types and the argv
//! templates the rows share.
//!
//! Split out of `table.rs` so that file holds rows and nothing else — the one
//! place a contributor edits when adding an example.

#![allow(dead_code)]

/// One shipped example and everything needed to prove it works.
pub struct Example {
    /// Directory name under `examples/`. Also the path the docs transclude.
    pub dir: &'static str,
    /// One line: what the docs use this example to show. Printed on failure, so
    /// the operator learns the *point* of the example without opening it.
    pub shows: &'static str,
    /// Environment for the child process, over and above the scrubbed baseline.
    /// The only lever that redirects a networked example at the stub.
    pub env: &'static [(&'static str, Env)],
    /// Endpoints the stub answers, keyed by a fragment of the request line.
    ///
    /// Each route scripts a *sequence* of bodies, one per matching request. A
    /// single body cannot express a case whose point is that two calls differ —
    /// `similar` embeds the output and the reference, so one repeated embedding
    /// scores 1.0 against itself and the threshold goes untested. Runs are
    /// serial by default, so the order is stable.
    pub stub: &'static [Route],
    /// Exactly how many requests the stub must serve across every step.
    ///
    /// Not decoration — this is the egress guard. If `${env:…}` ever stopped
    /// redirecting `base_url`, the request would go to the real vendor and the
    /// stub would serve zero, so a count mismatch is how "this test quietly
    /// started calling api.anthropic.com" gets caught. `0` asserts the example
    /// is genuinely offline.
    pub stub_calls: usize,
    /// The invocations, in order. More than one for an example whose point is a
    /// second run — a warm cache, a baseline diff.
    pub steps: &'static [Step],
}

/// One `domarinn` invocation and what it must produce.
pub struct Step {
    /// Everything after `domarinn`. `{dir}` is the example's absolute path and
    /// `{tmp}` the per-row scratch directory. Written out in full rather than
    /// assembled from flags, so a failure can print a command that pastes into
    /// a shell unchanged.
    pub argv: &'static [&'static str],
    pub exit: u8,
    /// The tallies this step's result document must carry. [`Cells::NONE`] for
    /// a step that writes none (`validate`, `list`).
    pub cells: Cells,
    /// Every `cell.test_id` the run must contain, order-insensitive. Empty only
    /// when `cells` is [`Cells::NONE`].
    pub case_ids: &'static [&'static str],
    /// Assert the run actually priced itself.
    ///
    /// `AssertKind::Cost` *passes* when `cost_usd` is `None` — "cost not
    /// reported; budget not enforced". So an example whose page claims a budget
    /// was enforced can be green while enforcing nothing, if the stub forgot
    /// `usage` or the model is not in the pricing table. Set this on any such
    /// example.
    pub priced: bool,
    /// Files this step must have written, non-empty, when it finishes.
    ///
    /// `{tmp}` is substituted as in `argv`. Exists for the examples whose whole
    /// subject is a side file — a JUnit report, a Markdown summary — which the
    /// cell tallies say nothing about: a run can be perfectly green while the
    /// reporter it is demonstrating writes nothing at all.
    pub writes: &'static [&'static str],
    /// The run must report at least this many cache hits.
    ///
    /// `0` for a step that should do real work. A caching example's whole claim
    /// is that its *second* run pays nothing, and without this the second step
    /// would look identical to the first whether the cache worked or not.
    pub cache_hits: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Cells {
    pub passed: u64,
    pub failed: u64,
    pub errored: u64,
    pub skipped: u64,
}

impl Cells {
    /// A step that writes no result document.
    pub const NONE: Cells = Cells {
        passed: 0,
        failed: 0,
        errored: 0,
        skipped: 0,
    };

    /// The common case: everything green.
    pub const fn pass(n: u64) -> Cells {
        Cells {
            passed: n,
            failed: 0,
            errored: 0,
            skipped: 0,
        }
    }

    pub fn total(&self) -> u64 {
        self.passed + self.failed + self.errored + self.skipped
    }
}

/// A stubbed endpoint: matched with `request_line.contains(fragment)`.
pub struct Route {
    pub fragment: &'static str,
    /// One body per matching request; the last repeats once exhausted.
    pub bodies: &'static [&'static str],
}

/// A value for the child's environment.
pub enum Env {
    Literal(&'static str),
    /// `http://127.0.0.1:<port>` — an Anthropic `base_url`, which the client
    /// suffixes with `/v1/messages`.
    StubBase,
    /// `http://127.0.0.1:<port>/v1` — an OpenAI-compatible `base_url`, which
    /// the client suffixes with `/chat/completions` (its real default already
    /// ends in `/v1`). Carried so an example's default value stays byte-honest
    /// about the endpoint shape.
    StubBaseV1,
}

/// The standard run: JSON to a scratch file, no progress bar, isolated cache.
///
/// `--cache-dir` is not optional. The cache defaults to
/// `<suite dir>/.domarinn/cache`, so without it a run writes into the
/// repository — and `.gitignore` hides it, which makes it worse: the *next*
/// run of that example is then served from a stale cache, the stub sees zero
/// requests, and the egress guard fails for a reason that looks like a broken
/// example.
pub const RUN: &[&str] = &[
    "run",
    "{dir}",
    "--format",
    "json",
    "--out",
    "{tmp}/result.json",
    "--no-progress",
    "--cache-dir",
    "{tmp}/cache",
];

/// A second run against the same cache directory — the warm half of a caching
/// example.
pub const RUN_AGAIN: &[&str] = RUN;

/// [`RUN`] with three trials per cell, for the confidence-interval example.
pub const REPEAT_3: &[&str] = &[
    "run",
    "{dir}",
    "--format",
    "json",
    "--out",
    "{tmp}/result.json",
    "--no-progress",
    "--cache-dir",
    "{tmp}/cache",
    "--repeat",
    "3",
];

/// [`RUN`] gated against the previous run in the local store.
pub const RUN_AGAINST_LATEST: &[&str] = &[
    "run",
    "{dir}",
    "--format",
    "json",
    "--out",
    "{tmp}/result.json",
    "--no-progress",
    "--cache-dir",
    "{tmp}/cache",
    "--against",
    "latest",
];

/// [`RUN`] plus the Markdown summary CI pastes into a job summary.
///
/// `--out` takes a single path, so a run emits ONE machine format to a file.
/// Producing a second one is a second invocation — which is what the row does,
/// and what the page shows.
pub const RUN_WITH_SUMMARY: &[&str] = &[
    "run",
    "{dir}",
    "--format",
    "json",
    "--out",
    "{tmp}/result.json",
    "--no-progress",
    "--cache-dir",
    "{tmp}/cache",
    "--summary-md",
    "{tmp}/summary.md",
];

/// The same suite again, emitting JUnit for a CI system to render.
pub const RUN_JUNIT: &[&str] = &[
    "run",
    "{dir}",
    "--no-progress",
    "--cache-dir",
    "{tmp}/cache",
    "--format",
    "junit",
    "--out",
    "{tmp}/results.xml",
];

/// Parse and structurally validate only. For an example that cannot run here
/// because it deliberately points at an endpoint only its reader has.
pub const VALIDATE: &[&str] = &["validate", "{dir}"];
