//! Terminal styling: a per-stream [`Palette`] and tagged status lines.
//!
//! Status lines put the tag right-aligned in a 6-char column, then one
//! space, so the message starts at column 8; continuation lines are indented
//! to the message column.
//!
//! ```text
//! [info] message
//!   [ok] message
//! [warn] message
//! [fail] message
//!        continuation
//! ```
//!
//! Diagnostics ([`info`], [`warn`], [`fail`]) go to stderr. The report on
//! stdout uses [`tagged`] with the stdout palette.
//!
//! Color is decided once per stream
//! ([`resolve_color`](echosrv::cli::color::resolve_color)) and carried in a
//! [`Palette`]; a plain palette never emits ANSI codes. The `colored` crate's
//! own detection only looks at stdout, so [`enable_ansi`] forces it on and
//! the palette is the single gate.

use colored::{ColoredString, Colorize};

/// Spaces for continuation lines: 6 (tag column) + 1 (separator).
const INDENT: &str = "       ";

/// Makes `colored` emit ANSI codes whenever asked; [`Palette`] decides when
/// to ask.
pub fn enable_ansi() {
    colored::control::set_override(true);
}

/// Styling for one output stream. A plain palette returns text unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    color: bool,
}

impl Palette {
    pub const PLAIN: Palette = Palette { color: false };

    pub fn new(color: bool) -> Self {
        Self { color }
    }

    fn style(self, s: &str, f: impl FnOnce(&str) -> ColoredString) -> String {
        if self.color {
            f(s).to_string()
        } else {
            s.to_string()
        }
    }

    /// Muted text (timestamps, zero counts, decoration).
    pub fn dim(self, s: &str) -> String {
        self.style(s, |s| s.dimmed())
    }

    /// Highlighted values.
    pub fn bold(self, s: &str) -> String {
        self.style(s, |s| s.bold())
    }

    pub fn green(self, s: &str) -> String {
        self.style(s, |s| s.green())
    }

    pub fn red(self, s: &str) -> String {
        self.style(s, |s| s.red())
    }

    pub fn bold_red(self, s: &str) -> String {
        self.style(s, |s| s.red().bold())
    }

    pub fn yellow(self, s: &str) -> String {
        self.style(s, |s| s.yellow())
    }

    pub fn cyan(self, s: &str) -> String {
        self.style(s, |s| s.cyan())
    }
}

/// Status tags, rendered right-aligned in a 6-char column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tag {
    Info,
    Ok,
    Warn,
    Fail,
}

impl Tag {
    /// The padded, styled tag (always 6 visible chars).
    pub fn render(self, p: Palette) -> String {
        match self {
            Tag::Info => p.cyan("[info]"),
            Tag::Ok => format!("  {}", p.green("[ok]")),
            Tag::Warn => p.yellow("[warn]"),
            Tag::Fail => p.red("[fail]"),
        }
    }
}

/// `<tag> message`, with any further message lines indented to the message
/// column.
pub fn tagged(p: Palette, tag: Tag, msg: &str) -> String {
    let mut lines = msg.lines();
    let mut out = format!("{} {}", tag.render(p), lines.next().unwrap_or_default());
    for line in lines {
        out.push('\n');
        out.push_str(INDENT);
        out.push_str(line);
    }
    out
}

/// Status line on stderr.
pub fn info(p: Palette, msg: &str) {
    eprintln!("{}", tagged(p, Tag::Info, msg));
}

/// Warning on stderr.
pub fn warn(p: Palette, msg: &str) {
    eprintln!("{}", tagged(p, Tag::Warn, msg));
}

/// Failure on stderr.
pub fn fail(p: Palette, msg: &str) {
    eprintln!("{}", tagged(p, Tag::Fail, msg));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tagged_layout() {
        let p = Palette::PLAIN;
        assert_eq!(tagged(p, Tag::Info, "hi"), "[info] hi");
        assert_eq!(tagged(p, Tag::Ok, "hi"), "  [ok] hi");
        assert_eq!(tagged(p, Tag::Warn, "hi"), "[warn] hi");
        assert_eq!(tagged(p, Tag::Fail, "a\nb"), "[fail] a\n       b");
    }

    #[test]
    fn palette_gates_ansi() {
        enable_ansi();
        assert_eq!(Palette::PLAIN.red("x"), "x");
        let colored = Palette::new(true);
        assert!(colored.red("x").contains("\x1b["));
        assert!(tagged(colored, Tag::Warn, "hi").contains("\x1b["));
        assert!(tagged(colored, Tag::Ok, "hi").ends_with(" hi"));
    }
}
