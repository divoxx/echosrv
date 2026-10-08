//! Help-text layout shared by the command-line binaries.
//!
//! Defaults that clap cannot print itself (optional or computed values) are
//! written inline at the end of an argument's doc comment, as
//! `... [default: X]`. That keeps them in the compact `-h` summary. For
//! `--help` they are moved onto their own line, matching how clap renders
//! the `[default: X]` of a `default_value`.
//!
//! This file is a module of each binary rather than of the library, so the
//! library does not depend on clap. Each binary uses a subset of it.

use clap::{ArgMatches, Command, CommandFactory, FromArgMatches};
use std::ffi::OsString;

/// `T::command()` with inline ` [default: …]` suffixes moved to their own
/// paragraph in the long help.
pub fn command<T: CommandFactory>() -> Command {
    T::command().mut_args(|arg| {
        if arg.get_long_help().is_some() {
            return arg;
        }
        let Some(help) = arg.get_help().map(ToString::to_string) else {
            return arg;
        };
        match help.split_once(" [default: ") {
            Some((text, default)) => arg.long_help(format!("{text}\n\n[default: {default}")),
            None => arg,
        }
    })
}

/// Drop-in for `T::parse()` that uses [`command`]. On `--help`,
/// `--version` or a usage error it prints and exits like clap (status 0 or 2).
#[allow(dead_code)] // Not every binary uses every entry point.
pub fn parse<T: CommandFactory + FromArgMatches>() -> T {
    parse_with_matches::<T>().0
}

/// Like [`parse`], but also returns the [`ArgMatches`], e.g. to tell values
/// given on the command line from defaults via `ArgMatches::value_source`.
#[allow(dead_code)] // Not every binary uses every entry point.
pub fn parse_with_matches<T: CommandFactory + FromArgMatches>() -> (T, ArgMatches) {
    let mut cmd = command::<T>();
    let matches = cmd.get_matches_mut();
    let parsed = T::from_arg_matches(&matches).unwrap_or_else(|e| e.format(&mut cmd).exit());
    (parsed, matches)
}

/// Like [`parse_with_matches`], but parses `args` (including the program
/// name) and returns errors instead of exiting.
///
/// `--help` and `--version` are returned as errors of kind
/// [`DisplayHelp`](clap::error::ErrorKind::DisplayHelp) /
/// [`DisplayVersion`](clap::error::ErrorKind::DisplayVersion) that carry the
/// rendered text; `print()` them to show it.
pub fn try_parse_from<T, I, A>(args: I) -> Result<(T, ArgMatches), clap::Error>
where
    T: CommandFactory + FromArgMatches,
    I: IntoIterator<Item = A>,
    A: Into<OsString> + Clone,
{
    let mut cmd = command::<T>();
    let matches = cmd.try_get_matches_from_mut(args)?;
    let parsed = T::from_arg_matches(&matches).map_err(|e| e.format(&mut cmd))?;
    Ok((parsed, matches))
}
