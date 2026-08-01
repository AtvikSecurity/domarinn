//! The CLI color/styling layer: a NO_COLOR/TTY-aware [`Palette`].
//!
//! Human output (the results table) is the only thing that gets colored; the
//! machine formats (json/jsonl/junit) never receive a [`Palette`], which is the
//! structural guarantee that their bytes are color-proof. Detection is a pure
//! function ([`resolve`]) so the precedence rules are exhaustively unit-tested;
//! [`Palette::detect`] is the thin wrapper that reads the real environment/TTY.

use std::io::IsTerminal;

use anstyle::{AnsiColor, Style};

/// The user's `--color` preference.
#[derive(clap::ValueEnum, Clone, Copy, Default, Debug, PartialEq, Eq)]
pub enum ColorChoice {
    /// Color when stdout is a terminal (honors `NO_COLOR` / `CLICOLOR_FORCE`).
    #[default]
    Auto,
    /// Always emit ANSI color.
    Always,
    /// Never emit ANSI color.
    Never,
}

/// A resolved decision about whether to color, plus the styling helpers.
///
/// `Copy` so it can be threaded by value through the command executors and the
/// renderers without ceremony.
#[derive(Clone, Copy)]
pub struct Palette {
    enabled: bool,
}

/// The pure color-enablement decision. Precedence, highest first:
///
/// 1. `--color always` / `--color never` (an explicit choice wins outright).
/// 2. `CLICOLOR_FORCE` set to a non-empty, non-`"0"` value → force color on.
/// 3. `NO_COLOR` present in the environment (any value) → color off. This
///    matches the logging layer's `var_os("NO_COLOR").is_none()` convention,
///    so the CLI and its logs agree on what "no color" means.
/// 4. Otherwise, color iff stdout is a terminal.
fn resolve(
    choice: ColorChoice,
    no_color: Option<&str>,
    clicolor_force: Option<&str>,
    is_tty: bool,
) -> bool {
    match choice {
        ColorChoice::Always => return true,
        ColorChoice::Never => return false,
        ColorChoice::Auto => {}
    }
    if clicolor_force.is_some_and(|v| !v.is_empty() && v != "0") {
        return true;
    }
    if no_color.is_some() {
        return false;
    }
    is_tty
}

impl Palette {
    /// Resolve a palette from the `--color` choice, the real environment, and
    /// whether stdout is a terminal.
    pub fn detect(choice: ColorChoice) -> Self {
        Palette {
            enabled: resolve(
                choice,
                std::env::var("NO_COLOR").ok().as_deref(),
                std::env::var("CLICOLOR_FORCE").ok().as_deref(),
                std::io::stdout().is_terminal(),
            ),
        }
    }

    /// A palette that never colors — used for `--out` file output and every
    /// machine format.
    pub fn disabled() -> Self {
        Palette { enabled: false }
    }

    /// Whether this palette emits ANSI escapes.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Wrap `text` in `style`'s escapes when enabled; return it verbatim
    /// otherwise (so a disabled palette is a guaranteed passthrough).
    fn paint(&self, style: Style, text: &str) -> String {
        if self.enabled {
            format!("{}{text}{}", style.render(), style.render_reset())
        } else {
            text.to_string()
        }
    }

    /// A passing status (green).
    pub fn pass(&self, text: &str) -> String {
        self.paint(Style::new().fg_color(Some(AnsiColor::Green.into())), text)
    }

    /// A failing status (red).
    pub fn fail(&self, text: &str) -> String {
        self.paint(Style::new().fg_color(Some(AnsiColor::Red.into())), text)
    }

    /// An infrastructure error (yellow).
    pub fn error(&self, text: &str) -> String {
        self.paint(Style::new().fg_color(Some(AnsiColor::Yellow.into())), text)
    }

