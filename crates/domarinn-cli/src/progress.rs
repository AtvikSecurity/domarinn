//! The CLI's live progress bar: an [`indicatif`] consumer of core
//! [`ProgressEvent`]s.
//!
//! Everything here draws on **stderr** so stdout stays byte-pure for machine
//! formats, and the bar hides itself entirely when stderr is not a terminal.
//! Enablement (TTY, `--no-progress`, verbosity) is decided by the caller in
//! `run::execute`; this type only knows how to render events it is given.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use domarinn_core::progress::{ProgressEvent, ProgressSink};
use domarinn_core::result::CaseStatus;
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};

use crate::style::Palette;

/// Truncate a case name to at most `max` characters, appending an ellipsis when
/// it was cut. Character-based (not byte-based) so multi-byte names never split
/// mid-codepoint.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{head}…")
}

/// A single indicatif progress bar driven by run progress events.
///
/// Tallies are atomics because [`ProgressSink::event`] is called concurrently
/// from in-flight cells under `buffer_unordered`; the bar itself is internally
/// synchronized by indicatif.
pub struct RunProgressBar {
    bar: ProgressBar,
    palette: Palette,
    passed: AtomicU64,
    failed: AtomicU64,
    errored: AtomicU64,
    xpassed: AtomicU64,
}

impl RunProgressBar {
    /// Build a stderr-targeted bar with a steady 100ms tick (so `{elapsed}` keeps
    /// advancing even while a single slow provider call is in flight).
    pub fn new(palette: Palette) -> Self {
        let bar = ProgressBar::new(0);
        bar.set_draw_target(ProgressDrawTarget::stderr());
        bar.set_style(
            ProgressStyle::with_template("{elapsed_precise} {bar:30} {pos}/{len} {msg}")
                .expect("static progress template is valid")
                .progress_chars("##-"),
        );
        bar.enable_steady_tick(Duration::from_millis(100));
        RunProgressBar {
            bar,
            palette,
            passed: AtomicU64::new(0),
            failed: AtomicU64::new(0),
            errored: AtomicU64::new(0),
            xpassed: AtomicU64::new(0),
        }
    }

    /// Clear the bar from the terminal. Idempotent, so the caller can invoke it
    /// unconditionally after the run returns without racing the `RunFinished`
    /// handler — neither an early error nor a normal finish leaves a stuck bar.
    pub fn finish(&self) {
        self.bar.finish_and_clear();
    }

    /// Rebuild the `{msg}` segment from the current tallies plus the case name to
    /// surface as "running". The fail tally is reddened only when the palette is
    /// enabled (a disabled palette is a verbatim passthrough).
    fn render_msg(&self, running: &str) -> String {
        let passed = self.passed.load(Ordering::Relaxed);
        let failed = self.failed.load(Ordering::Relaxed);
        let errored = self.errored.load(Ordering::Relaxed);

        let fail_text = format!("{failed} fail");
        let fail_seg = if failed > 0 {
            self.palette.fail(&fail_text)
        } else {
            fail_text
        };

        // xpasses are gate failures in progress; they render like fails, and
        // only when present. xfails are expected — not news — and join Skip
        // in leaving the bar untouched.
        let xpassed = self.xpassed.load(Ordering::Relaxed);
        let mut head = format!("{passed} pass · {fail_seg} · {errored} err");
        if xpassed > 0 {
            let xpass_text = format!("{xpassed} xpass");
            head.push_str(&format!(" · {}", self.palette.fail(&xpass_text)));
        }
        if running.is_empty() {
            head
        } else {
            format!("{head} · running: {running}")
        }
    }

    /// Set the bar message to reflect the tallies and the given running name.
    fn update_msg(&self, name: &Option<String>) {
        let running = name.as_deref().map(|n| truncate(n, 40)).unwrap_or_default();
        self.bar.set_message(self.render_msg(&running));
    }
}

impl ProgressSink for RunProgressBar {
    fn event(&self, event: &ProgressEvent) {
        match event {
            ProgressEvent::RunStarted { total } => {
                self.bar.set_length(*total as u64);
                self.bar.set_position(0);
            }
            ProgressEvent::CaseStarted { name, .. } => {
                self.update_msg(name);
            }
            ProgressEvent::CaseFinished { status, name, .. } => {
                match status {
                    CaseStatus::Pass => {
                        self.passed.fetch_add(1, Ordering::Relaxed);
                    }
                    CaseStatus::Fail => {
                        self.failed.fetch_add(1, Ordering::Relaxed);
                    }
                    CaseStatus::Error => {
                        self.errored.fetch_add(1, Ordering::Relaxed);
                    }
                    CaseStatus::Skip => {}
                    CaseStatus::XFail => {}
                    CaseStatus::XPass => {
                        self.xpassed.fetch_add(1, Ordering::Relaxed);
                    }
                }
                self.bar.inc(1);
                self.update_msg(name);
            }
            ProgressEvent::RunFinished { .. } => {
                self.bar.finish_and_clear();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_leaves_short_names_unchanged() {
        assert_eq!(truncate("short", 40), "short");
        assert_eq!(truncate("", 40), "");
    }

    #[test]
    fn truncate_caps_long_names_with_ellipsis() {
        let long = "a".repeat(50);
        let out = truncate(&long, 40);
        assert_eq!(out.chars().count(), 40);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn truncate_is_codepoint_safe() {
        // 45 multi-byte chars: truncation must count characters, not bytes, and
        // never split a codepoint.
        let s = "é".repeat(45);
        let out = truncate(&s, 40);
        assert_eq!(out.chars().count(), 40);
    }

    #[test]
    fn render_msg_reflects_tallies_and_running_name() {
        let bar = RunProgressBar::new(Palette::disabled());
        bar.passed.store(12, Ordering::Relaxed);
        bar.failed.store(1, Ordering::Relaxed);
        // Disabled palette → no ANSI, plain segments.
        assert_eq!(
            bar.render_msg("case-x"),
            "12 pass · 1 fail · 0 err · running: case-x"
        );
        // No running name → the segment is dropped.
        assert_eq!(bar.render_msg(""), "12 pass · 1 fail · 0 err");
    }

    /// An operator watching a run wants gate-failing states as they happen:
    /// xpasses get a live segment (only when nonzero — most runs never have
    /// one). Expected failures are not news and stay out of the bar.
    #[test]
    fn render_msg_surfaces_xpasses_only_when_nonzero() {
        let bar = RunProgressBar::new(Palette::disabled());
        bar.passed.store(3, Ordering::Relaxed);
        assert_eq!(bar.render_msg(""), "3 pass · 0 fail · 0 err");
        bar.xpassed.store(2, Ordering::Relaxed);
        assert_eq!(bar.render_msg(""), "3 pass · 0 fail · 0 err · 2 xpass");
    }
}
