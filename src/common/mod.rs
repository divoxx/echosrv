//! Traits and types shared by every echo server and client.
//!
//! [`EchoServerTrait`] is implemented by all servers and [`EchoClient`] by all
//! clients, so code can be written once against either protocol family.
//! [`ServerStats`] holds the counters every server keeps.

pub(crate) mod lifecycle;
pub mod stats;
pub mod traits;

pub use stats::ServerStats;
pub use traits::{EchoClient, EchoServerTrait};
