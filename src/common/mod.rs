//! Traits shared by every echo server and client.
//!
//! [`EchoServerTrait`] is implemented by all servers and [`EchoClient`] by all
//! clients, so code can be written once against either protocol family.

pub(crate) mod lifecycle;
pub mod traits;

pub use traits::{EchoClient, EchoServerTrait};