    /// Advice the reader may ignore (yellow).
    ///
    /// Deliberately not an alias for [`Self::error`], which is yellow because
    /// the *results table's* infra-ERROR status is yellow — a different concept
    /// that happens to share a color today. Separate names let either move.
    pub fn warn(&self, text: &str) -> String {
        self.paint(Style::new().fg_color(Some(AnsiColor::Yellow.into())), text)
    }

    /// A skipped status (dim).
    pub fn skip(&self, text: &str) -> String {
        self.paint(Style::new().dimmed(), text)
    }

    /// An added item (green) — used by diff rendering.
    pub fn added(&self, text: &str) -> String {
        self.paint(Style::new().fg_color(Some(AnsiColor::Green.into())), text)
    }

    /// A removed item (red) — used by diff rendering.
    pub fn removed(&self, text: &str) -> String {
        self.paint(Style::new().fg_color(Some(AnsiColor::Red.into())), text)
    }

    /// A table header (bold).
    pub fn header(&self, text: &str) -> String {
        self.paint(Style::new().bold(), text)
    }

    /// De-emphasized text (dim) — used by the diff and case-detail views.
    pub fn dim(&self, text: &str) -> String {
        self.paint(Style::new().dimmed(), text)
    }
}

#[cfg(test)]
impl Palette {
    /// A palette with a forced enablement state, for renderer tests in sibling
    /// modules (the `enabled` field is otherwise private).
    pub(crate) fn for_test(enabled: bool) -> Self {
        Palette { enabled }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ESC: char = '\x1b';

    #[test]
    fn resolve_flag_always_and_never_ignore_environment() {
        // An explicit flag wins over CLICOLOR_FORCE, NO_COLOR, and the TTY.
        assert!(resolve(ColorChoice::Always, Some("1"), None, false));
        assert!(resolve(ColorChoice::Always, None, Some("0"), false));
        assert!(!resolve(ColorChoice::Never, None, Some("1"), true));
        assert!(!resolve(ColorChoice::Never, None, None, true));
    }

    #[test]
    fn resolve_clicolor_force_beats_no_color_and_tty() {
        // CLICOLOR_FORCE forces color even when NO_COLOR is set and stdout is not
        // a terminal.
        assert!(resolve(ColorChoice::Auto, Some("1"), Some("1"), false));
        assert!(resolve(ColorChoice::Auto, None, Some("1"), false));
        // Empty / "0" do not force.
        assert!(!resolve(ColorChoice::Auto, None, Some(""), false));
        assert!(!resolve(ColorChoice::Auto, None, Some("0"), false));
    }

    #[test]
    fn resolve_no_color_beats_tty() {
        // NO_COLOR present (any value, including empty) disables color even on a
        // TTY, as long as CLICOLOR_FORCE is not forcing.
        assert!(!resolve(ColorChoice::Auto, Some("1"), None, true));
        assert!(!resolve(ColorChoice::Auto, Some(""), None, true));
    }

    #[test]
    fn resolve_falls_back_to_tty() {
        assert!(resolve(ColorChoice::Auto, None, None, true));
        assert!(!resolve(ColorChoice::Auto, None, None, false));
    }

    #[test]
    fn disabled_palette_is_verbatim_passthrough() {
        let p = Palette::disabled();
        for painted in [
            p.pass("x"),
            p.fail("x"),
            p.error("x"),
            p.skip("x"),
            p.added("x"),
            p.removed("x"),
            p.header("x"),
            p.dim("x"),
        ] {
            assert_eq!(painted, "x");
            assert!(!painted.contains(ESC));
        }
        assert!(!p.enabled());
    }

    #[test]
    fn enabled_palette_wraps_glyph_bytes_unchanged() {
        let p = Palette::for_test(true);
        let painted = p.pass("PASS");
        // The token itself is present verbatim inside the escapes.
        assert!(painted.contains("PASS"));
        assert!(painted.starts_with(ESC));
        // A reset closes the sequence so styling never bleeds into later output.
        assert!(painted.ends_with("m"));
        assert!(p.enabled());
    }
}
