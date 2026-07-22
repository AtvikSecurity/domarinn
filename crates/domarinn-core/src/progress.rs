//! UI-agnostic progress events for a run, and the sink that consumes them.
//!
//! The runner emits [`ProgressEvent`]s to an optional [`ProgressSink`] so a
//! front-end (the CLI's live bar) can show progress without core taking on any
//! terminal I/O. Two deliberate design choices keep this seam clean:
//!
//! - **A synchronous trait, not a channel.** Core stays UI-agnostic and grows no
//!   async plumbing: it just calls `sink.event(&e)` inline. The sink is what
//!   decides whether that turns into a redraw, a log line, or nothing.
//! - **A parameter, not a [`RunOptions`](crate::runner::RunOptions) field.** The
//!   sink is passed to [`run_with_progress`](crate::runner::run_with_progress)
//!   as `Option<&dyn ProgressSink>`. A trait object is neither `Debug` nor
//!   `Clone`, and `RunOptions` must keep both derives, so it cannot live there.

use crate::result::{CaseStatus, CellKey, RunSummary};

/// A sink for run progress events. Called from concurrently executing cells;
/// implementations must be cheap, non-blocking, and internally synchronized.
pub trait ProgressSink: Send + Sync {
    /// Handle one progress event. Invoked inline from a running cell, so it must
    /// not block or perform slow work.
    fn event(&self, event: &ProgressEvent);
}

/// A progress event emitted over the course of a run.
///
/// Ordering guarantees: exactly one [`RunStarted`](ProgressEvent::RunStarted)
/// first, then a [`CaseStarted`](ProgressEvent::CaseStarted) /
/// [`CaseFinished`](ProgressEvent::CaseFinished) pair per cell (interleaved
/// across cells under concurrency), then exactly one
/// [`RunFinished`](ProgressEvent::RunFinished) last.
#[derive(Debug, Clone)]
pub enum ProgressEvent {
    /// The matrix has been expanded; `total` cells will be executed.
    RunStarted {
        /// The number of cells (`CaseStarted`/`CaseFinished` pairs) to expect.
        total: usize,
    },
    /// A cell has begun executing. Emitted as the first statement inside the
    /// per-cell task, so it reflects true in-flight order under
    /// `buffer_unordered` — not the deterministic output order.
    CaseStarted {
        /// The cell's index in output order (0-based).
        index: usize,
        /// The cell's identity.
        cell: CellKey,
        /// The case's display name (description, falling back to its id).
        name: Option<String>,
    },
    /// A cell has finished; carries a copy of its outcome.
    CaseFinished {
        /// The cell's index in output order (0-based).
        index: usize,
        /// The cell's identity.
        cell: CellKey,
        /// The case's display name (description, falling back to its id).
        name: Option<String>,
        /// The case's verdict.
        status: CaseStatus,
        /// The case's weighted-mean score.
        score: f64,
        /// The provider call's wall-clock latency, in milliseconds.
        latency_ms: u64,
        /// Whether the provider response was served from cache.
        cached: bool,
    },
    /// The run is complete; carries the final summary.
    RunFinished {
        /// The aggregate outcome of the run.
        summary: RunSummary,
    },
}
