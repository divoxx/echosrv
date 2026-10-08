//! Whether a terminal stream gets color, from a `--color` choice and the
//! `NO_COLOR` / `CLICOLOR_FORCE` environment variables.

use clap::ValueEnum;
use std::ffi::OsStr;

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
    /// Reads `NO_COLOR` and `CLICOLOR_FORCE` from the process environment.
    pub fn from_env() -> Self {
        Self::from_vars(
            std::env::var_os("NO_COLOR").as_deref(),
            std::env::var_os("CLICOLOR_FORCE").as_deref(),
        )
    }

    /// Interprets the values of `NO_COLOR` and `CLICOLOR_FORCE` (`None` when
    /// unset). An empty value counts as unset, per <https://no-color.org>.
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
}
