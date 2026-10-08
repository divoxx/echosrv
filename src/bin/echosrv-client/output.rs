//! Terminal styling: color resolution, a per-stream [`Palette`] and tagged
//! status lines.
//!
//! Status lines follow the same layout as tinywyrd's CLI: tags are
//! right-aligned within a 6-char column and the message starts at column 8;
//! continuation lines are indented to the message column.
//!
//! ```text
//! [info]  message
//!   [ok]  message
//! [warn]  message
//! [fail]  message
//!         continuation
//! ```
//!
//! Diagnostics ([`info`], [`warn`], [`fail`]) go to stderr. The report on
//! stdout uses [`tagged`] with the stdout palette.
//!
//! Color is decided once per stream ([`resolve_color`]) and carried in a
//! [`Palette`]; a plain palette never emits ANSI codes. The `colored` crate's
//! own detection only looks at stdout, so [`enable_ansi`] forces it on and
//! the palette is the single gate.

use clap::ValueEnum;
use colored::{ColoredString, Colorize};
use std::ffi::OsStr;

/// Spaces for continuation lines: 6 (tag column) + 1 (separator).
const INDENT: &str = "       ";

/// `--color` setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ColorChoice {
    /// Color when the stream is a terminal (honors `NO_COLOR`, `CLICOLOR_FORCE`).
    Auto,
    /// Always color (human output only; JSON is never colored).
    Always,
    /// Never color.
    Never,
}

/// Color-related environment variables.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ColorEnv {
    /// `NO_COLOR` is set to a non-empty value.
    pub no_color: bool,
    /// `CLICOLOR_FORCE` is set to a non-empty value other than `0`.
    pub clicolor_force: bool,
}

impl ColorEnv {
    pub fn from_env() -> Self {
        Self::from_vars(
            std::env::var_os("NO_COLOR").as_deref(),
            std::env::var_os("CLICOLOR_FORCE").as_deref(),
        )
    }

    pub fn from_vars(no_color: Option<&OsStr>, clicolor_force: Option<&OsStr>) -> Self {
        Self {
            no_color: no_color.is_some_and(|v| !v.is_empty()),
            clicolor_force: clicolor_force.is_some_and(|v| !v.is_empty() && v != "0"),
        }
    }
}

/// Decides whether a stream gets color.
///
/// `always`/`never` win outright. For `auto`, `CLICOLOR_FORCE` enables color
/// even when piped, then `NO_COLOR` disables it, and otherwise color follows
/// whether the stream is a terminal.
pub fn resolve_color(choice: ColorChoice, is_terminal: bool, env: ColorEnv) -> bool {
    match choice {
        ColorChoice::Always => true,
        ColorChoice::Never => false,
        ColorChoice::Auto if env.clicolor_force => true,
        ColorChoice::Auto if env.no_color => false,
        ColorChoice::Auto => is_terminal,
    }
}

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

    const NONE: ColorEnv = ColorEnv {
        no_color: false,
        clicolor_force: false,
    };
    const NO_COLOR: ColorEnv = ColorEnv {
        no_color: true,
        clicolor_force: false,
    };
    const FORCE: ColorEnv = ColorEnv {
        no_color: false,
        clicolor_force: true,
    };
    const BOTH: ColorEnv = ColorEnv {
        no_color: true,
        clicolor_force: true,
    };

    #[test]
    fn color_resolution() {
        use ColorChoice::{Always, Auto, Never};
        for env in [NONE, NO_COLOR, FORCE, BOTH] {
            for tty in [false, true] {
                assert!(resolve_color(Always, tty, env), "{env:?} tty={tty}");
                assert!(!resolve_color(Never, tty, env), "{env:?} tty={tty}");
            }
        }
        assert!(resolve_color(Auto, true, NONE));
        assert!(!resolve_color(Auto, false, NONE));
        assert!(!resolve_color(Auto, true, NO_COLOR));
        assert!(!resolve_color(Auto, false, NO_COLOR));
        assert!(resolve_color(Auto, false, FORCE));
        assert!(resolve_color(Auto, true, FORCE));
        // CLICOLOR_FORCE beats NO_COLOR.
        assert!(resolve_color(Auto, false, BOTH));
    }

    #[test]
    fn color_env_values() {
        let os = |s: &'static str| Some(OsStr::new(s));
        assert_eq!(ColorEnv::from_vars(None, None), NONE);
        assert_eq!(ColorEnv::from_vars(os(""), os("")), NONE);
        assert_eq!(ColorEnv::from_vars(os("1"), os("0")), NO_COLOR);
        assert_eq!(ColorEnv::from_vars(None, os("1")), FORCE);
        assert_eq!(ColorEnv::from_vars(os("yes"), os("yes")), BOTH);
    }

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
