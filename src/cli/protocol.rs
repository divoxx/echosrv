//! The protocols both binaries speak: names, aliases and parsing.

use clap::builder::{PossibleValue, TypedValueParser};
use clap::error::ErrorKind;
use clap::{Command, ValueEnum};
use std::ffi::OsStr;
use std::fmt;

/// A protocol served by `echosrv` and spoken by `echosrv-client`.
///
/// The doc comments of the variants are the `--help` text of the
/// `PROTOCOL` argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, ValueEnum)]
pub enum Protocol {
    /// TCP echo server
    Tcp,
    /// UDP echo server
    Udp,
    /// HTTP echo server (echoes POST bodies)
    Http,
    /// Unix domain stream socket server
    UnixStream,
    /// Unix domain datagram socket server (alias: unix-datagram)
    #[value(name = "unix-dgram", alias = "unix-datagram")]
    UnixDatagram,
}

impl Protocol {
    /// Every protocol, in help order.
    pub const ALL: [Protocol; 5] = [
        Self::Tcp,
        Self::Udp,
        Self::Http,
        Self::UnixStream,
        Self::UnixDatagram,
    ];

    /// The name used on the command line and in reports.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tcp => "tcp",
            Self::Udp => "udp",
            Self::Http => "http",
            Self::UnixStream => "unix-stream",
            Self::UnixDatagram => "unix-dgram",
        }
    }

    /// Name used to look up an inherited socket (systemd
    /// `FileDescriptorName=`).
    pub fn service_name(self) -> &'static str {
        match self {
            Self::UnixDatagram => "unix-datagram",
            other => other.as_str(),
        }
    }

    /// Whether the protocol runs over a Unix domain socket (its target is a
    /// socket path rather than a port).
    pub fn is_unix(self) -> bool {
        matches!(self, Self::UnixStream | Self::UnixDatagram)
    }

    /// Whether the protocol is connectionless.
    pub fn is_datagram(self) -> bool {
        matches!(self, Self::Udp | Self::UnixDatagram)
    }

    /// Parses a protocol name case-insensitively, aliases included.
    ///
    /// # Errors
    ///
    /// `unknown protocol '<name>' (possible values: ...)` for any other name.
    pub fn from_name(name: &str) -> Result<Self, String> {
        <Self as ValueEnum>::from_str(name, true).map_err(|_| {
            let names: Vec<_> = Self::ALL.iter().map(|p| p.as_str()).collect();
            format!(
                "unknown protocol '{name}' (possible values: {})",
                names.join(", ")
            )
        })
    }

    /// The clap value parser for a `PROTOCOL` argument (see [`ProtocolParser`]).
    pub fn parser() -> ProtocolParser {
        ProtocolParser
    }
}

impl fmt::Display for Protocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Parses a [`Protocol`] with [`Protocol::from_name`], so a bad name is a
/// clap usage error reading `unknown protocol '<name>'`. The possible values
/// still appear in the help.
#[derive(Debug, Clone, Copy)]
pub struct ProtocolParser;

impl TypedValueParser for ProtocolParser {
    type Value = Protocol;

    fn parse_ref(
        &self,
        cmd: &Command,
        _arg: Option<&clap::Arg>,
        value: &OsStr,
    ) -> Result<Protocol, clap::Error> {
        Protocol::from_name(&value.to_string_lossy())
            .map_err(|msg| cmd.clone().error(ErrorKind::InvalidValue, msg))
    }

    fn possible_values(&self) -> Option<Box<dyn Iterator<Item = PossibleValue> + '_>> {
        Some(Box::new(
            Protocol::value_variants()
                .iter()
                .filter_map(ValueEnum::to_possible_value),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip_and_match_clap() {
        for p in Protocol::ALL {
            assert_eq!(Protocol::from_name(p.as_str()), Ok(p));
            assert_eq!(p.to_string(), p.as_str());
            let clap_name = p.to_possible_value().unwrap().get_name().to_owned();
            assert_eq!(clap_name, p.as_str());
        }
        assert_eq!(Protocol::value_variants(), Protocol::ALL);
    }

    #[test]
    fn parsing_is_case_insensitive_with_alias() {
        assert_eq!(Protocol::from_name("HTTP"), Ok(Protocol::Http));
        assert_eq!(Protocol::from_name("Unix-Stream"), Ok(Protocol::UnixStream));
        assert_eq!(
            Protocol::from_name("unix-datagram"),
            Ok(Protocol::UnixDatagram)
        );
        assert_eq!(
            Protocol::from_name("UNIX-DGRAM"),
            Ok(Protocol::UnixDatagram)
        );
        let err = Protocol::from_name("gopher").unwrap_err();
        assert_eq!(
            err,
            "unknown protocol 'gopher' (possible values: tcp, udp, http, unix-stream, unix-dgram)"
        );
    }

    #[test]
    fn service_names_and_kinds() {
        let service: Vec<_> = Protocol::ALL.iter().map(|p| p.service_name()).collect();
        assert_eq!(
            service,
            ["tcp", "udp", "http", "unix-stream", "unix-datagram"]
        );
        let unix: Vec<_> = Protocol::ALL.iter().map(|p| p.is_unix()).collect();
        assert_eq!(unix, [false, false, false, true, true]);
        let dgram: Vec<_> = Protocol::ALL.iter().map(|p| p.is_datagram()).collect();
        assert_eq!(dgram, [false, true, false, false, true]);
    }
}
